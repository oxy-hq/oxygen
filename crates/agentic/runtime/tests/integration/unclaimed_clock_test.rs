//! `find_pending_global_runs` reports how long each run has gone unclaimed, and
//! `find_stuck_runs` and `find_stuck_automation_runs` read the same clock.
//!
//! That number is what lets a node *prefer* to leave work for the fleet without
//! being able to strand it: `agentic_pipeline::recovery::may_drive` opens for a
//! kind the node would rather not run once the run has waited the grace. So the
//! clock has to be right in both directions — slow to call fresh work
//! "unclaimed" (or the preference is a coin flip), and never stuck at zero (or
//! the fallback never fires).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(unclaimed_clock_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::stuck_run_sweeper_test::{age_run, seed_run, test_db};

/// `agentic_pipeline::recovery::STRANDED_GRACE_SECS`, spelled out: this crate
/// sits below the pipeline and cannot name it. Also the grace every
/// `find_stuck_runs` caller in this suite passes.
const GRACE: u64 = 30;

/// A kind that is neither `workflow` nor `airway` — the two the periodic
/// stranded tick selects. Any `TaskSpec::Custom` kind behaves the same.
const CUSTOM_KIND: &str = "preagg_cycle";

/// A root run of `kind` with one `queued` Global queue row and no driver: the
/// shape a schedule tick or a `run-now` leaves behind.
async fn seed_global(db: &DatabaseConnection, kind: &str) -> String {
    let run_id = seed_run(db, kind).await;
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Custom {
            kind: kind.into(),
            payload: serde_json::json!({}),
        },
        None,
        crud::TaskScope::Global,
    )
    .await
    .unwrap();
    run_id
}

/// Backdate the run's queue rows, the way `age_run` backdates the run row.
async fn age_queue_rows(db: &DatabaseConnection, run_id: &str, secs: i64) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET updated_at = updated_at - ($1 || ' seconds')::interval \
         WHERE task_id = $2 OR task_id LIKE $2 || '.%'",
        [secs.into(), run_id.into()],
    ))
    .await
    .unwrap();
}

/// Leave a driver lease on the run whose last heartbeat was `secs` ago, as a
/// driver that died without releasing would.
async fn leave_dead_lease(db: &DatabaseConnection, run_id: &str, secs: i64) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs \
         SET driver_id = 'a-dead-driver', \
             driver_heartbeat_at = now() - ($1 || ' seconds')::interval \
         WHERE id = $2",
        [secs.into(), run_id.into()],
    ))
    .await
    .unwrap();
}

/// What the latency worker would read for this run, or `None` if its selection
/// does not include the run at all.
async fn unclaimed_secs(db: &DatabaseConnection, run_id: &str) -> Option<u64> {
    crud::find_pending_global_runs(db, Some(uuid::Uuid::nil()))
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.run_id == run_id)
        .map(|r| r.unclaimed_secs)
}

/// What the periodic stranded tick would read for this run, or `None` if
/// `find_stuck_runs` does not select it.
async fn stranded_unclaimed_secs(db: &DatabaseConnection, run_id: &str) -> Option<u64> {
    crud::find_stuck_runs(db, GRACE, Some(uuid::Uuid::nil()))
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.run_id == run_id)
        .map(|r| r.unclaimed_secs)
}

/// What the automation sweeper's selection reports for this run, or `None` if
/// `find_stuck_automation_runs` does not select it.
async fn sweeper_unclaimed_secs(db: &DatabaseConnection, run_id: &str) -> Option<u64> {
    crud::find_stuck_automation_runs(db, GRACE)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.run_id == run_id)
        .map(|r| r.unclaimed_secs)
}

/// Claim the run's root row as `worker`, then let that worker's heartbeat go
/// stale past the visibility timeout — the row a worker killed mid-run leaves
/// behind.
async fn claim_and_die(db: &DatabaseConnection, run_id: &str, worker: &str) {
    crud::claim_task_under_root(db, worker, run_id)
        .await
        .unwrap()
        .expect("claim must succeed");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue \
         SET last_heartbeat = now() - interval '10 minutes' \
         WHERE task_id = $1",
        [run_id.into()],
    ))
    .await
    .unwrap();
}

/// The periodic tick is the path that reaps, and it selects what it just
/// reaped. Its clock therefore has to be the latency worker's clock, or the
/// placement gate answers differently depending on which loop saw the run.
///
/// The scenario is the one the gate exists for: an hour-old airway pipeline
/// whose worker was OOM-killed. The reaper hands its row back (`claimed` →
/// `queued`, `updated_at = now()`), the dead worker's driver lease lapses, and
/// `find_stuck_runs` selects the run — with its `updated_at` an hour past the
/// grace. Read as age, that is "unclaimed for an hour" and a deferring node
/// takes it in the same tick it reaped it. Read as this clock, it is "handed
/// back just now", and the fleet gets its grace first.
#[tokio::test(flavor = "multi_thread")]
async fn the_periodic_tick_reads_the_same_clock_as_the_latency_worker() {
    let Some(db) = test_db().await else {
        return;
    };
    let ttl = crud::DRIVER_LEASE_TTL_SECS;
    let run_id = seed_global(&db, "airway").await;
    age_run(&db, &run_id, 3600).await;
    age_queue_rows(&db, &run_id, 3600).await;

    // A worker took it, drove it (holding the lease), and died.
    claim_and_die(&db, &run_id, "oom-killed-worker").await;
    leave_dead_lease(&db, &run_id, ttl + 2).await;
    assert_eq!(
        stranded_unclaimed_secs(&db, &run_id).await,
        None,
        "a `claimed` row — even a dead worker's — keeps the run out of the tick \
         until the reaper frees it"
    );

    // The reaper frees it, as the tick's own pre-pass does.
    let reaped = crud::reap_stale_tasks(&db).await.unwrap();
    assert!(reaped.requeued >= 1, "the stale claim was not re-queued");

    let tick = stranded_unclaimed_secs(&db, &run_id)
        .await
        .expect("an aged airway run with a queued Global row is stranded");
    let latency = unclaimed_secs(&db, &run_id)
        .await
        .expect("the same run is pending for the latency worker");
    assert!(
        tick < GRACE,
        "re-queued by the reaper a moment ago, yet the tick reads {tick}s \
         unclaimed — it is reading the run's age, and a deferring node would \
         take the pipeline in the tick that reaped it"
    );
    assert!(
        tick.abs_diff(latency) <= 1,
        "the two selections disagree about the same run: tick {tick}s, latency \
         worker {latency}s"
    );

    // Nobody took it for two minutes: now both call it unclaimed.
    age_queue_rows(&db, &run_id, 120).await;
    leave_dead_lease(&db, &run_id, ttl + 120).await;
    let tick = stranded_unclaimed_secs(&db, &run_id)
        .await
        .expect("still stranded");
    let latency = unclaimed_secs(&db, &run_id).await.expect("still pending");
    assert!(
        tick >= GRACE,
        "handed back two minutes ago, yet only {tick}s unclaimed"
    );
    assert!(
        tick.abs_diff(latency) <= 1,
        "tick {tick}s vs latency worker {latency}s"
    );
}

/// A stranded run with no queue row at all — `needs_resume` after a crash,
/// nothing re-queued — is invisible to the latency worker and reports its wait
/// to the tick from its own `updated_at`. If this read zero, a deferring node's
/// fallback would never open for it and the run would wait for a worker that
/// may not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranded_run_with_no_queue_row_still_counts_its_wait() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_run(&db, "airway").await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = 'needs_resume' WHERE id = $1",
        [run_id.as_str().into()],
    ))
    .await
    .unwrap();
    age_run(&db, &run_id, 120).await;

    assert_eq!(
        unclaimed_secs(&db, &run_id).await,
        None,
        "with no queued row the latency worker does not select the run"
    );
    let waited = stranded_unclaimed_secs(&db, &run_id)
        .await
        .expect("an aged needs_resume run with no queue row is stranded");
    assert!(
        waited >= GRACE,
        "stranded for two minutes with nothing queued, yet only {waited}s unclaimed"
    );
}

/// The automation sweeper's selection reads the same clock, not a placeholder.
///
/// It used to report `0` — "just arrived" to the placement gate — because the
/// sweeper re-enqueues rather than drives and reads no policy. Nothing broke
/// while that held, but a later caller handing those rows to
/// `partition_drivable` under `Defer` / `Only` would have declined a
/// long-stranded run on every tick, forever. With no queued row by
/// construction, it has to read what the periodic tick reads for the same
/// stranded run — including the lease term, so a driver that died a moment ago
/// is not an old wait.
#[tokio::test(flavor = "multi_thread")]
async fn the_automation_sweeper_reads_the_same_clock_as_the_periodic_tick() {
    let Some(db) = test_db().await else {
        return;
    };
    let ttl = crud::DRIVER_LEASE_TTL_SECS;
    let run_id = seed_run(&db, "workflow").await;
    age_run(&db, &run_id, 120).await;

    let swept = sweeper_unclaimed_secs(&db, &run_id)
        .await
        .expect("an aged workflow run with no queue row is the sweeper's");
    let tick = stranded_unclaimed_secs(&db, &run_id)
        .await
        .expect("the same run is stranded for the periodic tick");
    assert!(
        swept >= GRACE,
        "stranded for two minutes with nothing queued, yet the sweeper's \
         selection reads {swept}s unclaimed"
    );
    assert!(
        swept.abs_diff(tick) <= 1,
        "the two selections disagree about the same run: sweeper {swept}s, \
         tick {tick}s"
    );

    // Its driver died, and the lease lapsed five seconds ago.
    leave_dead_lease(&db, &run_id, ttl + 5).await;
    let swept = sweeper_unclaimed_secs(&db, &run_id)
        .await
        .expect("a lapsed lease does not hide the run from the sweeper");
    let tick = stranded_unclaimed_secs(&db, &run_id)
        .await
        .expect("a lapsed lease leaves the run stranded");
    assert!(
        swept < GRACE,
        "lease lapsed ~5s ago but the sweeper's selection reads {swept}s \
         unclaimed — the lease term is missing from its clock"
    );
    assert!(
        swept.abs_diff(tick) <= 1,
        "sweeper {swept}s vs tick {tick}s"
    );
}

/// The airway fallback, generalised to a kind the periodic tick does not cover
/// — and the reason the fallback for those kinds cannot be the periodic tick.
///
/// `an_unclaimed_global_airway_run_falls_back_to_the_periodic_tick` shows an
/// aged, unclaimed airway run is picked up by `find_stuck_runs`. That query is
/// scoped to `workflow` + `airway`, so the same run under any other kind is
/// invisible to it at any age: a node that declines such a run at the latency
/// worker and relies on the periodic tick to catch it has left it queued
/// forever. What does still see it is the latency worker's own selection, which
/// now says how long it has waited — the fact the deferring node falls back on.
#[tokio::test(flavor = "multi_thread")]
async fn an_unclaimed_custom_run_is_invisible_to_the_periodic_tick() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_global(&db, CUSTOM_KIND).await;
    // Nobody claimed it, and it is now well past the grace on every clock.
    age_run(&db, &run_id, 120).await;
    age_queue_rows(&db, &run_id, 120).await;

    let stuck = crud::find_stuck_runs(&db, GRACE, None).await.unwrap();
    assert!(
        !stuck.iter().any(|r| r.run_id == run_id),
        "find_stuck_runs selects only workflow + airway; if it has started \
         returning `{CUSTOM_KIND}` its source filter was widened, which also \
         re-drives runs that have no queue row at all — read its doc comment \
         before accepting that"
    );

    let waited = unclaimed_secs(&db, &run_id)
        .await
        .expect("an unclaimed Global run of any kind stays selectable by the latency worker");
    assert!(
        waited >= GRACE,
        "aged {CUSTOM_KIND} run reports {waited}s unclaimed; a node deferring \
         it would never see the grace elapse and never fall back to driving it"
    );

    // A claim closes it: once a worker owns the row the run is not pending for
    // anyone, so the fallback cannot poach work that is being done.
    crud::claim_task_under_root(&db, "a-worker", &run_id)
        .await
        .unwrap()
        .expect("claim must succeed");
    assert_eq!(
        unclaimed_secs(&db, &run_id).await,
        None,
        "a claimed row must take the run out of the latency worker's selection"
    );
}

/// A fresh submit reads as just-arrived. This is the half that makes deferral
/// mean something: the fleet's latency worker gets first refusal, rather than
/// racing a deferring node that already considers the run abandoned.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_global_submit_reads_as_just_arrived() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_global(&db, CUSTOM_KIND).await;
    let waited = unclaimed_secs(&db, &run_id)
        .await
        .expect("a fresh Global submit is selectable at once");
    assert!(
        waited < GRACE,
        "a run submitted a moment ago reports {waited}s unclaimed"
    );
}

/// The clock follows the queue row, not just the run's age.
///
/// A row handed back to the queue — a graceful release, a reaper re-queue, a
/// deferral — is a node saying "take this *now*". Reading the run's age alone
/// would call a long-lived run "unclaimed for an hour" in the same tick the
/// fleet first sees it, and the deferring node would race the workers for
/// exactly the heavy, long-running work it is deferring.
#[tokio::test(flavor = "multi_thread")]
async fn a_freshly_requeued_row_restarts_the_clock_on_an_old_run() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_global(&db, CUSTOM_KIND).await;

    // An hour-old run whose queue row was (re)queued just now.
    age_run(&db, &run_id, 3600).await;
    let waited = unclaimed_secs(&db, &run_id).await.expect("selectable");
    assert!(
        waited < GRACE,
        "an old run with a just-queued row reports {waited}s unclaimed — the \
         queue row's own `updated_at` is being ignored"
    );

    // Only once the row itself has sat does the run count as unclaimed.
    age_queue_rows(&db, &run_id, 3600).await;
    let waited = unclaimed_secs(&db, &run_id).await.expect("selectable");
    assert!(
        waited >= GRACE,
        "run and queue row both an hour old, yet only {waited}s unclaimed"
    );
}

/// The converse: a just-touched run is not unclaimed merely because its queue
/// row is old. The clock is the LATEST sign of life, not the earliest.
#[tokio::test(flavor = "multi_thread")]
async fn a_freshly_touched_run_restarts_the_clock_on_an_old_row() {
    let Some(db) = test_db().await else {
        return;
    };
    let run_id = seed_global(&db, CUSTOM_KIND).await;
    age_queue_rows(&db, &run_id, 3600).await;
    let waited = unclaimed_secs(&db, &run_id).await.expect("selectable");
    assert!(
        waited < GRACE,
        "a run updated a moment ago reports {waited}s unclaimed because its \
         queue row is old"
    );
}

/// A run whose driver died is unclaimed from the moment the lease LAPSED, not
/// from the last time anything wrote to it.
///
/// Until the lease lapses the run is not selectable at all, so the fleet has
/// had no chance at it. Counting from the run's age would hand a pipeline
/// whose worker just OOM-ed straight to the deferring node — the one pod this
/// policy exists to keep heavy work out of.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_drivers_lease_counts_from_when_it_lapsed() {
    let Some(db) = test_db().await else {
        return;
    };
    let ttl = crud::DRIVER_LEASE_TTL_SECS;
    let run_id = seed_global(&db, CUSTOM_KIND).await;
    age_run(&db, &run_id, 3600).await;
    age_queue_rows(&db, &run_id, 3600).await;

    // Still held: not selectable by anyone.
    leave_dead_lease(&db, &run_id, ttl - 20).await;
    assert_eq!(
        unclaimed_secs(&db, &run_id).await,
        None,
        "a run inside its driver's lease must not be selectable"
    );

    // Lapsed five seconds ago: selectable, and five seconds unclaimed — not an
    // hour.
    leave_dead_lease(&db, &run_id, ttl + 5).await;
    let waited = unclaimed_secs(&db, &run_id)
        .await
        .expect("a lapsed lease makes the run selectable again");
    assert!(
        waited < GRACE,
        "lease lapsed ~5s ago but the run reports {waited}s unclaimed — the \
         lease term is missing from the clock"
    );

    // Lapsed two minutes ago and still nobody: now it is unclaimed.
    leave_dead_lease(&db, &run_id, ttl + 120).await;
    let waited = unclaimed_secs(&db, &run_id).await.expect("selectable");
    assert!(
        waited >= GRACE,
        "lease lapsed two minutes ago, yet only {waited}s unclaimed"
    );
}
