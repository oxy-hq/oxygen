use super::{APP_FUNCTION_KIND, AppEnvironment, invocation_mode, read_task};
use crate::custom_apps_functions::FunctionJobTrigger;
use agentic_core::delegation::TaskSpec;
use serde_json::json;

fn spec(payload: serde_json::Value) -> TaskSpec {
    TaskSpec::Custom {
        kind: APP_FUNCTION_KIND.into(),
        payload,
    }
}

const APP: &str = "6f1c1f5e-4b2a-4d8e-9d59-0b8f8a3e2c11";

/// A task queued before `environment` existed is production, and runs.
#[test]
fn a_task_without_an_environment_runs_as_production() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "trigger": "scheduled",
    })))
    .expect("a legacy task is runnable");
    assert_eq!(job.app_id.to_string(), APP);
    assert_eq!(job.mode, "schedule");
    assert_eq!(job.environment, AppEnvironment::Production);
}

#[test]
fn a_task_naming_production_runs() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "environment": "production",
    })))
    .expect("runnable");
    assert_eq!(job.environment, AppEnvironment::Production);
}

/// The collision this reader prevents: a worker that ignored the field
/// would resolve the production build and run a staging task against it.
/// The environment rides to the runner, which refuses it unless the
/// function is a check there (`custom_apps::app_environments_phase_1b` and
/// `custom_app_functions_manual_run::environment_checks` drive both ends).
#[test]
fn a_task_for_a_non_production_environment_keeps_its_environment() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "environment": "staging",
    })))
    .expect("a known environment is read");
    assert_eq!(
        job.environment,
        AppEnvironment::Staging,
        "never read as production"
    );
}

#[test]
fn a_task_naming_an_unknown_environment_is_refused() {
    let err = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "environment": "Production",
    })))
    .expect_err("names are exact; an unknown one is not production");
    assert!(err.contains("unknown environment"), "{err}");
}

#[test]
fn the_input_rides_through_as_the_request_body() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "trigger": "manual",
        "input": { "store": 7 },
    })))
    .expect("runnable");
    assert_eq!(job.input, br#"{"store":7}"#);
    assert_eq!(job.mode, "manual");
}

/// Every label the seeding side can produce must have an arm here. Written
/// over the enum rather than string literals so adding a variant without an
/// arm fails this test instead of silently reporting `schedule` — and the
/// runner must read the label back as the trigger that wrote it.
#[test]
fn every_trigger_label_maps_to_its_own_mode() {
    for (trigger, mode) in [
        (FunctionJobTrigger::Manual, "manual"),
        (FunctionJobTrigger::Webhook, "webhook"),
        (FunctionJobTrigger::ScheduleNow, "manual"),
        (FunctionJobTrigger::Event, "event"),
    ] {
        let label = trigger.as_str();
        assert_eq!(
            invocation_mode(Some(label)),
            mode,
            "trigger '{label}' is falling through to the legacy `schedule` default"
        );
        assert_eq!(
            super::QueuedTrigger::from_label(Some(label)),
            trigger.queued(),
            "the runner reads '{label}' as another trigger"
        );
    }
}

#[test]
fn an_event_run_reports_mode_event() {
    assert_eq!(invocation_mode(Some("event")), "event");
}

/// Run now `as=schedule` in a sandbox: a `manual` invocation, read as the
/// trigger whose rule admits a scheduled function there.
#[test]
fn a_schedule_now_task_is_a_manual_run_admitted_as_a_schedule() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "refresh", "trigger": "schedule_now",
        "environment": "dev-a1",
    })))
    .expect("runnable");
    assert_eq!(job.mode, "manual");
    assert_eq!(job.trigger, super::QueuedTrigger::ScheduleNow);
    assert_eq!(job.environment.name(), "dev-a1");
}

/// A task queued before the field existed is a cron fire, and stays one.
#[test]
fn a_payload_without_a_trigger_is_a_schedule() {
    assert_eq!(invocation_mode(None), "schedule");
}

const DELIVERY: &str = "0b5f3c1e-8f0e-4c55-a3f4-6d3f1f6e2a77";

#[test]
fn an_event_task_names_its_delivery() {
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "on-upload", "trigger": "event",
        "event_delivery_id": DELIVERY,
    })))
    .expect("runnable");
    assert_eq!(
        job.event_delivery_id.map(|d| d.to_string()),
        Some(DELIVERY.into())
    );
    assert_eq!(job.mode, "event");
}

/// The delivery is what an event run answers for: without one there is
/// nothing to record the attempt on, so the task is refused.
#[test]
fn an_event_task_without_its_delivery_is_refused() {
    let err = read_task(&spec(json!({
        "app_id": APP, "function_name": "on-upload", "trigger": "event",
    })))
    .expect_err("no delivery");
    assert!(err.contains("delivery"), "{err}");
    let err = read_task(&spec(json!({
        "app_id": APP, "function_name": "on-upload", "trigger": "event",
        "event_delivery_id": "not-a-uuid",
    })))
    .expect_err("a bad delivery id");
    assert!(err.contains("delivery"), "{err}");
}

/// A label this release does not know was queued by a newer one: the task is
/// refused (the queue retries it, on a newer pod) and never read as a cron fire.
#[test]
fn a_task_with_a_trigger_this_release_does_not_know_is_refused() {
    for label in ["send", "backfill", "teleport"] {
        let err = read_task(&spec(json!({
            "app_id": APP, "function_name": "refresh", "trigger": label,
        })))
        .expect_err("an unknown trigger fails closed");
        assert!(err.contains("newer release"), "{label}: {err}");
    }
}

/// A call (`ctx.apps.call` or a delivered `ctx.apps.send`) reports mode `call`,
/// and the one that rides a delivery carries its id.
#[test]
fn a_call_task_reports_mode_call_and_carries_its_delivery() {
    assert_eq!(invocation_mode(Some("call")), "call");
    let job = read_task(&spec(json!({
        "app_id": APP, "function_name": "handle", "trigger": "call",
        "event_delivery_id": DELIVERY,
    })))
    .expect("runnable");
    assert_eq!(job.mode, "call");
    assert_eq!(job.trigger, super::QueuedTrigger::Call);
    assert!(job.event_delivery_id.is_some());
}
