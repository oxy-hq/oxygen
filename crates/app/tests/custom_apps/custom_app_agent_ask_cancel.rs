//! `POST /api/projects/{id}/agents/asks/{run_id}/cancel`, from a replica that
//! is not driving the run — which is most of them.
//!
//! The cancel always records the durable flag, and writes the run row itself
//! only when the run has not ended and nothing can be driving it: no live
//! driver lease, no queue entry a driver holds or is about to take. It used to
//! fail the row whenever this process held no cancel channel — over a run
//! that had already finished, and under a driver in another process.
//!
//! The replica is `router/public.rs`'s mount behind the real gate chain; the
//! caller is the guest, an Owner of the project's org.
//!
//! **Needs** Postgres only.

use std::sync::Arc;

use agentic_core::delegation::TaskSpec;
use agentic_http::AgenticState;
use agentic_runtime::crud::TaskScope;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use oxy_app::agentic_wiring::thread_owner::OxyThreadOwnerLookup;
use oxy_app::server::api::projects::agent_ask;
use oxy_app::server::router::bare_app_state;
use sea_orm::ConnectionTrait;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};

const CANCEL_ROUTE: &str = "/projects/{project_id}/agents/asks/{run_id}/cancel";

/// One serve replica and the agentic state it holds.
fn replica(t: &Tenant) -> (Router, Arc<AgenticState>) {
    let agentic = Arc::new(AgenticState::new(
        CancellationToken::new(),
        t.db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(t.db.clone())),
    ));
    let mut state = bare_app_state();
    state.agentic_state = Some(agentic.clone());
    let app = Router::new()
        .route(CANCEL_ROUTE, post(agent_ask::cancel_ask))
        .with_state(state);
    (app, agentic)
}

/// Cancel `run_id` from a replica that has never heard of it.
async fn cancel_from_another_replica(t: &Tenant, run_id: &str) {
    let (app, _state) = replica(t);
    cancel_on(&app, run_id).await;
}

async fn cancel_on(app: &Router, run_id: &str) {
    let path = format!(
        "/projects/{}/agents/asks/{run_id}/cancel",
        demo_workspace_id()
    );
    let response = app
        .clone()
        .oneshot(Request::post(path).body(Body::empty()).unwrap())
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// An analytics run of the project, `running`, as an ask's start leaves it.
async fn running_ask(t: &Tenant) -> String {
    let id = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(
        &t.db,
        &id,
        "how many orders?",
        None,
        "analytics",
        Some(json!({ "agent_id": "ask" })),
        demo_workspace_id(),
    )
    .await
    .expect("insert_run");
    id
}

async fn row(t: &Tenant, run_id: &str) -> agentic_runtime::entity::run::Model {
    agentic_runtime::crud::get_run(&t.db, run_id)
        .await
        .expect("get_run")
        .expect("the run exists")
}

async fn cancel_requested(t: &Tenant, run_id: &str) -> bool {
    agentic_runtime::crud::is_cancel_requested(&t.db, run_id)
        .await
        .expect("is_cancel_requested")
}

/// A run that already ended stays as it ended. The unguarded write turned a
/// `done` run into a failed one when a cancel landed a moment late.
#[tokio::test]
async fn a_cancel_never_rewrites_a_run_that_already_ended() {
    let t = seeded_tenant().await;

    let done = running_ask(&t).await;
    agentic_runtime::crud::update_run_done(&t.db, &done, "42 orders", None)
        .await
        .expect("finish");
    let failed = running_ask(&t).await;
    agentic_runtime::crud::update_run_failed(&t.db, &failed, "warehouse unreachable")
        .await
        .expect("fail");
    let cancelled = running_ask(&t).await;
    agentic_runtime::crud::transition_run(&t.db, &cancelled, "cancelled", None, None, None)
        .await
        .expect("cancel");

    for run_id in [&done, &failed, &cancelled] {
        let before = row(&t, run_id).await;
        cancel_from_another_replica(&t, run_id).await;
        let after = row(&t, run_id).await;
        assert_eq!(after.task_status, before.task_status, "status");
        assert_eq!(after.answer, before.answer, "answer");
        assert_eq!(after.error_message, before.error_message, "error");
    }
    assert_eq!(row(&t, &done).await.task_status.as_deref(), Some("done"));
    assert_eq!(row(&t, &done).await.answer.as_deref(), Some("42 orders"));
}

/// A driver in another process holds the run's lease. The cancel reaches it
/// through the flag it polls; this replica writes nothing terminal under it.
#[tokio::test]
async fn a_cancel_leaves_a_leased_run_to_the_driver_that_holds_it() {
    let t = seeded_tenant().await;
    let run_id = running_ask(&t).await;
    assert!(
        agentic_runtime::crud::try_acquire_driver(&t.db, &run_id, "recovery-elsewhere")
            .await
            .expect("lease")
    );
    assert!(!cancel_requested(&t, &run_id).await, "the control");

    cancel_from_another_replica(&t, &run_id).await;

    let after = row(&t, &run_id).await;
    assert_eq!(after.task_status.as_deref(), Some("running"));
    assert_eq!(after.error_message, None);
    assert_eq!(after.driver_id.as_deref(), Some("recovery-elsewhere"));
    assert!(
        cancel_requested(&t, &run_id).await,
        "the flag is what stops a run driven in another process"
    );
}

/// The same for a run on the task queue: `queued` is a driver about to take
/// it, `claimed` one that has.
#[tokio::test]
async fn a_cancel_leaves_a_queued_or_claimed_run_to_its_driver() {
    let t = seeded_tenant().await;
    let spec = TaskSpec::Custom {
        kind: "an_ask_on_the_queue".into(),
        payload: json!({}),
    };
    for claim in [false, true] {
        let run_id = running_ask(&t).await;
        agentic_runtime::crud::enqueue_task(
            &t.db,
            &run_id,
            &run_id,
            None,
            &spec,
            None,
            TaskScope::Global,
        )
        .await
        .expect("enqueue");
        if claim {
            agentic_runtime::crud::claim_task_under_root(&t.db, "a-worker", &run_id)
                .await
                .expect("claim")
                .expect("claimable");
        }

        cancel_from_another_replica(&t, &run_id).await;

        let after = row(&t, &run_id).await;
        assert_eq!(
            after.task_status.as_deref(),
            Some("running"),
            "claimed: {claim}"
        );
        assert_eq!(after.error_message, None, "claimed: {claim}");
        assert!(cancel_requested(&t, &run_id).await, "claimed: {claim}");
    }
}

/// Nothing drives the run and nothing will: its driver died, its lease lapsed
/// or was never taken. The cancel closes it, or it reads `running` for ever.
#[tokio::test]
async fn a_cancel_closes_a_run_nothing_is_driving() {
    let t = seeded_tenant().await;

    let undriven = running_ask(&t).await;
    let lapsed = running_ask(&t).await;
    assert!(
        agentic_runtime::crud::try_acquire_driver(&t.db, &lapsed, "recovery-dead")
            .await
            .expect("lease")
    );
    let stale = agentic_runtime::crud::DRIVER_LEASE_TTL_SECS + 30;
    t.db.execute_unprepared(&format!(
        "UPDATE agentic_runs \
         SET driver_heartbeat_at = now() - interval '{stale} seconds' \
         WHERE id = '{lapsed}'"
    ))
    .await
    .expect("age the lease");

    for run_id in [&undriven, &lapsed] {
        cancel_from_another_replica(&t, run_id).await;
        let after = row(&t, run_id).await;
        assert_eq!(after.task_status.as_deref(), Some("failed"));
        assert_eq!(after.error_message.as_deref(), Some("cancelled by user"));
        assert_eq!(after.driver_id, None, "a closed run holds no lease");
        assert!(cancel_requested(&t, run_id).await);
    }
}

/// The driver is in this process: the cancel signals it and leaves the row to
/// it, exactly as before.
#[tokio::test]
async fn a_cancel_signals_a_driver_in_this_process_and_leaves_it_the_row() {
    let t = seeded_tenant().await;
    let run_id = running_ask(&t).await;
    let (app, state) = replica(&t);
    let (answer_tx, _answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.runtime.register(&run_id, answer_tx, cancel_tx);

    cancel_on(&app, &run_id).await;

    assert!(*cancel_rx.borrow(), "the driver's cancel channel fired");
    assert_eq!(
        row(&t, &run_id).await.task_status.as_deref(),
        Some("running"),
        "the driver, not the endpoint, closes a run it is driving"
    );
    assert!(cancel_requested(&t, &run_id).await);
}
