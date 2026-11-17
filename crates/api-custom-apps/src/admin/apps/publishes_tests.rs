use entity::app_publish_attempts;
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::custom_apps_publish_attempts::{DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT, clamp_limit};

/// An attempt row as the table stores it. `stage_ms` is JSONB: it comes back
/// in no particular key order.
fn row(
    outcome: &str,
    failed_stage: Option<&str>,
    error: Option<&str>,
) -> app_publish_attempts::Model {
    serde_json::from_value(json!({
        "id": Uuid::from_u128(1),
        "app_id": Uuid::from_u128(2),
        "app_build_id": if outcome == "success" { Some(Uuid::from_u128(3)) } else { None },
        "build_id": "ci-42",
        "environment": "production",
        "published_by": Uuid::from_u128(4),
        "published_via": null,
        "published_token_id": null,
        "started_at": "2026-11-17T10:00:00+00:00",
        "finished_at": "2026-11-17T10:00:01.250+00:00",
        "outcome": outcome,
        "failed_stage": failed_stage,
        "error": error,
        "stage_ms": { "point": 30, "admit": 12, "inflate": 340, "store": 800 },
    }))
    .expect("an attempt row")
}

/// The wire shape the console and `oxyc api` read: every field named, the
/// stages as an ordered list, the duration computed.
#[test]
fn a_failed_attempt_answers_its_stage_its_error_and_its_timings_in_order() {
    let error = "migration 0003_add_index.sql failed: relation \"orders\" does not exist";
    let summary = PublishAttemptSummary::of(
        row("failed", Some("promote"), Some(error)),
        Some("dev@example.com".into()),
    );
    let body = serde_json::to_value(PublishAttemptList {
        attempts: vec![summary],
    })
    .expect("serialize");
    assert_eq!(
        body,
        json!({
            "attempts": [{
                "id": Uuid::from_u128(1),
                "app_build_id": null,
                "build_id": "ci-42",
                "environment": "production",
                "outcome": "failed",
                "failed_stage": "promote",
                "error": error,
                "started_at": "2026-11-17T10:00:00+00:00",
                "finished_at": "2026-11-17T10:00:01.250+00:00",
                "duration_ms": 1250,
                "stages": [
                    { "stage": "admit", "ms": 12 },
                    { "stage": "inflate", "ms": 340 },
                    { "stage": "store", "ms": 800 },
                    { "stage": "point", "ms": 30 },
                ],
                "published_by_email": "dev@example.com",
                "published_via": null,
                "published_token_id": null,
            }]
        })
    );
}

#[test]
fn a_landed_attempt_names_its_build_row() {
    let summary = PublishAttemptSummary::of(row("success", None, None), None);
    assert_eq!(summary.app_build_id, Some(Uuid::from_u128(3)));
    assert_eq!(summary.failed_stage, None);
    assert_eq!(summary.error, None);
    assert_eq!(summary.published_by_email, None);
}

#[test]
fn the_limit_defaults_and_is_held_to_its_bounds() {
    assert_eq!(clamp_limit(None), DEFAULT_LIST_LIMIT);
    assert_eq!(clamp_limit(Some(10)), 10);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(10_000)), MAX_LIST_LIMIT);
}
