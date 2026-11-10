//! A custom-app ask's `wait_seconds` (`agent_ask::wait`) against a real run
//! table: what counts as settled, that a NOTIFY ring ends the wait as soon as
//! the run does, and that a wait that runs out says so instead of guessing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_runtime::lifecycle::remote_wake::{RemoteWakes, RunEventWaker};
use oxy_app::server::api::projects::agent_ask::wait::{Settled, wait_for_settle};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

struct World {
    db: DatabaseConnection,
    wakes: Arc<RemoteWakes>,
}

impl World {
    async fn new() -> Self {
        let (db, _) = fresh_db(Schema::All).await;
        Self {
            db,
            wakes: Arc::new(RemoteWakes::new()),
        }
    }

    async fn run(&self, status: &str, answer: Option<&str>, error: Option<&str>) -> String {
        let id = Uuid::new_v4().to_string();
        exec(
            &self.db,
            "INSERT INTO agentic_runs \
               (id, question, answer, error_message, task_status, workspace_id, created_at, updated_at) \
             VALUES ($1, 'q', $2, $3, $4, $5, now(), now())",
            vec![
                id.clone().into(),
                answer.map(str::to_string).into(),
                error.map(str::to_string).into(),
                status.into(),
                Uuid::new_v4().into(),
            ],
        )
        .await;
        id
    }

    async fn suspend(&self, run: &str, prompt: &str, suggestions: &[&str]) {
        exec(
            &self.db,
            "INSERT INTO agentic_run_suspensions (run_id, prompt, suggestions, resume_data, created_at) \
             VALUES ($1, $2, $3, '{}', now())",
            vec![
                run.into(),
                prompt.into(),
                serde_json::json!(suggestions).into(),
            ],
        )
        .await;
    }

    async fn finish(&self, run: &str, answer: &str) {
        exec(
            &self.db,
            "UPDATE agentic_runs SET task_status = 'done', answer = $2 WHERE id = $1",
            vec![run.into(), answer.into()],
        )
        .await;
    }

    async fn wait(&self, run: &str, wait: Duration) -> Option<Settled> {
        wait_for_settle(&self.db, &self.wakes, run, wait)
            .await
            .expect("wait read")
    }
}

#[tokio::test]
async fn a_finished_run_answers_at_once() {
    let w = World::new().await;
    let run = w.run("done", Some("42 orders"), None).await;
    let started = Instant::now();
    assert_eq!(
        w.wait(&run, Duration::from_secs(10)).await,
        Some(Settled::Done {
            answer: "42 orders".into()
        })
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn a_failed_or_timed_out_run_carries_its_error() {
    let w = World::new().await;
    let failed = w.run("failed", None, Some("no warehouse")).await;
    let slow = w.run("timed_out", None, None).await;
    assert_eq!(
        w.wait(&failed, Duration::from_secs(5)).await,
        Some(Settled::Failed {
            timed_out: false,
            error: "no warehouse".into()
        })
    );
    assert!(matches!(
        w.wait(&slow, Duration::from_secs(5)).await,
        Some(Settled::Failed {
            timed_out: true,
            ..
        })
    ));
}

/// The ring, not the once-a-second recheck, is what ends the wait: the run
/// finishes 300 ms in and the wait returns well before its first recheck.
#[tokio::test]
async fn a_ring_ends_the_wait_as_soon_as_the_run_finishes() {
    let w = Arc::new(World::new().await);
    let run = w.run("running", None, None).await;
    let finisher = {
        let (w, run) = (Arc::clone(&w), run.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            w.finish(&run, "done now").await;
            w.wakes.wake_run(&run);
        })
    };
    let started = Instant::now();
    let settled = w.wait(&run, Duration::from_secs(10)).await;
    finisher.await.unwrap();
    assert_eq!(
        settled,
        Some(Settled::Done {
            answer: "done now".into()
        })
    );
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "took {:?}: the ring was not what woke the wait",
        started.elapsed()
    );
}

/// A run whose terminal update lands without a ring (the row is written just
/// after the last event's NOTIFY) is still found, by the recheck.
#[tokio::test]
async fn a_finish_with_no_ring_is_found_by_the_recheck() {
    let w = Arc::new(World::new().await);
    let run = w.run("running", None, None).await;
    let finisher = {
        let (w, run) = (Arc::clone(&w), run.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            w.finish(&run, "quietly").await;
        })
    };
    let settled = w.wait(&run, Duration::from_secs(10)).await;
    finisher.await.unwrap();
    assert_eq!(
        settled,
        Some(Settled::Done {
            answer: "quietly".into()
        })
    );
}

#[tokio::test]
async fn a_run_parked_on_a_question_returns_the_question() {
    let w = World::new().await;
    let run = w.run("awaiting_input", None, None).await;
    w.suspend(&run, "Which region?", &["EU", "US"]).await;
    assert_eq!(
        w.wait(&run, Duration::from_secs(5)).await,
        Some(Settled::AwaitingInput {
            question: "Which region?".into(),
            suggestions: vec!["EU".into(), "US".into()],
        })
    );
}

/// The status can flip before the question is written; until it is, the run
/// is not settled, so a wait never answers `awaiting_input` with no question.
#[tokio::test]
async fn a_parked_run_with_no_question_yet_is_still_going() {
    let w = World::new().await;
    let run = w.run("awaiting_input", None, None).await;
    assert_eq!(w.wait(&run, Duration::from_millis(1200)).await, None);
}

#[tokio::test]
async fn a_run_still_going_at_the_deadline_is_not_guessed_at() {
    let w = World::new().await;
    let run = w.run("running", None, None).await;
    let started = Instant::now();
    assert_eq!(w.wait(&run, Duration::from_millis(1500)).await, None);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(1500),
        "returned early: {took:?}"
    );
    assert!(took < Duration::from_secs(3), "overran the wait: {took:?}");
}
