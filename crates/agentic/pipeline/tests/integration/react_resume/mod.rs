//! A served react run resumes from its event log: after a restart with a
//! person's answer, and in crash recovery with none.
//!
//! Driven the way production drives it: a latency-worker pass claims the
//! queued chat (`recover_pending_global_runs`), a coordinator persists its
//! rows and suspension, `queue_answer` queues the `TaskSpec::Resume`, and a
//! second pass, on a fresh platform with a fresh engine, continues it. Nothing
//! survives in memory between the two passes, so the resume can only read
//! the database. The engine is scripted (`ProjectContext::react_engine`);
//! no test calls a model.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(react_resume)'

mod host;
mod prior_turns;
mod readiness;
mod redrive;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use agentic_core::human_input::SuspendedRunData;
use agentic_core::react::{Decision, Entry};
use agentic_pipeline::platform::{IdentityResolver, PlatformContext};
use agentic_pipeline::queued_answer::{QueuedAnswer, queue_answer};
use agentic_pipeline::queued_run::{QueuedAnalyticsRun, chat_run_spec, seed_queued_analytics_run};
use agentic_pipeline::recovery::DrivePolicy;
use agentic_runtime::crud;
use agentic_runtime::react::ReactEvent;
use agentic_runtime::state::RuntimeState;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use self::host::{Host, ask_user, latency_pass, turn};
use crate::automation_recovery_test::test_db;

const AGENT: &str = "agents/sales.agentic.yml";
const QUESTION: &str = "which year sold most?";

/// The shared test Postgres. A missing database fails the test: it must not
/// pass by skipping.
async fn db() -> DatabaseConnection {
    test_db().await.expect("the shared test Postgres")
}

// ── Driving ─────────────────────────────────────────────────────────────────

/// Polls the run row until `check` holds, or fails naming the last status.
async fn until(
    db: &DatabaseConnection,
    run_id: &str,
    what: &str,
    check: impl Fn(&agentic_runtime::entity::run::Model) -> bool,
) -> agentic_runtime::entity::run::Model {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let run = crud::get_run(db, run_id).await.unwrap().expect("run");
        if check(&run) {
            return run;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: run {:?} driver {:?} error {:?}",
            run.task_status,
            run.driver_id,
            run.error_message
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn ended(run: &agentic_runtime::entity::run::Model) -> bool {
    matches!(run.task_status.as_deref(), Some("done" | "failed"))
}

async fn rows(db: &DatabaseConnection, run_id: &str) -> Vec<(String, Value)> {
    crud::get_all_events(db, run_id)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.event_type, e.payload))
        .collect()
}

fn count(rows: &[(String, Value)], t: &str) -> usize {
    rows.iter().filter(|(et, _)| et == t).count()
}

/// A react run on `ws` with `rows` on its log, as an earlier attempt wrote
/// them: `runtime: react` metadata, `running`.
async fn seed_react_run(db: &DatabaseConnection, ws: Uuid, rows: &[(String, Value)]) -> String {
    let run_id = format!("react-{}", Uuid::new_v4());
    let metadata = json!({ "agent_id": AGENT, "runtime": "react" });
    crud::insert_run(db, &run_id, QUESTION, None, "analytics", Some(metadata), ws)
        .await
        .unwrap();
    append(db, &run_id, rows).await;
    run_id
}

/// Appends `rows` to the run's log after what is there.
async fn append(db: &DatabaseConnection, run_id: &str, rows: &[(String, Value)]) {
    let mut seq = crud::get_max_seq(db, run_id).await.unwrap() + 1;
    for (t, p) in rows {
        crud::insert_event(db, run_id, seq, t, p, 0).await.unwrap();
        seq += 1;
    }
}

/// The process that drove the run is gone: it is `running` and its lease
/// stopped beating ten minutes ago.
async fn lapse(db: &DatabaseConnection, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = 'running', driver_id = 'direct-dead', \
             driver_heartbeat_at = now() - interval '10 minutes', \
             updated_at = now() - interval '10 minutes' \
         WHERE id = $1",
        [run_id.into()],
    ))
    .await
    .unwrap();
}

/// One stranded-run recovery pass over `ws` on `host`, as a worker runs it.
async fn recover_stranded(db: &DatabaseConnection, ws: Uuid, host: &Arc<Host>) {
    let platform: Arc<dyn PlatformContext> = host.clone();
    agentic_pipeline::recovery::recover_stranded_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        platform,
        Arc::new(IdentityResolver),
        None,
        agentic_runtime::router::noop_router(),
        Some(ws),
        None,
        DrivePolicy::ALL,
    )
    .await;
}

/// Seeds a queued chat on `ws`, lets one process drive it to its `ask_user`,
/// and waits until it is parked with nothing driving it.
async fn parked_react_run(db: &DatabaseConnection, ws: Uuid) -> String {
    let run_id = queued_chat(db, ws).await;
    let starter = Host::new(true, vec![ask_user()]);
    latency_pass(db, ws, &starter).await;
    until(db, &run_id, "the react run never parked", |r| {
        r.task_status.as_deref() == Some("awaiting_input") && r.driver_id.is_none()
    })
    .await;
    assert_eq!(starter.engines_built.load(Ordering::SeqCst), 1);
    run_id
}

/// A chat on `ws` seeded as a request seeds one: the run and its `queued`
/// Global entry, which the next latency pass claims.
async fn queued_chat(db: &DatabaseConnection, ws: Uuid) -> String {
    queued_chat_on(db, ws, None).await
}

/// [`queued_chat`] on `thread`.
async fn queued_chat_on(db: &DatabaseConnection, ws: Uuid, thread: Option<Uuid>) -> String {
    let run = QueuedAnalyticsRun {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: ws,
        agent_id: AGENT.into(),
        question: QUESTION.into(),
        thread_id: thread,
        thinking_mode: Default::default(),
        metadata: Default::default(),
        spec: chat_run_spec(),
    };
    seed_queued_analytics_run(db, &run).await.expect("seed");
    run.run_id
}

/// Parks a react run, answers it `2024` from the queue, and lets a second
/// process, `resumer`, continue it. Returns the run and that process.
async fn answered_after_a_restart(resumer: Arc<Host>) -> (DatabaseConnection, String, Arc<Host>) {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = parked_react_run(&db, ws).await;

    let checkpoint = crud::get_suspension(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(checkpoint.from_state, "react");
    assert_eq!(checkpoint.stage_data["call_id"], "c1");
    let row = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(row.metadata.unwrap()["runtime"], "react");

    assert_eq!(
        queue_answer(&db, &run_id, "2024").await.unwrap(),
        QueuedAnswer::Queued
    );
    latency_pass(&db, ws, &resumer).await;
    until(&db, &run_id, "the resumed run never ended", ended).await;
    (db, run_id, resumer)
}

/// The reply the resumed run's model gives once it has the answer.
fn reply() -> Decision {
    Decision::Reply(turn("2024 sold most", vec![]))
}

/// The resumed engine's first transcript carries the answer as the parked
/// call's result, and the run ends with exactly one terminal row: `done`.
async fn assert_resumed_with_the_answer(db: &DatabaseConnection, run_id: &str, host: &Host) {
    let run = crud::get_run(db, run_id).await.unwrap().unwrap();
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "error: {:?}",
        run.error_message
    );
    assert_eq!(host.engines_built.load(Ordering::SeqCst), 1, "rebuilt once");
    assert_eq!(host.fsm_params.load(Ordering::SeqCst), 0, "not the fsm");
    let seen = host.seen();
    assert_eq!(seen.len(), 1, "one model call after the answer");
    let answer = Entry::ToolResult {
        call_id: "c1".into(),
        text: "2024".into(),
        is_error: false,
    };
    assert_eq!(
        seen[0].entries.last(),
        Some(&answer),
        "{:?}",
        seen[0].entries
    );
    let rows = rows(db, run_id).await;
    assert_eq!(count(&rows, "done"), 1, "{rows:?}");
    assert_eq!(count(&rows, "error"), 0, "{rows:?}");
    assert_eq!(count(&rows, "awaiting_input"), 1, "asked once: {rows:?}");
    assert_eq!(count(&rows, "react_turn"), 2, "{rows:?}");
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn react_run_resumes_with_the_answer_after_a_restart() {
    let resumer = Host::new(true, vec![reply()]);
    let (db, run_id, resumer) = answered_after_a_restart(resumer).await;
    assert_resumed_with_the_answer(&db, &run_id, &resumer).await;
}

/// Only where a run starts reads the flag and the agent's `runtime:` key:
/// on a process whose flag is off, with an agent that no longer opts in, a
/// parked react run still finishes on react.
#[tokio::test(flavor = "multi_thread")]
async fn react_resume_ignores_the_flag() {
    let resumer = Arc::new(Host {
        agent: "databases: [warehouse]\n",
        ..Host::build(false, vec![reply()])
    });
    let (db, run_id, resumer) = answered_after_a_restart(resumer).await;
    assert_resumed_with_the_answer(&db, &run_id, &resumer).await;
}

/// A react run whose process died after its reply was recorded and before
/// its `done`: recovery passes `""`, which react reads as no answer, and the
/// recorded reply ends the run without a model call. Nothing is suspended,
/// so only the run's `runtime: react` metadata routes it.
#[tokio::test(flavor = "multi_thread")]
async fn react_crash_recovery_passes_no_answer() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let reply = ReactEvent::ReactTurn {
        turn: turn("2024 sold most", vec![]),
    };
    let run_id = seed_react_run(&db, ws, &[reply.to_row()]).await;
    lapse(&db, &run_id).await;

    let host = Host::new(true, vec![]);
    recover_stranded(&db, ws, &host).await;
    let run = until(&db, &run_id, "the recovered run never ended", ended).await;

    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "error: {:?}",
        run.error_message
    );
    assert_eq!(
        host.engines_built.load(Ordering::SeqCst),
        1,
        "routed to react"
    );
    assert!(host.seen().is_empty(), "no model call: {:?}", host.seen());
    let rows = rows(&db, &run_id).await;
    assert_eq!(count(&rows, "done"), 1, "{rows:?}");
    assert_eq!(count(&rows, "error"), 0, "{rows:?}");
}

/// A suspension from the FSM's `clarifying` state resumes on the FSM, even
/// on a run whose metadata says react: the suspension's `from_state` wins.
/// The host has no warehouse, so the FSM's build refuses before any model.
#[tokio::test(flavor = "multi_thread")]
async fn fsm_resume_is_unchanged() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = format!("fsm-resume-{}", Uuid::new_v4());
    let metadata = json!({ "agent_id": AGENT, "runtime": "react" });
    crud::insert_run(
        &db,
        &run_id,
        QUESTION,
        None,
        "analytics",
        Some(metadata),
        ws,
    )
    .await
    .unwrap();
    let clarifying = SuspendedRunData {
        from_state: "clarifying".into(),
        original_input: QUESTION.into(),
        trace_id: run_id.clone(),
        stage_data: json!({}),
        question: "Which year?".into(),
        suggestions: vec![],
    };
    crud::upsert_suspension(&db, &run_id, "Which year?", &[], &clarifying)
        .await
        .unwrap();
    crud::update_task_status(&db, &run_id, "awaiting_input", None)
        .await
        .unwrap();
    assert_eq!(
        queue_answer(&db, &run_id, "2024").await.unwrap(),
        QueuedAnswer::Queued
    );

    let host = Arc::new(Host {
        warehouse: false,
        ..Host::build(true, vec![])
    });
    latency_pass(&db, ws, &host).await;
    let run = until(&db, &run_id, "the fsm resume never ended", ended).await;

    assert!(
        host.fsm_params.load(Ordering::SeqCst) > 0,
        "the fsm's resume_pipeline params were built"
    );
    assert_eq!(
        host.engines_built.load(Ordering::SeqCst),
        0,
        "no react build"
    );
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    let message = run.error_message.unwrap_or_default();
    assert!(message.contains("no databases configured"), "{message}");
    let rows = rows(&db, &run_id).await;
    assert!(
        !rows.iter().any(|(t, _)| t.starts_with("react_")),
        "{rows:?}"
    );
}
