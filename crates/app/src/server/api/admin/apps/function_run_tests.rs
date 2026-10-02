//! Unit tests for the function-run read-back (`function_run.rs`).

use super::{completed_invocation_id, effective_run_status, run_belongs_to_app, run_environment};
use serde_json::json;
use uuid::Uuid;

/// A run names its environment only when its trigger did; everything
/// queued before the field, and every production run, is production.
#[test]
fn a_run_without_a_stamped_environment_is_production() {
    assert_eq!(run_environment(None), "production");
    assert_eq!(
        run_environment(Some(&json!({ "trigger": "manual" }))),
        "production"
    );
    assert_eq!(
        run_environment(Some(&json!({ "environment": "staging" }))),
        "staging"
    );
    assert_eq!(
        run_environment(Some(&json!({ "environment": 7 }))),
        "production"
    );
}

/// The invocation is read from the completed event — the last one, since
/// each retry writes its own row — and is absent while the run is queued.
#[test]
fn a_run_reports_the_invocation_its_last_completed_event_names() {
    let first = Uuid::new_v4();
    let last = Uuid::new_v4();
    let started = json!({ "function_name": "smoke" });
    let log = json!({ "level": "log", "message": "x", "invocation_id": Uuid::new_v4() });
    let one = json!({ "status": "error", "invocation_id": first });
    let two = json!({ "status": "success", "invocation_id": last });

    let running = [("app_function_started", &started), ("function_log", &log)];
    assert_eq!(completed_invocation_id(running.into_iter()), None);

    let retried = [
        ("app_function_started", &started),
        ("app_function_completed", &one),
        ("function_log", &log),
        ("app_function_completed", &two),
    ];
    assert_eq!(completed_invocation_id(retried.into_iter()), Some(last));

    // An event from before the field carries none, and is not an error.
    let legacy = json!({ "status": "success" });
    assert_eq!(
        completed_invocation_id([("app_function_completed", &legacy)].into_iter()),
        None
    );
}

#[test]
fn effective_status_reports_queued_vs_running() {
    // Freshly enqueued: run stamped "running" but the queue task is still
    // queued (no worker claimed it) → report "queued", not a false "running".
    assert_eq!(
        effective_run_status(Some("running"), Some("queued")),
        "queued"
    );
    // Claimed by a worker → genuinely running.
    assert_eq!(
        effective_run_status(Some("running"), Some("claimed")),
        "running"
    );
    // A terminal run status always wins over the queue.
    assert_eq!(
        effective_run_status(Some("done"), Some("completed")),
        "done"
    );
    assert_eq!(effective_run_status(Some("failed"), None), "failed");
    assert_eq!(effective_run_status(Some("timed_out"), None), "timed_out");
    // Dead-lettered (retries exhausted) surfaces as failed.
    assert_eq!(
        effective_run_status(Some("running"), Some("dead")),
        "failed"
    );
    // No queue row (already pruned) falls back to the run status.
    assert_eq!(effective_run_status(Some("running"), None), "running");
}

#[test]
fn ownership_guard_matches_only_this_apps_function_runs() {
    let app = Uuid::new_v4();
    let other = Uuid::new_v4();

    // A function run seeded for this app → owned.
    assert!(run_belongs_to_app(
        Some("app_function"),
        &format!("fn:{app}/refresh-token"),
        app
    ));
    // Another app's function run → rejected (the cross-app read guard).
    assert!(!run_belongs_to_app(
        Some("app_function"),
        &format!("fn:{other}/refresh-token"),
        app
    ));
    // Right question shape but wrong source_type → rejected.
    assert!(!run_belongs_to_app(
        Some("workflow"),
        &format!("fn:{app}/refresh-token"),
        app
    ));
    // Non-function run (e.g. an agent run) → rejected.
    assert!(!run_belongs_to_app(
        Some("app_function"),
        "some agent question",
        app
    ));
    assert!(!run_belongs_to_app(None, &format!("fn:{app}/x"), app));
}
