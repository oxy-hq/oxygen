//! Unit tests for the Functions admin read side (`functions.rs`).

use super::to_summary;
use serde_json::json;

#[test]
fn summary_projects_manifest_surfaces_and_retries() {
    // Route defaults on when no other surface is declared.
    let s = to_summary("plain".into(), Some(&json!({})));
    assert!(s.route);
    assert!(s.schedule.is_none());
    assert!(!s.airway);

    // A schedule-only function is still route-servable (no explicit
    // route:false) — the badge reflects actual runtime invocability — and its
    // schedule / retries / secrets are all projected.
    let s = to_summary(
        "cron".into(),
        Some(&json!({
            "schedule": "*/50 * * * *",
            "timezone": "UTC",
            "retries": { "maxAttempts": 3, "minTimeoutMs": 1000, "maxTimeoutMs": 30000 },
            "secrets": { "write": true }
        })),
    );
    assert!(s.route);
    assert_eq!(s.schedule.as_deref(), Some("*/50 * * * *"));
    assert!(s.secrets_write);
    assert_eq!(s.retries.expect("retries projected").max_attempts, 3);

    // Explicit opt-out → not route-servable.
    let opted_out = to_summary(
        "no-route".into(),
        Some(&json!({ "route": false, "schedule": "0 * * * *" })),
    );
    assert!(!opted_out.route);

    // maxAttempts <= 1 is not a retry policy.
    let s = to_summary(
        "once".into(),
        Some(&json!({ "retries": { "maxAttempts": 1 } })),
    );
    assert!(s.retries.is_none());
}

#[test]
fn summary_projects_the_check_flag() {
    let with = json!({ "check": true, "schedule": "*/15 * * * *" });
    assert!(to_summary("smoke".into(), Some(&with)).check);
    let without = json!({ "route": true });
    assert!(!to_summary("echo".into(), Some(&without)).check);
    assert!(!to_summary("bare".into(), None).check);
}
