//! Op-level units: the log cap, the reply envelope, an unbound op.

use super::*;

/// The cap has to actually cap. The first version returned at the marker
/// without pushing it, so the buffer parked at exactly MAX forever: every
/// later call saw `Equal`, re-emitted the marker, and `Silent` was
/// unreachable — 99,500 identical warn events for a 100k-line loop, each
/// retained in the span until close. Pushing the marker is what advances
/// the count past MAX and makes the third state reachable.
#[test]
fn the_log_cap_emits_exactly_one_marker_then_goes_silent() {
    use super::super::ops::{LogAction, MAX_CAPTURED_LOGS, log_action};

    assert_eq!(log_action(0), LogAction::Emit);
    assert_eq!(log_action(MAX_CAPTURED_LOGS - 1), LogAction::Emit);
    assert_eq!(log_action(MAX_CAPTURED_LOGS), LogAction::Marker);
    // The marker pushes a line, so the very next call is already over.
    assert_eq!(log_action(MAX_CAPTURED_LOGS + 1), LogAction::Silent);

    // Simulate the loop the cap exists for: 10k lines, one marker.
    let mut buffered = 0usize;
    let mut markers = 0;
    let mut emitted = 0;
    for _ in 0..10_000 {
        match log_action(buffered) {
            LogAction::Emit => {
                emitted += 1;
                buffered += 1;
            }
            LogAction::Marker => {
                markers += 1;
                buffered += 1;
            }
            LogAction::Silent => {}
        }
    }
    assert_eq!(
        markers, 1,
        "the marker must fire once, not per dropped line"
    );
    assert_eq!(emitted, MAX_CAPTURED_LOGS);
    assert_eq!(buffered, MAX_CAPTURED_LOGS + 1);
}

#[test]
fn reply_json_ok_passes_value_through() {
    let out = reply_json("ctx.query", Ok(serde_json::json!({ "rows": [] })));
    assert_eq!(out, r#"{"rows":[]}"#);
}

#[test]
fn reply_json_err_wraps_as_oxy_error() {
    let out = reply_json("ctx.warehouse", Err("boom".into()));
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["__oxyError"], "HostError");
    assert_eq!(parsed["message"], "ctx.warehouse: boom");
}

#[test]
fn cancelled_flag_defaults_false() {
    let flag = Arc::new(AtomicBool::new(false));
    assert!(!flag.load(Ordering::Relaxed));
    flag.store(true, Ordering::Relaxed);
    assert!(flag.load(Ordering::Relaxed));
}

/// An op that reaches an isolate before an invocation is bound is refused with
/// the dropped-broker text; it must not panic the isolate thread.
#[test]
fn an_op_with_no_invocation_bound_is_refused_not_panicked() {
    let refusal = super::super::ops::host_sender_of(None)
        .err()
        .expect("refused");
    assert_eq!(refusal.to_string(), "function host unavailable");
}
