//! The ops dashboard's Postgres-only routes, answered by a replica that owns
//! no working copy and drives no run — the Factory (`OXY_ROLE=ide`) being down
//! must not take the view of the queue with it.
//!
//! `agentic_http::router_roles` declares `GET /coordinator/{runs,recovery,queue}`
//! and `PATCH /runs/{id}/thinking_mode` `FleetOk`. That is a claim about the
//! handlers, and this is where it is tested rather than reasoned: the real
//! router over a fresh `AgenticState` (so empty in-process maps), a platform
//! context whose workspace has no path, and the process flagged as owning no
//! workspace files so `workspace_fs_probe` counts any reach for one.
//!
//! The last case is the other direction — why `active-runs` and `tree` are
//! declared `IdeOnly` by name although they read the same tables.
//!
//! Database-backed through [`crate::common::fresh_db`] with every migrator
//! `oxy serve` runs.

use std::collections::BTreeMap;
use std::sync::Arc;

use agentic_core::delegation::TaskSpec;
use agentic_http::AgenticState;
use agentic_pipeline::platform::PlatformContext;
use agentic_runtime::crud::{TaskScope, enqueue_task, update_run_done, update_run_failed};
use agentic_runtime::lifecycle::crud::runs::insert_run;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use entity::users::UserStatus;
use oxy::workspace_fs_probe::{leaks, reset_leaks, set_process_owns_workspace_files};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use super::run_routes_workspace_scope::{NoThreads, Platform};

/// A process that owns no workspace files, as `OXY_ROLE=serve` configures it.
/// `fresh_db` asserts nextest's process-per-test isolation, so the flag is this
/// test's alone; it is restored anyway.
struct DisklessReplica;

impl DisklessReplica {
    fn enter() -> Self {
        set_process_owns_workspace_files(false);
        reset_leaks();
        DisklessReplica
    }
}

impl Drop for DisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        reset_leaks();
    }
}

/// One replica's analytics surface: its own `AgenticState` — so its own,
/// empty, in-process run maps — over the shared database.
fn replica(db: &DatabaseConnection, workspace_id: Uuid) -> (Router, Arc<AgenticState>) {
    let state = Arc::new(AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(NoThreads),
    ));
    let platform: Arc<dyn PlatformContext> = Arc::new(Platform { workspace_id });
    let user = AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("operator@acme.test".into()),
        name: "Operator".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    };
    let router = Router::new()
        .nest(
            "/{workspace_id}/analytics",
            agentic_http::router(state.clone()),
        )
        .layer(Extension(platform))
        .layer(Extension(user));
    (router, state)
}

async fn call(router: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, json)
}

async fn get(router: &Router, path: &str) -> Value {
    let (status, body) = call(router, "GET", path, json!({})).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    body
}

async fn seed(db: &DatabaseConnection, ws: Uuid, id: &str, source_type: &str) {
    insert_run(db, id, "a run", None, source_type, None, ws)
        .await
        .expect("insert run");
}

/// `run_id -> status` out of a `{ runs: [...] }` or `{ nodes: [...] }` body.
fn statuses(body: &Value, list: &str) -> BTreeMap<String, String> {
    body[list]
        .as_array()
        .unwrap_or_else(|| panic!("no `{list}` array in {body}"))
        .iter()
        .map(|r| {
            (
                r["run_id"].as_str().expect("run_id").to_string(),
                r["status"].as_str().expect("status").to_string(),
            )
        })
        .collect()
}

/// Nothing registered, nothing to notify, answer or cancel: this process
/// drives no run and was told about none.
fn holds_no_run_state(state: &AgenticState) -> bool {
    state.statuses.is_empty()
        && state.notifiers.is_empty()
        && state.answer_txs.is_empty()
        && state.cancel_txs.is_empty()
}

#[tokio::test]
async fn the_dashboard_reads_answer_from_postgres_on_a_replica_that_drives_nothing() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let _diskless = DisklessReplica::enter();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());

    seed(&db, a, "wf-done", "workflow").await;
    update_run_done(&db, "wf-done", "ok", None).await.unwrap();
    seed(&db, a, "air-failed", "airway").await;
    update_run_failed(&db, "air-failed", "source refused")
        .await
        .unwrap();
    seed(&db, a, "wf-running", "workflow").await;
    seed(&db, b, "other-workspace", "workflow").await;
    let spec = TaskSpec::Custom {
        kind: "noop".into(),
        payload: json!({}),
    };
    for id in ["wf-running", "other-workspace"] {
        enqueue_task(&db, id, id, None, &spec, None, TaskScope::Global)
            .await
            .expect("enqueue");
    }

    let (router, state) = replica(&db, a);

    let runs = get(&router, &format!("/{a}/analytics/coordinator/runs")).await;
    let expected: BTreeMap<String, String> = [
        ("air-failed", "failed"),
        ("wf-done", "done"),
        ("wf-running", "running"),
    ]
    .into_iter()
    .map(|(id, status)| (id.to_string(), status.to_string()))
    .collect();
    assert_eq!(
        statuses(&runs, "runs"),
        expected,
        "run history is this workspace's rows, with the status Postgres holds"
    );
    assert_eq!(runs["total"], 3);

    let recovery = get(&router, &format!("/{a}/analytics/coordinator/recovery")).await;
    assert_eq!(recovery["total_runs"], 3, "{recovery}");
    assert_eq!(recovery["succeeded_count"], 1, "{recovery}");
    assert_eq!(recovery["failed_count"], 1, "{recovery}");

    let queue = get(&router, &format!("/{a}/analytics/coordinator/queue")).await;
    assert_eq!(
        queue["queued"], 1,
        "one task queued in this workspace: {queue}"
    );

    assert!(
        holds_no_run_state(&state),
        "the reads answered without this process holding any run"
    );
    assert_eq!(
        leaks(),
        0,
        "a coordinator read resolved a workspace path on a process that owns no \
         working copy — it is declared FleetOk, so it must read Postgres only"
    );
}

/// The save is one row, and the process that took it keeps nothing. The row
/// is the one the thread reads load (`get_analytics_extensions`), and those
/// are `FleetOk` already.
#[tokio::test]
async fn a_thinking_mode_save_writes_one_row_and_registers_nothing() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let _diskless = DisklessReplica::enter();
    let ws = Uuid::new_v4();
    agentic_pipeline::insert_run(&db, "chat-1", "sales", "revenue?", None, None, ws)
        .await
        .expect("insert analytics run");

    let (accepting, accepting_state) = replica(&db, ws);
    let (status, body) = call(
        &accepting,
        "PATCH",
        &format!("/{ws}/analytics/runs/chat-1/thinking_mode"),
        json!({ "thinking_mode": "extended_thinking" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        holds_no_run_state(&accepting_state),
        "the save must not register anything in the process that took it — \
         that is what lets any replica take it"
    );

    let saved = agentic_pipeline::get_analytics_extension(&db, "chat-1")
        .await
        .expect("read the extension row")
        .expect("the run has one");
    assert_eq!(saved.thinking_mode.as_deref(), Some("extended_thinking"));
    assert_eq!(leaks(), 0, "the save may not reach for a working copy");
}

/// Why `GET /coordinator/runs/{id}/tree` and `/coordinator/active-runs` stay
/// `IdeOnly`: they overlay each row with `RuntimeState::statuses` and the map
/// wins. A replica that accepts an airway submit `register`s the run
/// (`routes::airway::start_and_drive`) and is never told how it ended — a
/// worker in another process drives it — so that replica reports `running`
/// for good while every other one reports what Postgres holds.
///
/// When this fails because both replicas say `done`, the overlay is gone:
/// declare the two routes `FleetOk` in `agentic_http::router_roles` and delete
/// this case.
#[tokio::test]
async fn two_replicas_disagree_about_a_run_only_one_of_them_accepted() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let ws = Uuid::new_v4();
    seed(&db, ws, "air-1", "airway").await;

    // The replica that took the submit: what `start_and_drive` does after it.
    let (accepted, accepted_state) = replica(&db, ws);
    let (answer_tx, _answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, _cancel_rx) = watch::channel(false);
    accepted_state.register("air-1", answer_tx, cancel_tx);
    // The worker fleet finishes the run; only Postgres hears of it.
    update_run_done(&db, "air-1", "loaded", None).await.unwrap();

    let (elsewhere, _) = replica(&db, ws);
    let path = format!("/{ws}/analytics/coordinator/runs/air-1/tree");
    let from_postgres = statuses(&get(&elsewhere, &path).await, "nodes");
    let from_memory = statuses(&get(&accepted, &path).await, "nodes");

    assert_eq!(from_postgres["air-1"], "done");
    assert_eq!(
        from_memory["air-1"], "running",
        "the replica that accepted the submit now answers from Postgres too — \
         see this case's doc comment for what to flip"
    );
}
