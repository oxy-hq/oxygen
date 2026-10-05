//! `router::recovery` as the driver of a custom-app procedure run.
//!
//! The run's start handler enqueues `TaskScope::Global` and drives nothing, so
//! whether a procedure ever executes is decided here: by which roles leave the
//! kind at selection and for how long, by whether the registry the driver
//! injects knows it, and by `drive_pending` actually taking it. Going `Global`
//! before a driver could take the kind is what hung every airway run and had
//! to be reverted (`54355b198`); these are the three ways that could happen
//! again.

use std::time::{Duration, Instant};

use agentic_pipeline::recovery::{DrivePolicy, STRANDED_GRACE_SECS, may_drive};
use entity::customer_app_procedure_runs as proc_run;
use sea_orm::{ActiveModelTrait, ActiveValue};

use super::super::drive_policy::{IdeDeferral, drive_policy, drive_policy_for};
use super::*;
use crate::server::api::projects::automation_run::task::{
    PROCEDURE_RUN_KIND, ProcedureRunTask, submit,
};
use crate::server::role_manifest::{Role, current_process_role};

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
/// claims a procedure run fails it as an unknown `Custom` task.
#[test]
fn the_driver_registry_executes_procedure_runs() {
    let registry = build_custom_task_registry(
        &sea_orm::DatabaseConnection::default(),
        &PreaggCacheCtx::default(),
    );
    assert!(
        registry.get(PROCEDURE_RUN_KIND).is_some(),
        "`build_custom_task_registry` does not register {PROCEDURE_RUN_KIND}"
    );
}

/// `OXY_ROLE=all`: one process, no worker fleet. The node that accepted the
/// run must be the node that executes it, through nothing but its own
/// latency-worker pass — production's registry, production's drive policy.
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

    let automation: agentic_automation::AutomationConfig = serde_yaml::from_str(
        "name: greet\ntasks:\n  - name: greeting\n    type: formatter\n    \
         template: \"hello {{ params.store }}\"\n",
    )
    .expect("automation parses");
    let caller = uuid::Uuid::new_v4();
    let task = ProcedureRunTask {
        run_id: uuid::Uuid::new_v4(),
        workspace_id,
        procedure_id: "greet".into(),
        user_id: caller,
        staging_pin: None,
        automation: serde_json::to_value(&automation).expect("serialize"),
        params: Some(serde_json::json!({ "store": "s-7" })),
    };
    submit(&db, &task).await.expect("queue the run");

    let preagg = PreaggCacheCtx::default();
    let ctx = build_cloud_project_ctx(workspace_id, &path, &db, &preagg)
        .await
        .expect("the driver builds a context for the workspace");
    let runtime = Arc::new(RuntimeState::new());
    let router = agentic_runtime::router::noop_router();

    let deadline = Instant::now() + Duration::from_secs(60);
    let row = loop {
        drive_pending(
            &db,
            &runtime,
            ctx.clone(),
            None,
            None,
            None,
            &router,
            Some(workspace_id),
            &preagg,
            policy,
        )
        .await;
        let row = proc_run::Entity::find_by_id(task.run_id)
            .one(&db)
            .await
            .expect("row lookup")
            .expect("the run's row");
        if row.status != "running" {
            break row;
        }
        assert!(
            Instant::now() < deadline,
            "the run was never executed: with no worker fleet nothing else will"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    assert_eq!(row.status, "done", "error: {:?}", row.error_message);
    let outputs = row.result_outputs.expect("a done run records its outputs");
    assert!(
        outputs["greeting"].to_string().contains("hello s-7"),
        "outputs: {outputs}"
    );
}
