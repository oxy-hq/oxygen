//! Unit tests for the internal-jobs admin module.
//!
//! These tests cover the pieces that don't require a live Postgres
//! connection — task-type extraction, status-counter accumulation,
//! scheduled-jobs registry shape. The DB-dependent endpoints
//! (`reenqueue_dead`, `delete_dead`, `fetch_queue_stats`) are covered
//! end-to-end by the integration test suite under
//! `tests/admin_internal_jobs.rs` when a Postgres connection is
//! available; the unit suite here keeps fast feedback on the parts that
//! can run without one.

use super::super::internal_jobs_fleet::scheduled_jobs;
use super::super::internal_jobs_rows::{extract_task_type, redact_secrets};
use super::{QueueStatusCounts, accumulate};
use serde_json::json;

#[test]
fn redact_secrets_blanks_secret_shaped_keys_recursively() {
    let spec = json!({
        "type": "airway",
        "pipeline_ref": "pipelines/toast.airway.yml",
        "variables": {
            "host": "db.internal",
            "password": "hunter2",
            "api_key": "sk-live-abc",
            "nested": { "client_secret": "shh", "rows": 10 }
        },
        "list": [{ "token": "t0" }, { "ok": "keep" }]
    });
    let out = redact_secrets(spec);
    assert_eq!(out["pipeline_ref"], json!("pipelines/toast.airway.yml"));
    assert_eq!(out["variables"]["host"], json!("db.internal"));
    assert_eq!(out["variables"]["password"], json!("***redacted***"));
    assert_eq!(out["variables"]["api_key"], json!("***redacted***"));
    assert_eq!(
        out["variables"]["nested"]["client_secret"],
        json!("***redacted***")
    );
    assert_eq!(out["variables"]["nested"]["rows"], json!(10));
    assert_eq!(out["list"][0]["token"], json!("***redacted***"));
    assert_eq!(out["list"][1]["ok"], json!("keep"));
}

#[test]
fn redact_secrets_leaves_clean_specs_untouched() {
    let spec = json!({"type": "agent", "agent_id": "analytics", "question": "revenue?"});
    assert_eq!(redact_secrets(spec.clone()), spec);
}

#[test]
fn accumulate_routes_each_status_into_the_right_field() {
    let mut counts = QueueStatusCounts::default();
    accumulate(&mut counts, "queued", 2);
    accumulate(&mut counts, "claimed", 3);
    accumulate(&mut counts, "completed", 5);
    accumulate(&mut counts, "failed", 7);
    accumulate(&mut counts, "cancelled", 11);
    accumulate(&mut counts, "dead", 13);
    assert_eq!(counts.queued, 2);
    assert_eq!(counts.claimed, 3);
    assert_eq!(counts.completed, 5);
    assert_eq!(counts.failed, 7);
    assert_eq!(counts.cancelled, 11);
    assert_eq!(counts.dead, 13);
}

#[test]
fn accumulate_ignores_unknown_statuses() {
    let mut counts = QueueStatusCounts::default();
    accumulate(&mut counts, "completed", 4);
    accumulate(&mut counts, "novel_status_we_do_not_track", 99);
    assert_eq!(counts.completed, 4);
    // Everything else stayed at default 0.
    assert_eq!(
        counts,
        QueueStatusCounts {
            queued: 0,
            claimed: 0,
            completed: 4,
            failed: 0,
            cancelled: 0,
            dead: 0,
        }
    );
}

#[test]
fn extract_task_type_reads_the_internal_tag() {
    // Real `TaskSpec` is internally tagged: the variant lives under `type`.
    let spec = json!({"type": "agent", "agent_id": "analytics", "question": "hi"});
    assert_eq!(extract_task_type(&spec), Some("agent".to_string()));
}

#[test]
fn extract_task_type_falls_back_to_first_object_key() {
    let spec = json!({"AnalyticsTurn": {"foo": 1}});
    assert_eq!(extract_task_type(&spec), Some("AnalyticsTurn".to_string()));
}

#[test]
fn extract_task_type_returns_none_for_non_object() {
    assert_eq!(extract_task_type(&json!("plain string")), None);
    assert_eq!(extract_task_type(&json!(42)), None);
    assert_eq!(extract_task_type(&json!(null)), None);
}

#[test]
fn extract_task_type_returns_none_for_empty_object() {
    assert_eq!(extract_task_type(&json!({})), None);
}

#[test]
fn scheduled_jobs_registry_lists_the_known_loops() {
    let jobs = scheduled_jobs();
    assert_eq!(jobs.len(), 4);
    let names: Vec<&str> = jobs.iter().map(|j| j.name).collect();
    assert!(names.contains(&"reaper"));
    assert!(names.contains(&"matcher_health_probe"));
    assert!(names.contains(&"worker_recovery_loop"));
    assert!(names.contains(&"task_queue_retention"));
}

#[test]
fn manual_triggers_are_reaper_and_retention() {
    let jobs = scheduled_jobs();

    let reaper = jobs.iter().find(|j| j.name == "reaper").unwrap();
    assert_eq!(
        reaper.trigger_path,
        Some("/api/admin/internal-jobs/run-reaper")
    );
    let retention = jobs
        .iter()
        .find(|j| j.name == "task_queue_retention")
        .unwrap();
    assert_eq!(
        retention.trigger_path,
        Some("/api/admin/internal-jobs/run-retention")
    );

    // Exactly those two are triggerable; the rest are observation-only.
    let triggerable: Vec<&str> = jobs
        .iter()
        .filter(|j| j.trigger_path.is_some())
        .map(|j| j.name)
        .collect();
    assert_eq!(triggerable.len(), 2);
    for j in jobs
        .iter()
        .filter(|j| j.name == "matcher_health_probe" || j.name == "worker_recovery_loop")
    {
        assert!(
            j.trigger_path.is_none(),
            "{} should not have a trigger_path",
            j.name
        );
    }
}
