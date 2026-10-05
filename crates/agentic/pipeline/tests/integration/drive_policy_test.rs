//! The placement gate against Postgres: what `recover_pending_global_runs`
//! actually takes the driver lease on, under each [`DrivePolicy`] form.
//!
//! The unit tests on `may_drive` pin the answer; these pin the consequence. A
//! run the policy declines must come out of a drive pass untouched —
//! `driver_id IS NULL`, no recovery budget spent — because that is the whole
//! handoff: an undriven run stays selectable by the node that should take it.
//! "Driven" is read off `agentic_runs.attempt`, which `recover_single_run`
//! increments only after it wins the driver lease, and which stays put when
//! the run later goes terminal (the lease itself is cleared then).
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(drive_policy_test)'

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use agentic_core::delegation::TaskSpec;
use agentic_pipeline::automation_run::start_automation_run;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext, RunPlatformResolver};
use agentic_pipeline::recovery::{
    DrivePolicy, STRANDED_GRACE_SECS, partition_drivable, recover_active_runs,
    recover_pending_global_runs, recover_stranded_runs,
};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};
use crate::run_platform_resolver_test::request;

/// A kind the periodic stranded tick does not select (it covers `workflow` and
/// `airway` only), so the latency worker's own fallback is its only net.
const CUSTOM_KIND: &str = "preagg_cycle";

/// The shape a deferring `ide` runs with in production.
const ONLY_COMPILE: DrivePolicy = DrivePolicy::Only(&["compile"]);

/// A root run stamped `source_type` with one `queued` Global queue row.
async fn seed_global(db: &DatabaseConnection, ws: Uuid, source_type: &str) -> String {
    let run_id = format!("{source_type}-policy-{}", Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, source_type, None, ws)
        .await
        .expect("insert run");
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Custom {
            kind: source_type.into(),
            payload: serde_json::json!({}),
        },
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("enqueue");
    run_id
}

/// The same, with the `source_type` column NULL — a row written by something
/// other than `insert_run`, which always stamps one.
async fn seed_untyped_global(db: &DatabaseConnection, ws: Uuid) -> String {
    let run_id = seed_global(db, ws, "untyped").await;
    exec(
        db,
        "UPDATE agentic_runs SET source_type = NULL WHERE id = $1",
        &run_id,
    )
    .await;
    run_id
}

async fn exec(db: &DatabaseConnection, sql: &str, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [run_id.into()],
    ))
    .await
    .unwrap();
}

/// Make the run look as if it has sat unclaimed for two minutes: backdate the
/// run row and its queue rows, the two clocks `unclaimed_secs` reads.
async fn let_it_wait(db: &DatabaseConnection, run_id: &str) {
    exec(
        db,
        "UPDATE agentic_runs SET updated_at = now() - interval '2 minutes' WHERE id = $1",
        run_id,
    )
    .await;
    exec(
        db,
        "UPDATE agentic_task_queue SET updated_at = now() - interval '2 minutes' \
         WHERE task_id = $1 OR task_id LIKE $1 || '.%'",
        run_id,
    )
    .await;
}

/// One latency-worker drive pass over `ws`, as `drive_pending` makes it.
///
/// The return value is not used: it counts runs whose drive *succeeded*, and
/// these fixtures (no executor registered, a fake platform) are not built to
/// succeed. What a pass took the lease on is read off the run rows instead.
async fn drive_pass(db: &DatabaseConnection, ws: Uuid, policy: DrivePolicy) {
    let base: Arc<dyn PlatformContext> = Arc::new(FakePlatform);
    recover_pending_global_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        base,
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        None,
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(ws),
        None,
        policy,
    )
    .await;
}

/// How many times a driver has taken this run's lease.
async fn times_driven(db: &DatabaseConnection, run_id: &str) -> i32 {
    crud::get_run(db, run_id)
        .await
        .unwrap()
        .expect("run row")
        .attempt
}

async fn assert_untouched(db: &DatabaseConnection, run_id: &str, why: &str) {
    let row = crud::get_run(db, run_id).await.unwrap().expect("run row");
    assert_eq!(row.driver_id, None, "{why}: the driver lease was taken");
    assert_eq!(row.attempt, 0, "{why}: recovery budget was spent");
}

/// The safety net, at the layer that takes the lease — for a kind the periodic
/// tick does not cover, for the automation kind, and for a run with no
/// `source_type` at all.
///
/// Fresh, each is left exactly as it was found, so the fleet can take it.
/// Unclaimed past the grace, each is driven here. Without the second half a
/// deferring `ide` beside a missing or wedged worker fleet would leave all
/// three queued forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferring_node_takes_unclaimed_work_after_the_grace() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let custom = seed_global(&db, ws, CUSTOM_KIND).await;
    let automation = start_automation_run(
        &db,
        request("defer.procedure.yml"),
        crud::TaskScope::Global,
        ws,
    )
    .await
    .expect("seed automation run");
    let untyped = seed_untyped_global(&db, ws).await;
    let all = [&custom, &automation, &untyped];

    drive_pass(&db, ws, ONLY_COMPILE).await;
    for run_id in all {
        assert_untouched(&db, run_id, &format!("fresh {run_id}")).await;
    }

    for run_id in all {
        let_it_wait(&db, run_id).await;
    }
    drive_pass(&db, ws, ONLY_COMPILE).await;
    for run_id in all {
        assert_eq!(
            times_driven(&db, run_id).await,
            1,
            "{run_id} sat unclaimed past the {STRANDED_GRACE_SECS}s grace and \
             was still not driven by the deferring node"
        );
    }
}

/// The kind the deferring node keeps is driven at once, with no wait — and the
/// unlisted run beside it, in the same workspace and the same pass, is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferring_node_drives_its_own_kind_at_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let compile = seed_global(&db, ws, "compile").await;
    let custom = seed_global(&db, ws, CUSTOM_KIND).await;

    drive_pass(&db, ws, ONLY_COMPILE).await;
    assert_eq!(
        times_driven(&db, &compile).await,
        1,
        "the listed kind must not wait out any grace"
    );
    assert_untouched(&db, &custom, "fresh unlisted run beside a listed one").await;
}

/// A deny-list is a capability, and waiting does not grow one. A compile no
/// `ide` has taken — the `ide` is down — must still be there when it comes
/// back: a worker that took it after the grace would fail it on a node with no
/// working copy and spend its recovery budget doing so.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_cannot_run_a_kind_never_takes_it_however_long_it_waits() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let compile = seed_global(&db, ws, "compile").await;
    let_it_wait(&db, &compile).await;

    let worker = DrivePolicy::Except(&["compile"]);
    drive_pass(&db, ws, worker).await;
    assert_untouched(&db, &compile, "long-unclaimed compile on a worker").await;
}

/// The probe and the gate agree — checked on what they do, not on how they
/// are written.
///
/// The cloud latency worker selects twice. The *probe* reads every workspace's
/// pending runs at once and keeps the workspaces holding something this node
/// drives; the *gate* re-selects inside each visited workspace and takes the
/// lease. If the probe skips more than the gate would drive, that work
/// silently stops running and nothing fails.
///
/// So: compute the probe's answer the way `tick_cloud` does, then run the gate
/// over EVERY workspace — including the ones the probe would have skipped —
/// and require that the runs the gate drove are exactly the runs the probe
/// said to drive. Three workspaces cover the three ways they could split:
/// nothing to drive, something to drive only because it waited, and a listed
/// kind beside an unlisted one.
#[tokio::test(flavor = "multi_thread")]
async fn the_probe_and_the_gate_drive_the_same_runs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (ws_fresh, ws_waited, ws_mixed) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let fresh_custom = seed_global(&db, ws_fresh, CUSTOM_KIND).await;
    let waited_custom = seed_global(&db, ws_waited, CUSTOM_KIND).await;
    let_it_wait(&db, &waited_custom).await;
    let mixed_compile = seed_global(&db, ws_mixed, "compile").await;
    let mixed_custom = seed_global(&db, ws_mixed, CUSTOM_KIND).await;
    let mine = [ws_fresh, ws_waited, ws_mixed];
    let seeded = [
        (ws_fresh, &fresh_custom),
        (ws_waited, &waited_custom),
        (ws_mixed, &mixed_compile),
        (ws_mixed, &mixed_custom),
    ];

    // The probe: one unfiltered selection, split by the policy. Other tests
    // share this database, so keep only this test's workspaces.
    let pending: Vec<_> = crud::find_pending_global_runs(&db, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| mine.contains(&r.workspace_id))
        .collect();
    assert_eq!(
        pending.len(),
        seeded.len(),
        "the probe did not select every seeded run"
    );
    let (probe_drive, _) = partition_drivable(pending, ONLY_COMPILE);
    let probe_runs: BTreeSet<String> = probe_drive.iter().map(|r| r.run_id.clone()).collect();
    let probe_workspaces: BTreeSet<Uuid> = probe_drive.iter().map(|r| r.workspace_id).collect();

    // The gate, over every workspace rather than only the probed ones.
    for ws in mine {
        drive_pass(&db, ws, ONLY_COMPILE).await;
    }
    let mut gate_runs = BTreeSet::new();
    let mut gate_workspaces = BTreeSet::new();
    for (ws, run_id) in seeded {
        if times_driven(&db, run_id).await > 0 {
            gate_runs.insert(run_id.clone());
            gate_workspaces.insert(ws);
        }
    }

    assert_eq!(
        probe_runs, gate_runs,
        "the probe and the gate disagree about which runs this node drives"
    );
    assert_eq!(
        probe_workspaces, gate_workspaces,
        "the probe would skip a workspace the gate drives work in (or visit one it drives nothing in)"
    );
    // And the agreement is on the right answer, not merely mutual.
    assert_eq!(
        gate_runs,
        BTreeSet::from([waited_custom.clone(), mixed_compile.clone()]),
        "wrong runs driven under {ONLY_COMPILE:?}"
    );
    assert_untouched(&db, &fresh_custom, "fresh unlisted run").await;
    assert_untouched(&db, &mixed_custom, "fresh unlisted run beside a listed one").await;
}

/// One periodic-tick pass over `ws`, as `recover_all_workspaces` makes it —
/// reaper pre-pass included. Read what it took off the run rows, as with
/// [`drive_pass`].
async fn stranded_pass(db: &DatabaseConnection, ws: Uuid, policy: DrivePolicy) {
    let base: Arc<dyn PlatformContext> = Arc::new(FakePlatform);
    recover_stranded_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        base,
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        None,
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(ws),
        None,
        policy,
    )
    .await;
}

/// An hour-old airway pipeline whose worker died holding both the queue claim
/// and the driver lease: the row's heartbeat is past the visibility timeout
/// (the reaper will re-queue it) and the lease has just lapsed (the run is
/// selectable again).
async fn seed_pipeline_whose_worker_died(db: &DatabaseConnection, ws: Uuid) -> String {
    let run_id = seed_global(db, ws, "airway").await;
    exec(
        db,
        "UPDATE agentic_runs SET updated_at = now() - interval '1 hour' WHERE id = $1",
        &run_id,
    )
    .await;
    crud::claim_task_under_root(db, "oom-killed-worker", &run_id)
        .await
        .unwrap()
        .expect("claim must succeed");
    exec(
        db,
        "UPDATE agentic_task_queue SET last_heartbeat = now() - interval '10 minutes' \
         WHERE task_id = $1",
        &run_id,
    )
    .await;
    lapse_dead_lease(db, &run_id, 2).await;
    run_id
}

/// The dead worker's driver lease, lapsed `secs_ago` seconds ago.
async fn lapse_dead_lease(db: &DatabaseConnection, run_id: &str, secs_ago: i64) {
    let ttl = crud::DRIVER_LEASE_TTL_SECS + secs_ago;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs \
         SET driver_id = 'oom-killed-worker', \
             driver_heartbeat_at = now() - ($1 || ' seconds')::interval \
         WHERE id = $2",
        [ttl.into(), run_id.into()],
    ))
    .await
    .unwrap();
}

/// The reaper-then-drive path. The periodic tick is the pass that frees a dead
/// worker's claim, and it selects the freed run in the same call — with the
/// run's `updated_at` an hour past the grace and nothing else excluding it.
/// Before the gate reached this path, a deferring `ide` drove the pipeline it
/// had just reaped: the heavy run the flag exists to keep off that node,
/// arriving by the one selection the flag did not cover.
///
/// Now the tick applies the same policy as the latency worker, on the same
/// clock: the reaper's re-queue restarts it, so the deferring node leaves the
/// pipeline for the fleet for one grace — and takes it once nobody has.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferring_node_leaves_a_reaped_pipeline_for_the_fleet_first() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let pipeline = seed_pipeline_whose_worker_died(&db, ws).await;

    // The tick that reaps it must not be the tick that drives it.
    stranded_pass(&db, ws, ONLY_COMPILE).await;
    let row = crud::get_run(&db, &pipeline)
        .await
        .unwrap()
        .expect("run row");
    assert_eq!(
        row.attempt, 0,
        "the deferring node drove the pipeline in the same tick its reaper freed it"
    );
    let queued = crud::get_queue_entry(&db, &pipeline)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(
        queued.queue_status, "queued",
        "the pre-pass should have re-queued the dead worker's claim"
    );

    // Still nobody after the grace: the fallback, same as the latency worker's.
    exec(
        &db,
        "UPDATE agentic_task_queue SET updated_at = now() - interval '2 minutes' \
         WHERE task_id = $1",
        &pipeline,
    )
    .await;
    lapse_dead_lease(&db, &pipeline, 120).await;
    stranded_pass(&db, ws, ONLY_COMPILE).await;
    assert_eq!(
        times_driven(&db, &pipeline).await,
        1,
        "the pipeline sat unclaimed past the {STRANDED_GRACE_SECS}s grace and the \
         deferring node's tick still left it"
    );
}

/// A worker's tick is unchanged by the gate: `Except([compile])` never applies
/// to the `workflow` / `airway` runs this path selects, so a reaped pipeline is
/// driven at once — which is what makes "leave it for the fleet" a handoff
/// rather than a delay.
#[tokio::test(flavor = "multi_thread")]
async fn a_workers_tick_takes_a_reaped_pipeline_at_once() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let pipeline = seed_pipeline_whose_worker_died(&db, ws).await;

    stranded_pass(&db, ws, DrivePolicy::Except(&["compile"])).await;
    assert_eq!(
        times_driven(&db, &pipeline).await,
        1,
        "a worker's tick reaped the pipeline and then did not drive it"
    );
}

/// Records every root the startup pass gets as far as driving, and refuses
/// each one — so a pass drives nothing, and what it *would* have driven is
/// exactly the roots that got past the placement gate. (The startup pass does
/// not spend recovery budget, so `attempt` cannot tell us.)
#[derive(Default)]
struct WouldDrive(Mutex<Vec<String>>);

#[async_trait]
impl RunPlatformResolver for WouldDrive {
    async fn platform_for(
        &self,
        root: &agentic_runtime::entity::run::Model,
        _base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        self.0.lock().unwrap().push(root.id.clone());
        Err("recording only".into())
    }
}

/// One startup pass over `ws` — reaper pre-pass included — returning the
/// roots it would have driven.
async fn startup_pass(db: &DatabaseConnection, ws: Uuid, policy: DrivePolicy) -> Vec<String> {
    let base: Arc<dyn PlatformContext> = Arc::new(FakePlatform);
    let would = Arc::new(WouldDrive::default());
    recover_active_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        base,
        would.clone(),
        None,
        None,
        None,
        None,
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(ws),
        None,
        policy,
    )
    .await;
    would.0.lock().unwrap().clone()
}

/// The other pass that reaps. `get_resumable_root_runs` selects every unleased
/// active root in the workspace, not just this process's own, so an `ide`
/// booting while an OOM-killed worker's pipeline sits reapable would re-queue
/// it in its pre-pass and drive it — ungated. With the gate, the startup pass
/// leaves it for the fleet like the other two loops do, because its queued
/// Global row is what every node's latency worker polls for.
///
/// What it must still keep is a root no other loop is sure to see: one with no
/// queued Global row (here an airway run left `needs_resume` with nothing
/// queued). Declining that would leave it to the periodic tick at best — and
/// for an analytics or builder run, to nobody.
#[tokio::test(flavor = "multi_thread")]
async fn a_deferring_nodes_startup_pass_leaves_queued_work_for_the_fleet() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let reaped = seed_pipeline_whose_worker_died(&db, ws).await;
    let compile = seed_global(&db, ws, "compile").await;
    let unqueued = format!("airway-unqueued-{}", Uuid::new_v4());
    crud::insert_run(&db, &unqueued, "Q", None, "airway", None, ws)
        .await
        .expect("insert run");
    exec(
        &db,
        "UPDATE agentic_runs SET task_status = 'needs_resume' WHERE id = $1",
        &unqueued,
    )
    .await;

    let driven: BTreeSet<String> = startup_pass(&db, ws, ONLY_COMPILE)
        .await
        .into_iter()
        .collect();
    assert!(
        !driven.contains(&reaped),
        "the deferring node's startup pass drove the pipeline its own reaper had \
         just freed"
    );
    assert!(
        driven.contains(&compile),
        "the startup pass declined the kind this node keeps"
    );
    assert!(
        driven.contains(&unqueued),
        "the startup pass declined a root no other loop is guaranteed to see"
    );
    let queued = crud::get_queue_entry(&db, &reaped)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(
        queued.queue_status, "queued",
        "the pre-pass should have re-queued the dead worker's claim, leaving it \
         visible to the fleet's latency workers"
    );

    // With no preference, the same pass takes the pipeline as it always did.
    let driven = startup_pass(&db, ws, DrivePolicy::ALL).await;
    assert!(
        driven.contains(&reaped),
        "with DrivePolicy::ALL the startup pass must still resume the reaped pipeline"
    );
}
