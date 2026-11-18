use super::*;
use crate::custom_apps_functions::env_policy::{Decision, HostOp};

fn resolved(environment: AppEnvironment) -> ResolvedEnvironment {
    ResolvedEnvironment {
        environment,
        build_id: Some(Uuid::nil()),
    }
}

fn check() -> Declared {
    Declared {
        check: true,
        ..Declared::default()
    }
}

fn manual(environment: AppEnvironment, declared: Declared) -> Result<Admission, QueuedError> {
    admit_queued(
        &resolved(environment),
        QueuedTrigger::Manual,
        declared,
        None,
    )
}

/// Production is admitted whatever the function declares, with the policy
/// that allows every op: a cron fire, a webhook and Run now are unchanged.
#[test]
fn check_run_admits_production_whatever_the_function_declares() {
    let triggers = [
        QueuedTrigger::Cron,
        QueuedTrigger::Manual,
        QueuedTrigger::Webhook,
    ];
    for trigger in triggers {
        for declared in [check(), Declared::default()] {
            let admission = admit_queued(
                &resolved(AppEnvironment::Production),
                trigger,
                declared,
                None,
            )
            .expect("production is always admitted");
            assert!(admission.policy.is_production(), "{trigger:?}");
            assert_eq!(admission.policy.decide(HostOp::StoragePut), Decision::Allow);
        }
    }
}

/// Outside production a check is admitted, and under the environment's
/// own policy — never production's — so its writes are held or isolated.
#[test]
fn check_run_admits_a_check_outside_production_under_that_environments_policy() {
    let admission = manual(AppEnvironment::Staging, check()).expect("a check runs in staging");
    assert_eq!(admission.environment.environment, AppEnvironment::Staging);
    assert!(!admission.policy.is_production());
    assert_eq!(admission.policy.environment(), &AppEnvironment::Staging);
    assert_ne!(
        admission.policy.decide(HostOp::WarehouseInsert),
        Decision::Allow,
        "a staging check must not write production's warehouse"
    );
}

/// Outside production, a manual run of a function that is not a check is
/// refused by the trigger's rule — before the gate is asked.
#[test]
fn check_run_refuses_a_manual_run_outside_production_that_is_not_a_check() {
    let dev = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    for environment in [AppEnvironment::Staging, dev] {
        let refused = manual(environment.clone(), Declared::default())
            .expect_err("a manual run outside production is a check");
        assert_eq!(
            refused,
            QueuedError::Refused(QueuedRefused {
                environment: environment.clone(),
                why: QueuedRefusal::NotACheck,
            }),
            "{environment}"
        );
    }
}

/// A cron fire admitted outside production runs under that environment's
/// policy; one with the switch off never reaches the gate.
#[test]
fn check_run_admits_a_cron_fire_only_with_the_switch_on() {
    let scheduled = Declared {
        schedule: true,
        ..Declared::default()
    };
    let dev = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    let on = Some(EnvironmentState {
        schedules_enabled: true,
        expired: false,
        ..EnvironmentState::default()
    });
    let admission = admit_queued(&resolved(dev.clone()), QueuedTrigger::Cron, scheduled, on)
        .expect("a scheduled function fires with the switch on");
    assert_eq!(admission.policy.environment(), &dev);
    let off = Some(EnvironmentState::default());
    assert!(matches!(
        admit_queued(&resolved(dev), QueuedTrigger::Cron, scheduled, off),
        Err(QueuedError::Refused(QueuedRefused {
            why: QueuedRefusal::SchedulesOff,
            ..
        }))
    ));
}

/// `admit_queued` adds no admission of its own: a run is admitted
/// exactly where the gate admits a staff route call, so an environment
/// the gate refuses outright stays refused for a queued run too.
#[test]
fn check_run_admits_a_check_only_where_the_gate_admits_a_staff_route_call() {
    let dev = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    for environment in [AppEnvironment::Staging, dev] {
        let resolved = resolved(environment.clone());
        let by_route = environment_gate::admit(
            &resolved,
            Entrance::Route {
                non_production_reach: true,
            },
        )
        .map_err(QueuedError::Gate);
        assert_eq!(
            admit_queued(&resolved, QueuedTrigger::Manual, check(), None),
            by_route,
            "{environment}: a check takes the staff route entrance and nothing wider"
        );
    }
}

#[test]
fn check_run_reads_the_check_flag_strictly() {
    let yes = serde_json::json!({ "check": true });
    assert!(manifest_marks_check(Some(&yes)));

    // Everything else is a no, and each of these is a shape a real manifest
    // produces: a handler with no `check` key, one that opted out, one
    // whose value is a truthy non-boolean, and a row with no manifest.
    for no in [
        serde_json::json!({}),
        serde_json::json!({ "check": false }),
        serde_json::json!({ "check": "yes" }),
        serde_json::json!({ "check": 1 }),
        serde_json::json!(null),
    ] {
        assert!(!manifest_marks_check(Some(&no)), "{no}");
    }
    assert!(!manifest_marks_check(None));
}
