//! The pure halves of held runs: the flag, and reading steps and holds back
//! out of an automation's results. The queue itself is exercised against a
//! database in `tests/platform/preview_runs.rs`.

use serde_json::json;

use super::notes::{count_held, first_held};
use super::report::{outcome_of, step_kind, step_status};
use super::*;

#[test]
fn the_flag_is_off_unless_truthy() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var(RUNS_FLAG_ENV) };
    assert!(!runs_enabled());
    for (value, on) in [
        ("1", true),
        ("true", true),
        ("on", true),
        ("", false),
        ("0", false),
        ("no", false),
    ] {
        unsafe { std::env::set_var(RUNS_FLAG_ENV, value) };
        assert_eq!(runs_enabled(), on, "{value:?}");
    }
}

#[test]
fn every_kind_of_hold_is_read_back_in_the_contract_shape() {
    let results = json!({
        "load": { "rows": [[1]], "sql": "SELECT 1" },
        "write": { "columns": [], "rows": [], "sql": "INSERT INTO j VALUES (1)",
                   "preview": { "held": true, "reason": "r", "verb": "INSERT", "targets": ["j"] } },
        "post": { "status": null, "preview": { "held": true, "reason": "h", "method": "POST", "url": "https://x" } },
        "sync": { "preview": { "held": true, "reason": "a", "verb": "AIRWAY RUN", "targets": ["airway/t.airway.yml"], "pipeline_ref": "airway/t.airway.yml" } },
        "loop": { "iterations": [ { "preview": { "held": true, "verb": "DELETE", "targets": ["x"], "reason": "r" }, "sql": "DELETE FROM x" } ] }
    });
    assert_eq!(count_held(&results), 4);
    let write = first_held(&results["write"]).unwrap();
    assert_eq!(
        (write.verb.as_str(), write.targets.clone()),
        ("INSERT", vec!["j".to_string()])
    );
    assert_eq!(write.sql.as_deref(), Some("INSERT INTO j VALUES (1)"));
    let post = first_held(&results["post"]).unwrap();
    assert_eq!(
        (post.verb.as_str(), post.targets.clone(), post.sql),
        ("POST", vec!["https://x".to_string()], None)
    );
    assert_eq!(
        first_held(&results["loop"]).unwrap().verb,
        "DELETE",
        "found at depth"
    );
    assert!(first_held(&results["load"]).is_none());
    // A note that says it is not held is not a hold.
    assert_eq!(
        count_held(&json!({ "s": { "preview": { "held": false } } })),
        0
    );
}

#[test]
fn a_step_reads_as_where_the_run_is() {
    // Finished successfully: results decide.
    assert_eq!(
        step_status(true, false, 0, 2, Some("succeeded")),
        "succeeded"
    );
    assert_eq!(step_status(true, true, 1, 2, Some("succeeded")), "held");
    // Running at step 1.
    assert_eq!(step_status(true, false, 0, 1, None), "succeeded");
    assert_eq!(step_status(false, false, 1, 1, None), "running");
    assert_eq!(step_status(false, false, 2, 1, None), "pending");
    // Failed at step 1: that step failed (its result is the error), the rest never ran.
    assert_eq!(step_status(true, false, 1, 1, Some("failed")), "failed");
    assert_eq!(step_status(false, false, 2, 1, Some("failed")), "pending");
}

#[test]
fn kinds_and_outcomes_map_to_the_contract() {
    assert_eq!(step_kind(Some("execute_sql")), "execute_sql");
    assert_eq!(step_kind(Some("semantic_query")), "other");
    assert_eq!(step_kind(None), "other");
    assert_eq!(outcome_of(Some("done")), Some("succeeded"));
    assert_eq!(outcome_of(Some("timed_out")), Some("failed"));
    assert_eq!(outcome_of(Some("cancelled")), Some("cancelled"));
    assert_eq!(outcome_of(Some("running")), None);
    assert_eq!(outcome_of(None), None);
}

#[test]
fn refusals_carry_the_contract_codes() {
    use axum::http::StatusCode;
    for (e, code, status) in [
        (
            RunRequestError::Disabled,
            "preview_runs_disabled",
            StatusCode::NOT_FOUND,
        ),
        (
            RunRequestError::PreviewNotFound("b".into()),
            "preview_not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            RunRequestError::NotReady("b".into()),
            "preview_not_ready",
            StatusCode::CONFLICT,
        ),
        (
            RunRequestError::RefNotInRevision("r".into()),
            "ref_not_in_revision",
            StatusCode::NOT_FOUND,
        ),
        (
            RunRequestError::BadRequest("x".into()),
            "bad_request",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!((e.code(), e.status()), (code, status));
    }
}

#[test]
fn the_ceiling_defaults_to_an_hour_and_takes_a_positive_override() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var(MAX_MINUTES_ENV) };
    assert_eq!(max_minutes(), 60);
    for (value, want) in [
        ("15", 15),
        (" 90 ", 90),
        ("0", 60),
        ("-5", 60),
        ("soon", 60),
    ] {
        unsafe { std::env::set_var(MAX_MINUTES_ENV, value) };
        assert_eq!(max_minutes(), want, "{value:?}");
    }
}
