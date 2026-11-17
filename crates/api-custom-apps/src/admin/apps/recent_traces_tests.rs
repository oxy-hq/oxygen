//! Unit tests for the app map's trace digest (`recent_traces.rs`).

use axum::http::StatusCode;
use chrono::DateTime;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    DEFAULT_PER_FUNCTION, MAX_PER_FUNCTION, TraceRow, assemble, parse_per_function, touches_of,
};
use crate::custom_apps_call_trace::{CallRecord, CallStatus, calls_of};

fn call(family: &str, op: &str, verb: Option<&str>, target: Option<&str>) -> CallRecord {
    CallRecord {
        seq: 0,
        op: op.into(),
        family: family.into(),
        start_ms: 0,
        duration_ms: 1,
        status: CallStatus::Ok,
        error_type: None,
        verb: verb.map(Into::into),
        target: target.map(Into::into),
        count: None,
        rows: None,
        rows_truncated: None,
        http_status: None,
    }
}

fn failed(mut call: CallRecord, error_type: &str) -> CallRecord {
    call.status = CallStatus::Error;
    call.error_type = Some(error_type.into());
    call
}

/// A loop of a hundred identical queries is one shape called a hundred times;
/// a different verb, target or outcome is a shape of its own, so a failure is
/// never folded into the successes beside it.
#[test]
fn a_run_folds_to_its_distinct_call_shapes_in_first_call_order() {
    let select = || call("query", "query", Some("SELECT"), Some("orders"));
    let mut calls: Vec<CallRecord> = (0..100).map(|_| select()).collect();
    calls.insert(
        1,
        call("fetch", "fetch", Some("POST"), Some("api.stripe.com")),
    );
    calls.push(call("query", "query", Some("INSERT"), Some("orders")));
    calls.push(failed(select(), "timeout"));
    calls.push(failed(select(), "timeout"));

    let touches = touches_of(calls);
    let shapes: Vec<_> = touches
        .iter()
        .map(|t| {
            (
                t.family.as_str(),
                t.verb.as_deref(),
                t.target.as_deref(),
                t.status,
                t.calls,
            )
        })
        .collect();
    assert_eq!(
        shapes,
        vec![
            ("query", Some("SELECT"), Some("orders"), CallStatus::Ok, 100),
            (
                "fetch",
                Some("POST"),
                Some("api.stripe.com"),
                CallStatus::Ok,
                1
            ),
            ("query", Some("INSERT"), Some("orders"), CallStatus::Ok, 1),
            (
                "query",
                Some("SELECT"),
                Some("orders"),
                CallStatus::Error,
                2
            ),
        ]
    );
    assert_eq!(touches[3].error_type.as_deref(), Some("timeout"));
    assert!(touches_of(Vec::new()).is_empty());
}

/// The bound is a bound: absent is the default, and outside 1–20 is a 400
/// rather than a quiet clamp that would read as the whole history.
#[test]
fn runs_per_function_is_bounded_and_out_of_range_is_refused() {
    assert_eq!(parse_per_function(None).unwrap(), DEFAULT_PER_FUNCTION);
    assert_eq!(parse_per_function(Some(1)).unwrap(), 1);
    assert_eq!(
        parse_per_function(Some(MAX_PER_FUNCTION)).unwrap(),
        MAX_PER_FUNCTION
    );
    for refused in [0, MAX_PER_FUNCTION + 1, u64::MAX] {
        let e = parse_per_function(Some(refused)).expect_err("out of range");
        assert_eq!(
            (e.status, e.code),
            (StatusCode::BAD_REQUEST, "invalid_per_function"),
            "{refused}"
        );
    }
}

fn document(calls: &[CallRecord]) -> Value {
    json!({ "v": 1, "calls": calls })
}

fn run(function: &str, at: &str, status: &str, calls: &[CallRecord]) -> TraceRow {
    TraceRow {
        function_name: function.into(),
        invocation_id: Some(Uuid::new_v4()),
        created_at: Some(DateTime::parse_from_rfc3339(at).unwrap()),
        mode: Some("route".into()),
        status: Some(status.into()),
        failure_fingerprint: None,
        total_calls: Some(calls.len() as i32),
        dropped: Some(0),
        calls: Some(document(calls)),
    }
}

/// What the lateral join's `LEFT … ON TRUE` gives a function with no run.
fn no_run(function: &str) -> TraceRow {
    TraceRow {
        function_name: function.into(),
        invocation_id: None,
        created_at: None,
        mode: None,
        status: None,
        failure_fingerprint: None,
        total_calls: None,
        dropped: None,
        calls: None,
    }
}

/// Every function of the build is answered, in the order the query gives,
/// with its runs in the order the query gives — and one that never ran is
/// present with none, so the map can say "no recorded run" instead of
/// dropping it.
#[test]
fn rows_assemble_into_one_entry_per_function_keeping_functions_that_never_ran() {
    let orders = call("query", "query", Some("SELECT"), Some("orders"));
    let rows = vec![
        no_run("cleanup"),
        run(
            "sync",
            "2026-11-17T10:00:00Z",
            "success",
            std::slice::from_ref(&orders),
        ),
        run(
            "sync",
            "2026-11-17T09:00:00Z",
            "error",
            std::slice::from_ref(&orders),
        ),
        run("webhook", "2026-11-16T08:00:00Z", "success", &[]),
    ];
    let functions = assemble(rows);

    let names: Vec<_> = functions.iter().map(|f| f.function.as_str()).collect();
    assert_eq!(names, ["cleanup", "sync", "webhook"]);
    assert!(functions[0].traces.is_empty());

    let sync = &functions[1].traces;
    assert_eq!(sync.len(), 2);
    assert!(sync[0].created_at > sync[1].created_at, "newest first");
    // The pager's rule decides `failed`, not the raw status alone.
    assert_eq!((sync[0].failed, sync[1].failed), (false, true));
    assert_eq!(sync[0].touches[0].target.as_deref(), Some("orders"));

    // A run that made no calls is still a run: it counts towards freshness.
    assert_eq!(functions[2].traces.len(), 1);
    assert!(functions[2].traces[0].touches.is_empty());
}

/// A `success` the pager fingerprinted (it caught a failed ctx call, or
/// answered 5xx) is a failed run here too.
#[test]
fn a_fingerprinted_success_is_a_failed_run() {
    let mut row = run("sync", "2026-11-17T10:00:00Z", "success", &[]);
    row.failure_fingerprint = Some("af597dfd3a7536b8".into());
    assert!(assemble(vec![row])[0].traces[0].failed);
}

/// A stored document of a version this binary does not read is a run with no
/// touches, never a failed read: `calls_of`'s own rule, applied here.
#[test]
fn an_unknown_trace_version_is_a_run_with_no_touches() {
    let mut row = run("sync", "2026-11-17T10:00:00Z", "success", &[]);
    row.calls = Some(json!({ "v": 99, "calls": [{ "family": "fetch" }] }));
    assert!(calls_of(row.calls.as_ref().unwrap()).is_empty());
    let functions = assemble(vec![row]);
    assert_eq!(functions[0].traces.len(), 1);
    assert!(functions[0].traces[0].touches.is_empty());
}

/// The wire shape the console reads: absent facets are omitted, the status is
/// lowercase, and a shape's count is `calls`.
#[test]
fn a_touch_serializes_without_absent_facets() {
    let touches = touches_of(vec![
        call("notify", "notify", None, None),
        failed(
            call("fetch", "fetch", Some("GET"), Some("example.com")),
            "upstream_5xx",
        ),
    ]);
    assert_eq!(
        serde_json::to_value(&touches).unwrap(),
        json!([
            { "family": "notify", "op": "notify", "status": "ok", "calls": 1 },
            {
                "family": "fetch", "op": "fetch", "verb": "GET", "target": "example.com",
                "status": "error", "error_type": "upstream_5xx", "calls": 1
            }
        ])
    );
}
