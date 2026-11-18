//! Unit tests for the call-trace read-back (`invocation_calls.rs`).

use chrono::{Duration, Utc};
use entity::{app_function_invocation_calls, app_function_invocations};
use uuid::Uuid;

use super::{answer, reason};

fn invocation(
    status: &str,
    age: Duration,
    duration_ms: Option<i64>,
) -> app_function_invocations::Model {
    app_function_invocations::Model {
        id: Uuid::new_v4(),
        app_id: Uuid::new_v4(),
        build_id: Uuid::new_v4(),
        function_name: "store".into(),
        mode: "route".into(),
        user_id: None,
        status: status.into(),
        duration_ms,
        error: None,
        cancel_requested_at: None,
        created_at: (Utc::now() - age).fixed_offset(),
        idempotency_key: None,
        result_body: None,
        request_hash: None,
        result_status: None,
        failure_fingerprint: None,
        environment: "production".into(),
        credential_token_id: None,
        error_kind: None,
        parent_invocation_id: None,
    }
}

fn trace(init_ms: Option<i32>) -> app_function_invocation_calls::Model {
    app_function_invocation_calls::Model {
        invocation_id: Uuid::new_v4(),
        app_id: Uuid::new_v4(),
        created_at: Utc::now().fixed_offset(),
        init_ms,
        total_calls: 1,
        dropped: 0,
        unfinished: 0,
        calls: serde_json::json!({ "v": 1, "calls": [{
            "seq": 0, "op": "query", "family": "query", "start_ms": 40,
            "duration_ms": 12, "status": "ok", "verb": "SELECT", "target": "oxymart"
        }] }),
    }
}

fn why(status: &str, age: Duration, duration_ms: Option<i64>) -> Option<&'static str> {
    reason(&invocation(status, age, duration_ms), None, 3, Utc::now())
}

/// Each way a trace can be missing reads as its own reason, and a `running`
/// row is reported running only inside the function ceiling — an orphan a
/// crash left is not polled forever.
#[test]
fn a_missing_trace_says_why() {
    let s = Duration::seconds;
    assert_eq!(why("running", s(10), None), Some("running"));
    assert_eq!(why("running", s(400), None), Some("not_recorded"));
    assert_eq!(
        why("success", s(3), Some(1_000)),
        Some("running"),
        "write on its way"
    );
    assert_eq!(why("success", s(120), Some(1_000)), Some("not_recorded"));
    assert_eq!(why("shed", s(1), None), Some("did_not_start"));
    assert_eq!(why("error", Duration::days(4), Some(5)), Some("expired"));
}

#[test]
fn a_stored_trace_is_recorded_unless_tenant_code_never_ran() {
    let iv = invocation("success", Duration::seconds(5), Some(80));
    assert_eq!(reason(&iv, Some(&trace(Some(38))), 3, Utc::now()), None);
    assert_eq!(
        reason(&iv, Some(&trace(None)), 3, Utc::now()),
        Some("did_not_start")
    );
}

#[test]
fn the_answer_carries_the_stored_calls_and_the_invocation_facts() {
    let iv = invocation("success", Duration::seconds(5), Some(80));
    let id = iv.id;
    let out = answer(iv, Some(trace(Some(38))), Some("b-42".into()), Utc::now());
    assert!(out.recorded);
    assert_eq!(out.invocation_id, id);
    assert_eq!(out.init_ms, Some(38));
    assert_eq!(out.calls.len(), 1);
    assert_eq!(out.calls[0].target.as_deref(), Some("oxymart"));
    assert_eq!(out.build_id.as_deref(), Some("b-42"));
    assert!(!out.failed);

    let iv = invocation("running", Duration::seconds(1), None);
    let out = answer(iv, None, None, Utc::now());
    assert!(!out.recorded);
    assert_eq!(out.reason, Some("running"));
    assert!(out.calls.is_empty());
}
