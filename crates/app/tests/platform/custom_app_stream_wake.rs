//! The custom-app agent-run stream (`agent_run_stream::run_events`) wakes on
//! the run-events NOTIFY, not a timer: an event or a row-only terminal write
//! reaches the client on its ring, well inside the 10 s safety net, and a
//! ring that brings nothing terminal leaves the stream open.
//!
//! The ring is struck by hand here (`wake_run`), standing in for the router's
//! LISTEN connection, which a test database does not run.

use std::sync::Arc;
use std::time::Duration;

use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::lifecycle::remote_wake::{RemoteWakes, RunEventWaker};
use futures::{Stream, StreamExt};
use oxy_app::server::api::projects::agent_run_stream::{StreamItem, run_events};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

/// Far inside the stream's 10 s safety net: an item that arrives within this
/// was carried by the ring.
const RING: Duration = Duration::from_secs(2);
/// Long enough to say "nothing came".
const QUIET: Duration = Duration::from_millis(500);

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
    shutdown: CancellationToken,
    run: String,
}

impl World {
    async fn new(status: &str) -> Self {
        let (db, _) = fresh_db(Schema::All).await;
        let run = Uuid::new_v4().to_string();
        exec(
            &db,
            "INSERT INTO agentic_runs (id, question, task_status, workspace_id, created_at, updated_at) \
             VALUES ($1, 'q', $2, $3, now(), now())",
            vec![run.clone().into(), status.into(), Uuid::new_v4().into()],
        )
        .await;
        Self {
            db,
            wakes: Arc::new(RemoteWakes::new()),
            shutdown: CancellationToken::new(),
            run,
        }
    }

    fn stream(&self, last_seq: i64) -> impl Stream<Item = StreamItem> + Unpin {
        Box::pin(run_events(
            self.db.clone(),
            &self.wakes,
            Arc::new(EventRegistry::new()),
            self.run.clone(),
            "analytics".into(),
            last_seq,
            self.shutdown.clone(),
        ))
    }

    /// Append an event as a driver would, then ring as the listener would.
    async fn append(&self, seq: i64, event_type: &str, payload: serde_json::Value) {
        agentic_runtime::crud::insert_event(&self.db, &self.run, seq, event_type, &payload, 0)
            .await
            .expect("insert event");
        self.wakes.wake_run(&self.run);
    }

    async fn set_status(&self, status: &str, error: Option<&str>) {
        exec(
            &self.db,
            "UPDATE agentic_runs SET task_status = $2, error_message = $3 WHERE id = $1",
            vec![
                self.run.clone().into(),
                status.into(),
                error.map(str::to_string).into(),
            ],
        )
        .await;
    }
}

/// The next item within `within`, or `None` when nothing came.
async fn next(
    stream: &mut (impl Stream<Item = StreamItem> + Unpin),
    within: Duration,
) -> Option<Option<StreamItem>> {
    tokio::time::timeout(within, stream.next()).await.ok()
}

#[tokio::test]
async fn an_event_reaches_the_client_on_its_ring() {
    let w = World::new("running").await;
    let mut stream = w.stream(-1);
    assert!(
        next(&mut stream, QUIET).await.is_none(),
        "nothing written yet"
    );

    w.append(0, "llm_token", serde_json::json!({ "token": "hel" }))
        .await;
    let item = next(&mut stream, RING)
        .await
        .expect("the ring carried it")
        .expect("open");
    assert_eq!((item.id, item.event.as_str()), (0, "text_delta"));

    w.append(
        1,
        "done",
        serde_json::json!({ "answer": "42", "trace_id": "t" }),
    )
    .await;
    let item = next(&mut stream, RING)
        .await
        .expect("the ring carried it")
        .expect("open");
    assert_eq!((item.id, item.event.as_str()), (1, "done"));
    assert_eq!(
        next(&mut stream, RING).await,
        Some(None),
        "a terminal event ends the stream"
    );
}

/// A driver that dies, or recovery closing a run, writes the row and no
/// event. The write rings too, and that ring is enough to close the stream.
#[tokio::test]
async fn a_row_only_terminal_write_closes_the_stream_on_its_ring() {
    let w = World::new("running").await;
    let mut stream = w.stream(-1);
    assert!(next(&mut stream, QUIET).await.is_none());

    w.set_status("failed", Some("driver died")).await;
    w.wakes.wake_run(&w.run);
    let item = next(&mut stream, RING)
        .await
        .expect("the ring closed it")
        .expect("open");
    assert_eq!(item.event, "error");
    assert!(item.data.contains("driver died"), "{}", item.data);
    assert_eq!(next(&mut stream, RING).await, Some(None));
}

/// A run parked on a question is not over: a ring that finds it there leaves
/// the stream open, where closing would hand the client an end the run never
/// reached.
#[tokio::test]
async fn a_ring_on_a_parked_run_keeps_the_stream_open() {
    let w = World::new("running").await;
    let mut stream = w.stream(-1);
    w.set_status("awaiting_input", None).await;
    w.wakes.wake_run(&w.run);
    assert!(
        next(&mut stream, QUIET).await.is_none(),
        "still open, nothing sent"
    );
}

#[tokio::test]
async fn a_reconnect_resumes_after_its_last_event_id() {
    let w = World::new("running").await;
    w.append(0, "llm_token", serde_json::json!({ "token": "a" }))
        .await;
    w.append(1, "llm_token", serde_json::json!({ "token": "b" }))
        .await;
    let mut stream = w.stream(0);
    let item = next(&mut stream, RING)
        .await
        .expect("replayed")
        .expect("open");
    assert_eq!(item.id, 1, "seq 0 was already delivered");
}

/// Shutdown ends the stream without inventing a terminal event: the client
/// reconnects, with its `Last-Event-ID`, to a pod that is staying up.
#[tokio::test]
async fn shutdown_ends_the_stream_without_a_terminal_event() {
    let w = World::new("running").await;
    let mut stream = w.stream(-1);
    assert!(next(&mut stream, QUIET).await.is_none());
    w.shutdown.cancel();
    assert_eq!(next(&mut stream, RING).await, Some(None));
}
