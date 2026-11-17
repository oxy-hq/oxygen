use entity::app_publish_attempts;
use serde_json::json;
use uuid::Uuid;

use super::*;

fn attempt(build_id: &str, outcome: &str, minute: u32) -> app_publish_attempts::Model {
    serde_json::from_value(json!({
        "id": Uuid::new_v4(),
        "app_id": Uuid::from_u128(1),
        "app_build_id": null,
        "build_id": build_id,
        "environment": "staging",
        "published_by": null,
        "published_via": null,
        "published_token_id": null,
        "started_at": format!("2026-11-17T10:{minute:02}:00+00:00"),
        "finished_at": format!("2026-11-17T10:{minute:02}:01+00:00"),
        "outcome": outcome,
        "failed_stage": null,
        "error": null,
        "stage_ms": {},
    }))
    .expect("an attempt row")
}

/// The two outcomes' windows interleave by time: a long run of failures
/// after the last landed build leaves the landed ones in the answer.
#[test]
fn the_two_windows_merge_newest_first() {
    let landed = vec![attempt("ok-2", "success", 2), attempt("ok-1", "success", 1)];
    let failed = vec![
        attempt("bad-9", "failed", 9),
        attempt("bad-8", "failed", 8),
        attempt("bad-0", "failed", 0),
    ];
    let merged: Vec<String> = newest_first(landed, failed)
        .into_iter()
        .map(|a| a.build_id)
        .collect();
    assert_eq!(merged, ["bad-9", "bad-8", "ok-2", "ok-1", "bad-0"]);
}

#[test]
fn a_read_never_asks_for_more_than_retention_keeps() {
    assert_eq!(DEFAULT_LIST_LIMIT, KEEP_ATTEMPTS);
    assert_eq!(clamp_limit(None), KEEP_ATTEMPTS);
    assert_eq!(clamp_limit(Some(10)), 10);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(10_000)), KEEP_ATTEMPTS);
}
