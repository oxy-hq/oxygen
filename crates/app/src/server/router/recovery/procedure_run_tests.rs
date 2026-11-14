//! `router::recovery` as the driver of a custom-app procedure run.
//!
//! The run's start handler enqueues `TaskScope::Global` and drives nothing, so
//! whether a procedure ever executes is decided here: by which roles leave the
//! kind at selection and for how long, by whether the registry the driver
//! injects knows it, and by `drive_pending` actually taking it. Going `Global`
//! before a driver could take the kind is what hung every airway run and had
//! to be reverted (`54355b198`); these are the three ways that could happen
//! again.
//!
//! The procedure run's executor is `oxy-api-custom-apps`' and reaches the
//! registry through `SurfaceSeams::tasks`; the kind is a required one
//! (`CUSTOM_APP_TASK_KINDS`), so a process without it does not boot. What is
//! left to decide here is the driver's: these tests hand the kind in as a stub
//! seam. The real executor's drive, through `recover_pending_global_runs`, is
//! `oxy-api-custom-apps`' `custom_app_procedure_run_queue`.

use std::time::{Duration, Instant};

use agentic_pipeline::recovery::{DrivePolicy, STRANDED_GRACE_SECS, may_drive};
use std::sync::atomic::{AtomicBool, Ordering};

use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use sea_orm::{ActiveModelTrait, ActiveValue};

use super::super::drive_policy::{IdeDeferral, drive_policy, drive_policy_for};
use super::*;
use crate::server::role_manifest::{Role, current_process_role};
use crate::server::router::seams::CUSTOM_APP_TASK_KINDS;

/// The kind `oxy-api-custom-apps` queues a procedure run under — a persisted
/// queue contract, so spelled out.
const PROCEDURE_RUN_KIND: &str = "custom_app_procedure_run";

const EVERY_ROLE: [Role; 4] = [Role::All, Role::Ide, Role::Worker, Role::Serve];
const NEITHER: IdeDeferral = IdeDeferral {
    airway: false,
    queue_work: false,
};
/// `OXY_IDE_DEFER_AIRWAY` alone.
const AIRWAY: IdeDeferral = IdeDeferral {
    airway: true,
    queue_work: false,
};
/// `OXY_IDE_DEFER_QUEUE_WORK` alone.
const QUEUE_WORK: IdeDeferral = IdeDeferral {
    airway: false,
    queue_work: true,
};
const BOTH: IdeDeferral = IdeDeferral {
    airway: true,
    queue_work: true,
};
const FRESH: u64 = 0;

/// Does `role`, deployed with `defer`, drive a procedure run that has been
/// selectable for `unclaimed_secs`? Production's decision and production's
/// predicate, not a restatement of either.
fn drives(role: Role, defer: IdeDeferral, unclaimed_secs: u64) -> bool {
    may_drive(
        Some(PROCEDURE_RUN_KIND),
        unclaimed_secs,
        drive_policy_for(role, defer),
    )
}

/// A deployment that sets no gate, or only the airway one, drives a procedure
/// run wherever it is first seen: no role waits.
#[test]
fn every_role_drives_a_fresh_procedure_run_unless_the_ide_defers_queue_work() {
    for defer in [NEITHER, AIRWAY] {
        for role in EVERY_ROLE {
            assert!(
                drives(role, defer, FRESH),
                "{role:?} with {defer:?} leaves a fresh procedure run"
            );
        }
    }
}

/// `OXY_IDE_DEFER_QUEUE_WORK` covers procedure runs: the ide leaves a fresh
/// one for the worker fleet, and takes it once nobody has claimed it for the
/// grace. Only the ide — `all` is the whole fleet in one process, and a
/// worker or a serve is where the ide is leaving it.
#[test]
fn an_ide_deferring_queue_work_leaves_a_procedure_run_for_the_fleet_then_takes_it() {
    for defer in [QUEUE_WORK, BOTH] {
        assert!(
            !drives(Role::Ide, defer, FRESH),
            "an ide with {defer:?} must leave a fresh procedure run for the fleet"
        );
        assert!(!drives(Role::Ide, defer, STRANDED_GRACE_SECS - 1));
        assert!(
            drives(Role::Ide, defer, STRANDED_GRACE_SECS),
            "an ide with {defer:?} must take a procedure run nobody claimed for the grace"
        );
        for role in [Role::All, Role::Worker, Role::Serve] {
            assert!(
                drives(role, defer, FRESH),
                "{role:?} must be unaffected by {defer:?}"
            );
        }
    }
}

/// No role may decline a procedure run for ever, under any gate. A role that
/// did would leave the run `queued` with nothing failing — on a
/// single-process install (`All`) permanently, since there is no other node
/// to take it. Leaving it for the grace is placement; a policy under which
/// waiting never opens the gate is a stall, and that is what this fails on.
#[test]
fn no_role_declines_a_procedure_run_for_ever() {
    for role in EVERY_ROLE {
        for defer in [NEITHER, AIRWAY, QUEUE_WORK, BOTH] {
            assert!(
                drives(role, defer, STRANDED_GRACE_SECS),
                "{role:?} with {defer:?} still declines a procedure run nobody \
                 claimed for the grace: it would never run there"
            );
        }
    }
}

/// The registry `drive_pending` injects must know the kind, or the pod that
/// claims a procedure run fails it as an unknown `Custom` task. The executor
/// arrives through the seams, and the kind being required is what makes a
/// process without it refuse to start rather than drain runs into failures.
#[test]
fn a_process_without_the_procedure_run_executor_does_not_start() {
    assert!(
        CUSTOM_APP_TASK_KINDS.contains(&PROCEDURE_RUN_KIND),
        "{PROCEDURE_RUN_KIND} must stay a required custom-app task kind"
    );
}

/// Set by [`Stub`] when the driver hands it a task. One test uses it.
static STUB_RAN: AtomicBool = AtomicBool::new(false);

/// The procedure run's executor, as far as the driver can tell: the seam hands
/// in something that runs the kind.
struct Stub;

#[async_trait::async_trait]
impl TaskExecutor for Stub {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        if matches!(&assignment.spec, TaskSpec::Custom { kind, .. } if kind == PROCEDURE_RUN_KIND) {
            STUB_RAN.store(true, Ordering::SeqCst);
        }
        Err("stub".to_string())
    }
}

/// `OXY_ROLE=all`: one process, no worker fleet. The node that accepted the
/// run must be the node that executes it, through nothing but its own
/// latency-worker pass — production's registry built from the seams,
/// production's drive policy.
///
/// Scoped to this test's workspace because lib tests share one database:
/// `drive_pending` is what `tick_cloud` calls per workspace, and calling it
/// for ours alone leaves every other test's queued work untouched.
#[tokio::test]
async fn a_single_process_executes_the_run_it_queued() {
    let Some(db) = crate::server::test_support::test_db().await else {
        return;
    };
    assert_eq!(
        current_process_role().as_str(),
        Role::All.as_str(),
        "this test states what a single-process deployment does; an unset \
         OXY_ROLE derives `all`"
    );
    // The one value this process reads at boot and hands every selection.
    let policy = drive_policy();
    assert_eq!(
        policy,
        DrivePolicy::ALL,
        "a single process is the whole fleet: it defers nothing, whatever gate is set"
    );

    let dir = tempfile::tempdir().expect("workspace dir");
    std::fs::write(dir.path().join("config.yml"), "databases: []\nmodels: []\n")
        .expect("write config.yml");
    let workspace_id = uuid::Uuid::new_v4();
    let path = dir.path().to_string_lossy().to_string();
    entity::workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("procedure-run-{workspace_id}")),
        path: ActiveValue::Set(Some(path.clone())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");

    // Queued as the start handler queues it: an `agentic_runs` row and a
    // `Global` entry of the procedure-run kind.
    let task_id = uuid::Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(
        &db,
        &task_id,
        "procedure: greet",
        None,
        PROCEDURE_RUN_KIND,
        None,
        workspace_id,
    )
    .await
    .expect("the run's agentic_runs row");
    let spec = TaskSpec::Custom {
        kind: PROCEDURE_RUN_KIND.to_string(),
        payload: serde_json::json!({}),
    };
    agentic_runtime::crud::enqueue_task(
        &db,
        &task_id,
        &task_id,
        None,
        &spec,
        None,
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await
    .expect("queue the run");
    let tasks = [TaskSeam {
        kind: PROCEDURE_RUN_KIND,
        build: |_| Arc::new(Stub),
    }];

    let preagg = PreaggCacheCtx::default();
    let ctx = build_cloud_project_ctx_inner(workspace_id, &path, &db, &preagg)
        .await
        .map(Arc::new)
        .expect("the driver builds a context for the workspace");
    let runtime = Arc::new(RuntimeState::new());
    let router = agentic_runtime::router::noop_router();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        drive_pending(
            &db,
            &runtime,
            ctx.clone(),
            None,
            None,
            &router,
            Some(workspace_id),
            &preagg,
            &tasks,
            policy,
        )
        .await;
        if STUB_RAN.load(Ordering::SeqCst) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the run was never executed: with no worker fleet nothing else will"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
