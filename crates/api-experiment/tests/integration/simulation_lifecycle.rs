//! The run row's status transitions, against a real database.
//!
//! `store/tests.rs` pins the *shape* of the claim statement — that its `WHERE`
//! carries a status precondition at all. What it cannot show is the thing the
//! precondition exists for, because that property is a **sequence**: a run that
//! reached a terminal status, followed by a late claim from a second attempt of
//! the same task, must still be terminal afterwards. That needs two writes and
//! a row to survive between them.
//!
//! Two attempts of one payload is routine, not exotic. The durable queue's
//! reaper requeues a task whose lease expired, and the transport drops an
//! outcome whose claim a peer has taken over (`TerminalWrite::NotOwned` — "this
//! process just finished work the queue is discarding"): in that second case the
//! run row is already `done` while the queue row is still claimed, so the reaper
//! hands the payload out again and the second attempt claims a finished run.
//!
//! What a resurrected run costs, concretely: `api::simulation::runs::limits`
//! counts `queued`/`running` against `MAX_IN_FLIGHT_PER_WORKSPACE`, so the row
//! holds a workspace cap slot until `IN_FLIGHT_MAX_AGE_HOURS` ages it out — and
//! a reader scoped to terminal runs stops seeing evidence already written.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use entity::simulation_runs;
use oxy_api_experiment::simulation::store::{self, Claim, NewRun};
use oxy_api_experiment::simulation::{
    SIMULATION_RUN_KIND, SimulationRunPayload, SimulationTaskExecutor,
};
use oxy_simulation::{PolicyKind, ResponseCurve, SimulationSpec};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// A world small enough to parse; nothing here runs it.
fn spec() -> SimulationSpec {
    SimulationSpec::from_yaml(
        r#"
name: lifecycle
seed: 7
periods: 3
period_days: 7
history_days: 120
start_date: 2025-01-06
entities:
  count: 4
  scale_sigma: 0.4
baseline:
  sales_per_entity_day: 1500.0
  margin: 0.36
  demand_shock_rho: 0.0
  demand_shock_sd: 0.01
  weekly_seasonality: 0.15
mechanism:
  driver: marketing_spend
  target: net_sales
  lag_days: 7
  noise_ratio: 0.05
  calibrate:
    anchor_spend_share: 0.02
    local_slope_at_anchor: 4.0
    optimum_at: 3.0
"#,
    )
    .expect("spec")
}

fn truth() -> ResponseCurve {
    ResponseCurve {
        theta: 0.668,
        scale: 12.5,
        anchor_spend: 30.0,
        optimum_spend: 90.0,
    }
}

/// Enqueue one run, the way the HTTP handler does.
async fn queue(db: &DatabaseConnection) -> Uuid {
    let run_id = Uuid::new_v4();
    store::queue_run(
        db,
        NewRun {
            run_id,
            // No foreign key on this column, so the run needs no seeded
            // workspace — and giving each run its own keeps two cases in one
            // database from reading each other's rows.
            workspace_id: Uuid::new_v4(),
            revision_id: None,
            policy: PolicyKind::Hold,
            replicate: 0,
        },
        &spec(),
    )
    .await
    .expect("queue run");
    run_id
}

async fn status_of(db: &DatabaseConnection, run_id: Uuid) -> simulation_runs::Model {
    simulation_runs::Entity::find_by_id(run_id)
        .one(db)
        .await
        .expect("read run")
        .expect("run row exists")
}

/// The blocker this file exists for. A completed run, then a second attempt's
/// claim: the run must still be `done`, and the claim must say so rather than
/// silently reporting success.
#[tokio::test]
async fn a_late_claim_leaves_a_finished_run_finished() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    let run_id = queue(&db).await;

    assert_eq!(
        store::mark_running(&db, run_id).await.expect("first claim"),
        Claim::Claimed,
        "the ordinary claim has to move a queued run"
    );
    assert_eq!(status_of(&db, run_id).await.status, "running");

    store::finish_run(&db, run_id, truth())
        .await
        .expect("finish");

    // The late writer: a requeued attempt of the same payload, arriving after
    // the terminal write.
    let late = store::mark_running(&db, run_id).await.expect("late claim");

    // The row first, and the answer second, deliberately. What the guard is for
    // is the state of the run — a `Claim` that says the wrong thing is only how
    // the executor learns of it. Asserting the return value first would stop the
    // test one line before the damage, and report a mismatched enum rather than
    // a finished run that is running again.
    let run = status_of(&db, run_id).await;
    assert_eq!(
        run.status, "done",
        "a terminal run was driven back to `{}` by a late claim",
        run.status
    );
    assert_eq!(
        late,
        Claim::AlreadyClosed,
        "a claim on a terminal run must report that it moved nothing"
    );
    assert!(
        run.truth.is_some(),
        "the resurrected run also has to keep the truth it recorded"
    );
    assert!(
        run.finished_at.is_some(),
        "a run that is still `done` must still have a finish time"
    );
}

/// The other two terminal statuses, for the same reason and by the same
/// statement. `cancelled` matters most: it is somebody's decision, and a claim
/// that undid it would restart work an operator stopped.
#[tokio::test]
async fn a_late_claim_leaves_failed_and_cancelled_runs_alone() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;

    let failed = queue(&db).await;
    store::mark_running(&db, failed).await.expect("claim");
    store::fail_run(&db, failed, "warehouse fell over".to_string())
        .await
        .expect("fail");
    let late = store::mark_running(&db, failed).await.expect("late claim");
    let run = status_of(&db, failed).await;
    assert_eq!(
        run.status, "failed",
        "a failed run was driven back to `{}` by a late claim",
        run.status
    );
    assert_eq!(
        run.error.as_deref(),
        Some("warehouse fell over"),
        "the diagnosis the first attempt wrote has to survive the second"
    );
    assert_eq!(late, Claim::AlreadyClosed);

    let cancelled = queue(&db).await;
    store::mark_running(&db, cancelled).await.expect("claim");
    store::cancel_run(&db, cancelled).await.expect("cancel");
    let late = store::mark_running(&db, cancelled)
        .await
        .expect("late claim");
    let run = status_of(&db, cancelled).await;
    assert_eq!(
        run.status, "cancelled",
        "a late claim restarted work an operator stopped: the run is `{}`",
        run.status
    );
    assert_eq!(late, Claim::AlreadyClosed);
}

/// The claim a lease expiry produces: the first attempt died mid-run, so the
/// row is `running` and nothing terminal was ever written. That one has to be
/// claimable — the guard is on terminal statuses, not on "already started" —
/// and it restamps `started_at`, because the attempt that owns the run's
/// runtime is the one now executing.
#[tokio::test]
async fn a_requeued_run_that_never_finished_is_still_claimable() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    let run_id = queue(&db).await;

    store::mark_running(&db, run_id).await.expect("first claim");
    let first = status_of(&db, run_id).await.started_at;

    assert_eq!(
        store::mark_running(&db, run_id)
            .await
            .expect("requeued claim"),
        Claim::Claimed,
        "a run still in flight must be re-claimable after its lease expires"
    );
    let second = status_of(&db, run_id).await;
    assert_eq!(second.status, "running");
    assert!(
        second.started_at >= first,
        "the requeued attempt owns the runtime, so it restamps started_at"
    );
}

/// A run whose row is gone reports the same "moved nothing" as a terminal one,
/// rather than an error. The distinction the executor needs is "may I run?",
/// and the answer is no either way — an error here would have it close out a
/// run that does not exist.
#[tokio::test]
async fn a_claim_on_a_missing_run_moves_nothing() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    assert_eq!(
        store::mark_running(&db, Uuid::new_v4())
            .await
            .expect("a missing run is not a database error"),
        Claim::AlreadyClosed
    );
}

/// Make the claim write — and only the claim write — fail against a live
/// database.
///
/// A `CHECK` rather than a broken pool, because a pool that is down fails the
/// *close-out* too, and then there is nothing to observe: the question here is
/// what the executor does with a claim error while it can still write. This
/// constraint rejects exactly the statement under test (`status = 'running'`)
/// and leaves `fail_run` working, which is the shape of every real cause —
/// a serialization failure, a statement timeout, a pool checkout that timed out
/// on that one call.
async fn reject_claim_writes(db: &DatabaseConnection) {
    db.execute_unprepared(
        "ALTER TABLE simulation_runs \
         ADD CONSTRAINT test_claim_writes_fail CHECK (status <> 'running')",
    )
    .await
    .expect("add constraint");
}

/// Drive the real executor over one queued run, and wait for its outcome.
async fn run_executor(db: &DatabaseConnection, run_id: Uuid) -> TaskOutcome {
    let payload = SimulationRunPayload {
        run_id,
        workspace_id: Uuid::new_v4(),
        revision_id: None,
        spec: serde_json::to_value(spec()).expect("spec to json"),
        policy: PolicyKind::Hold,
        replicate: 0,
    };
    let executor = SimulationTaskExecutor { db: db.clone() };
    let mut task = executor
        .execute(TaskAssignment {
            task_id: run_id.to_string(),
            parent_task_id: None,
            run_id: run_id.to_string(),
            spec: TaskSpec::Custom {
                kind: SIMULATION_RUN_KIND.to_string(),
                payload: serde_json::to_value(&payload).expect("payload to json"),
            },
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        })
        .await
        .expect("executor accepted the assignment");

    task.outcomes
        .recv()
        .await
        .expect("the executor has to report an outcome")
}

/// A claim write that **errors** must leave the run in a state a reader can
/// diagnose — not at `queued`.
///
/// `queue_run` writes the row synchronously at enqueue, so the run exists
/// before any worker sees it. Returning `Failed` stamps the *queue* row
/// terminal (`durable`'s outcome arm → `crud::fail_queue_task`), so there is no
/// second attempt coming: an executor that walks away here leaves a `queued`
/// row with nothing alive to move it. It reads as a run still waiting for a
/// worker, it counts against `MAX_IN_FLIGHT_PER_WORKSPACE`, and it is invisible
/// to any reader scoped to terminal runs, until `IN_FLIGHT_MAX_AGE_HOURS` ages
/// it out. That backstop is still there and still needed — a worker can die
/// with nothing left to write at all — but a claim error is not one of the
/// cases that has to rely on it, because the process that saw the error is
/// still alive and still connected.
#[tokio::test]
async fn a_claim_that_errors_closes_the_run_out() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    let run_id = queue(&db).await;
    reject_claim_writes(&db).await;

    let outcome = run_executor(&db, run_id).await;

    let run = status_of(&db, run_id).await;
    assert_ne!(
        run.status, "queued",
        "the claim failed and the executor walked away: the run is stranded at \
         `queued` with nothing alive to move it"
    );
    assert_eq!(run.status, "failed");
    assert!(
        run.error
            .as_deref()
            .is_some_and(|e| e.contains("mark run running")),
        "the run's `error` has to name the claim as what failed, got {:?}",
        run.error
    );
    assert!(
        run.finished_at.is_some(),
        "a closed-out run needs a finish time, or the listing still reads it as open"
    );
    assert!(
        matches!(outcome, TaskOutcome::Failed(_)),
        "the queue still has to hear that this attempt failed, got {outcome:?}"
    );
}
