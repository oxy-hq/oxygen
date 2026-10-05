//! A custom app's procedure run, on the queue:
//! `POST /api/projects/{id}/procedures/{pid}/runs` registers the run and
//! enqueues it, a **separate** driver claims and executes it, and
//! `GET /api/projects/{id}/procedures/runs/{run_id}` reports it — from any
//! replica, whether or not the one that took the request still exists.
//!
//! The replicas, the driver and the identity they run under are
//! `custom_app_procedure_run_fixture`'s; read its header for what is the
//! production path and what is the test's own.
//!
//! **Needs** Postgres only.

use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use oxy_app::server::api::custom_apps_gates::check_custom_app_gates;
use oxy_app::server::api::projects::automation_run::task::PROCEDURE_RUN_KIND;
use sea_orm::ConnectionTrait;
use serde_json::json;

use crate::common::read_repo_file;
use crate::custom_app_procedure_run_attempts::{
    kill_the_driver, mark_begun_by_a_dead_attempt, mark_begun_by_a_live_attempt,
    wait_until_handed_back,
};
use crate::custom_app_procedure_run_fixture::{
    CANCEL_ROUTE, POLL_ROUTE, START_ROUTE, driver_platform, fixture, poll, replica, run_row, send,
    spawn_driver, start, started_events, wait_for_driver, wait_for_run,
};

#[tokio::test]
async fn a_queued_run_is_finished_by_a_driver_that_never_saw_the_request() {
    let f = fixture().await;
    let submitter = replica(&f.t.db);

    let run_id = start(&submitter, f.workspace_id).await;

    // The handler drove nothing: the run is on the queue for whoever claims
    // it, with no driver and no claim.
    assert_eq!(
        poll(&submitter, f.workspace_id, &run_id).await,
        json!({ "status": "running" })
    );
    let queued = agentic_runtime::crud::get_queue_entry(&f.t.db, &run_id)
        .await
        .expect("queue lookup")
        .expect("the start enqueues a task");
    assert_eq!(queued.queue_status, "queued");
    assert!(
        !queued.scope_owned,
        "the task must be Global: a scope-owned row is claimable only by a \
         coordinator in the process that enqueued it, and there is none"
    );
    let seeded = agentic_runtime::crud::get_run(&f.t.db, &run_id)
        .await
        .expect("run lookup")
        .expect("the start registers a run for the driver to lease");
    assert_eq!(seeded.source_type.as_deref(), Some(PROCEDURE_RUN_KIND));
    assert_eq!(seeded.workspace_id, f.workspace_id);
    assert_eq!(seeded.driver_id, None);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        poll(&submitter, f.workspace_id, &run_id).await,
        json!({ "status": "running" }),
        "with no driver the run must stay queued — the replica does not execute it"
    );

    // The replica that took the request goes away. The run must not.
    drop(submitter);

    let (platform, _platform_dir) = driver_platform().await;
    assert_eq!(
        platform.subject(),
        None,
        "the control: a driver's own platform carries no caller"
    );
    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let other_replica = replica(&f.t.db);
    let done = wait_for_run(&other_replica, f.workspace_id, &run_id).await;
    driver.abort();

    assert_eq!(
        done,
        json!({
            "status": "done",
            "result": {
                "summary": "Automation completed — 1 task outputs.",
                "outputs": done["result"]["outputs"].clone(),
            }
        })
    );
    let greeting = done["result"]["outputs"]["greeting"].to_string();
    assert!(
        greeting.contains("hello s-7"),
        "the request's params must reach the run through the queue: {done}"
    );

    // The run acted as the caller the gate authenticated — the identity the
    // handler's own context has — and not as the driver's subject-less
    // platform, which `airhouse_managed` would have minted a system Admin for.
    let gate = check_custom_app_gates(&HeaderMap::new(), f.workspace_id)
        .await
        .unwrap_or_else(|_| panic!("the guest passes the gate"));
    let handler_context = gate
        .build_project_context()
        .await
        .unwrap_or_else(|_| panic!("the handler's context builds"));
    assert_eq!(handler_context.subject(), Some(f.t.guest_id));
    assert_eq!(handler_context.role(), None);

    let started = started_events(&f.t.db, &run_id).await;
    assert_eq!(
        started.len(),
        1,
        "the run executed exactly once: {started:?}"
    );
    assert_eq!(
        started[0]["subject"],
        json!(handler_context.subject()),
        "the driven run's subject"
    );
    assert_eq!(
        started[0]["role"],
        json!(handler_context.role().map(|r| r.as_str())),
        "the driven run's role"
    );

    // The attempt that ran it left the stamp every later attempt reads.
    let row = run_row(&f.t.db, &run_id).await;
    assert!(
        row.execution_started_at.is_some(),
        "the attempt that executed the run must have stamped execution_started_at"
    );
    assert!(
        row.execution_started_at >= Some(row.started_at),
        "execution begins after the run is accepted: {row:?}"
    );
    assert!(
        row.execution_heartbeat_at.is_some()
            && row.execution_heartbeat_at >= row.execution_started_at,
        "the attempt that executed the run must have left its heartbeat: {row:?}"
    );
}

/// At most once. The driver had begun executing the run — the stamp is there —
/// and died mid-run. The inline runner keeps no checkpoint, so running the
/// payload again would repeat every step the dead attempt finished, outside
/// effects included. The next attempt must close the run instead.
#[tokio::test]
async fn a_run_whose_driver_died_mid_run_is_closed_as_interrupted_not_re_run() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;
    mark_begun_by_a_dead_attempt(&f.t.db, &run_id).await;
    let (platform, _platform_dir) = kill_the_driver(&f, &app, &run_id).await;

    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let closed = wait_for_run(&app, f.workspace_id, &run_id).await;
    let settled = wait_for_driver(&f.t.db, &run_id).await;
    driver.abort();

    assert_eq!(
        closed,
        json!({
            "status": "failed",
            "error": {
                "message": "the automation was interrupted while running and was not \
                            retried, to avoid repeating steps that already ran; start it again",
                "code": "automation_run_interrupted",
            }
        }),
        "the bundle must learn the run was interrupted, through the `failed` it already handles"
    );
    assert!(
        started_events(&f.t.db, &run_id).await.is_empty(),
        "the second attempt must not have started the automation"
    );
    let row = run_row(&f.t.db, &run_id).await;
    assert_eq!(
        row.result_outputs, None,
        "nothing ran, so nothing was produced"
    );
    assert_eq!(
        settled, "failed",
        "the driver's own record agrees with the row"
    );
    let run = agentic_runtime::crud::get_run(&f.t.db, &run_id)
        .await
        .expect("run lookup")
        .expect("run row");
    assert!(
        run.error_message
            .as_deref()
            .is_some_and(|m| m.contains("interrupted")),
        "driver-side error: {:?}",
        run.error_message
    );
}

/// The claim was handed on under a driver that is still executing: the reaper
/// requeued it, but the attempt that began the run is beating. The stamp alone
/// reads exactly as it does for a dead attempt; the heartbeat is what tells
/// the next claimant to leave the run alone — no terminal state on the row or
/// on the driver's record, nothing run — and hand the task back to the queue.
#[tokio::test]
async fn a_run_a_live_attempt_is_executing_is_left_alone_by_the_next_claimant() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;
    mark_begun_by_a_live_attempt(&f.t.db, &run_id).await;
    let live = run_row(&f.t.db, &run_id).await;
    let (platform, _platform_dir) = kill_the_driver(&f, &app, &run_id).await;

    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let handed_back = wait_until_handed_back(&f.t.db, &run_id).await;
    driver.abort();

    assert_eq!(handed_back, "queued", "deferred, not dead-lettered");
    assert_eq!(
        run_row(&f.t.db, &run_id).await,
        live,
        "the claimant must write nothing to a run another attempt is executing"
    );
    assert_eq!(
        poll(&app, f.workspace_id, &run_id).await,
        json!({ "status": "running" }),
        "the bundle keeps seeing the live attempt's run as running"
    );
    let run = agentic_runtime::crud::get_run(&f.t.db, &run_id)
        .await
        .expect("run lookup")
        .expect("run row");
    assert!(
        !matches!(
            run.task_status.as_deref(),
            Some("done" | "failed" | "cancelled" | "timed_out")
        ),
        "stepping aside must not end the driver's record of the run: {:?}",
        run.task_status
    );
    assert!(
        started_events(&f.t.db, &run_id).await.is_empty(),
        "the claimant must not have started the automation"
    );
}

/// The inverse: the driver died holding the claim but before it began — the
/// stamp is absent, so no step can have run — and the next attempt runs it.
#[tokio::test]
async fn a_run_whose_driver_died_before_it_began_is_finished_by_another() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;
    assert_eq!(
        run_row(&f.t.db, &run_id).await.execution_started_at,
        None,
        "the start must not stamp: it queues, it does not execute"
    );
    let (platform, _platform_dir) = kill_the_driver(&f, &app, &run_id).await;

    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let done = wait_for_run(&app, f.workspace_id, &run_id).await;
    driver.abort();

    assert_eq!(done["status"], "done", "run: {done}");
    assert!(
        done["result"]["outputs"]["greeting"]
            .to_string()
            .contains("hello s-7"),
        "run: {done}"
    );
    assert_eq!(
        started_events(&f.t.db, &run_id).await.len(),
        1,
        "the run executed exactly once"
    );
    assert!(
        run_row(&f.t.db, &run_id)
            .await
            .execution_started_at
            .is_some()
    );
}

#[tokio::test]
async fn a_run_cancelled_before_a_driver_claims_it_never_executes() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;

    // Cancel from a replica that did not take the start, with nothing driving.
    let canceller = replica(&f.t.db);
    let (status, _) = send(
        &canceller,
        Request::post(format!(
            "/projects/{}/procedures/runs/{run_id}/cancel",
            f.workspace_id
        ))
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        poll(&app, f.workspace_id, &run_id).await,
        json!({ "status": "cancelled" }),
        "a cancel is visible at once, from any replica, with no driver alive"
    );

    // A driver that claims the task afterwards must not run it.
    let (platform, _platform_dir) = driver_platform().await;
    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let settled = wait_for_driver(&f.t.db, &run_id).await;
    driver.abort();

    assert_eq!(settled, "cancelled");
    assert!(
        started_events(&f.t.db, &run_id).await.is_empty(),
        "the automation must not have started"
    );
    assert_eq!(
        poll(&app, f.workspace_id, &run_id).await,
        json!({ "status": "cancelled" })
    );
}

/// What a replica still on the previous release writes for a cancel: the
/// stamp, and nothing else. A driver on this release must honour it.
#[tokio::test]
async fn a_cancel_stamp_alone_stops_the_run_at_claim() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;
    f.t.db
        .execute_unprepared(&format!(
            "UPDATE customer_app_procedure_runs SET cancel_requested_at = now() \
             WHERE id = '{run_id}'"
        ))
        .await
        .expect("stamp the cancel");

    let (platform, _platform_dir) = driver_platform().await;
    let driver = spawn_driver(f.t.db.clone(), platform, f.workspace_id);
    let closed = wait_for_run(&app, f.workspace_id, &run_id).await;
    driver.abort();

    assert_eq!(closed, json!({ "status": "cancelled" }));
    assert!(started_events(&f.t.db, &run_id).await.is_empty());
}

/// A run its driver gave up on — the recovery budget ran out, the task was
/// dead-lettered, or a pod that predates this kind refused it — gets no
/// terminal write from that driver. The poll closes it.
#[tokio::test]
async fn a_run_its_driver_abandoned_is_closed_by_the_poll() {
    let f = fixture().await;
    let app = replica(&f.t.db);
    let run_id = start(&app, f.workspace_id).await;

    agentic_runtime::crud::retire_run(&f.t.db, &run_id, "exceeded 4 recovery attempts")
        .await
        .expect("retire, as the recovery loop does");

    assert_eq!(
        poll(&app, f.workspace_id, &run_id).await,
        json!({
            "status": "failed",
            "error": {
                "message": "automation was interrupted and could not be resumed",
                "code": "automation_run_orphaned",
            }
        })
    );
}

/// The replicas here mount the handlers by hand; production must mount the
/// same three at the same paths.
#[test]
fn production_mounts_the_three_routes_here() {
    let public = read_repo_file("crates/app/src/server/router/public.rs");
    let compact: String = public.split_whitespace().collect();
    for (path, method, handler) in [
        (START_ROUTE, "post", "start_automation_run"),
        (POLL_ROUTE, "get", "poll_automation_run"),
        (CANCEL_ROUTE, "post", "cancel_automation_run"),
    ] {
        let mount =
            format!(".route_fleet(\"{path}\",{method}(projects::automation_run::{handler}),)");
        assert!(
            compact.contains(&mount),
            "public.rs no longer mounts {handler} at {path}"
        );
    }
}
