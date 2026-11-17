//! Unit tests for the Functions admin read side (`functions.rs`).

use super::{next_run_at, to_summary};
use chrono::{TimeZone, Utc};
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

#[test]
fn summary_projects_declared_schemas() {
    let input = json!({ "type": "object", "properties": { "store": { "type": "integer" } } });
    let output = json!({ "type": "array", "items": { "type": "string" } });
    let s = to_summary(
        "quote".into(),
        Some(&json!({ "inputSchema": input, "outputSchema": output })),
    );
    assert_eq!(s.input_schema, Some(input));
    assert_eq!(s.output_schema, Some(output));

    // Absent → None; a value that is not a schema object is not projected
    // (publish refuses one, so only a pre-gate row could hold it).
    let s = to_summary("bare".into(), Some(&json!({ "inputSchema": "object" })));
    assert!(s.input_schema.is_none());
    assert!(s.output_schema.is_none());
    assert!(to_summary("none".into(), None).input_schema.is_none());

    // Serialized under the snake_case names the console reads.
    let wire = serde_json::to_value(to_summary(
        "quote".into(),
        Some(&json!({ "inputSchema": { "type": "object" } })),
    ))
    .unwrap();
    assert_eq!(wire["input_schema"], json!({ "type": "object" }));
    assert_eq!(wire["output_schema"], serde_json::Value::Null);
}

/// The next run is the scheduler's own reading of the cron, in the schedule's
/// timezone: 09:00 in New York is 14:00 UTC in November (EST) and 13:00 in
/// July (EDT), and no timezone means UTC, as the schedule row defaults it.
#[test]
fn next_run_is_the_schedulers_reading_in_the_timezone() {
    let november = Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap();
    let ny = next_run_at(Some("0 9 * * *"), Some("America/New_York"), november);
    assert_eq!(ny.as_deref(), Some("2026-11-17T14:00:00Z"));
    let july = Utc.with_ymd_and_hms(2026, 7, 1, 10, 0, 0).unwrap();
    let ny = next_run_at(Some("0 9 * * *"), Some("America/New_York"), july);
    assert_eq!(ny.as_deref(), Some("2026-07-01T13:00:00Z"));
    // No timezone: UTC, so 09:00 has passed and the next is tomorrow's.
    let utc = next_run_at(Some("0 9 * * *"), None, november);
    assert_eq!(utc.as_deref(), Some("2026-11-18T09:00:00Z"));
    // Strictly after: a fire due exactly now is the following one.
    let on_the_hour = next_run_at(Some("0 * * * *"), None, november);
    assert_eq!(on_the_hour.as_deref(), Some("2026-11-17T11:00:00Z"));
    // No schedule, an unreadable one, or an unknown zone: no next run.
    assert_eq!(next_run_at(None, None, november), None);
    assert_eq!(next_run_at(Some("every tuesday"), None, november), None);
    assert_eq!(
        next_run_at(Some("0 9 * * *"), Some("Mars/Olympus"), november),
        None
    );
}

#[test]
fn summary_carries_the_next_run_of_a_schedule() {
    let cron = json!({ "schedule": "*/15 * * * *" });
    assert!(to_summary("tick".into(), Some(&cron)).next_run_at.is_some());
    assert!(
        to_summary("plain".into(), Some(&json!({})))
            .next_run_at
            .is_none()
    );
}
