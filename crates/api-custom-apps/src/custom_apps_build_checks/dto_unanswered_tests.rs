//! The answer where nobody judged a check, or nobody will: a skip that is no
//! pass, and a check a runner owes once its clock has run out.

use super::*;

/// Just past the thirty minutes a claim made at `at` may stay open.
fn past(at: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    at + Duration::seconds(30 * 60 + 1)
}

/// A check the platform closed without judging it (staging had moved on by
/// the claim) is `skipped` on the wire and marked `not_checked`: beside
/// passes it never rolls up as `passed`.
#[test]
fn an_attempt_with_a_check_nobody_judged_never_reads_passed() {
    let unjudged = |kind: &str| {
        let mut row = row(kind, 1, "skipped", false);
        row.detail = Some(json!({ "findings": [], "not_checked": true }));
        row
    };
    let mut rows = vec![
        row("validation", 1, "passed", false),
        row("design_system", 1, "passed", false),
        unjudged("functions"),
        unjudged("loads"),
    ];
    let out = response(&build(), "staging", rows.clone(), None, soon());
    assert_eq!((out.state, out.stalled), ("none", None));
    assert_eq!(out.attempts[0].state, "none");
    assert_eq!(attempt_state(&rows, soon()), "none");
    assert_eq!((out.counts.passed, out.counts.skipped), (2, 2));
    let loads = out.checks.iter().find(|c| c.kind == "loads").unwrap();
    assert_eq!(loads.status, "skipped", "the wire's status is unchanged");
    assert_eq!(loads.detail["not_checked"], true);

    // A verdict beside it decides as for any attempt: a failure, an open check.
    rows[1].status = "failed".into();
    assert_eq!(attempt_state(&rows, soon()), "failed");
    rows[1].status = "running".into();
    assert_eq!(attempt_state(&rows, soon()), "running");

    // A skip the check decided itself is a pass, as before; and an advisory
    // row nobody judged decides nothing.
    let mut advisory = unjudged("changes");
    advisory.advisory = true;
    let own = vec![
        row("validation", 1, "passed", false),
        row("functions", 1, "skipped", false),
        advisory,
    ];
    assert_eq!(attempt_state(&own, soon()), "passed");
}

/// The runner reported `smoke` and died during `changes`: every check a
/// runner still owes is answered once nobody will write it, the advisory one
/// included, and it decides no state.
#[test]
fn an_advisory_check_a_dead_runner_left_open_reads_error_and_stalls_nothing() {
    let at = queued_at() + Duration::minutes(2);
    let left = claimed(
        vec![
            row("validation", 1, "passed", false),
            row("smoke", 1, "passed", false),
            row("changes", 1, "running", true),
        ],
        at,
    );
    let inside = at + Duration::minutes(30);
    let out = response(&build(), "staging", left.clone(), None, inside);
    let changes = out.checks.iter().find(|c| c.kind == "changes").unwrap();
    assert_eq!((changes.status.as_str(), out.state), ("running", "passed"));

    let out = response(&build(), "staging", left, None, past(at));
    let changes = out.checks.iter().find(|c| c.kind == "changes").unwrap();
    assert_eq!(changes.status, "error");
    let summary = changes.summary.as_deref().unwrap();
    assert!(summary.starts_with("the runner stopped before a verdict"));
    assert_eq!((out.state, out.stalled), ("passed", None));
    assert_eq!(out.attempts[0].state, "passed");
}

/// In a stalled attempt the advisory check reads `error` with the checks
/// that count; in an attempt nothing was started for, `skipped` with them.
#[test]
fn an_advisory_check_is_answered_with_the_others_of_its_attempt() {
    let at = queued_at() + Duration::minutes(2);
    let stalled = claimed(
        vec![
            row("smoke", 1, "running", false),
            row("changes", 1, "running", true),
        ],
        at,
    );
    let out = response(&build(), "staging", stalled, None, past(at));
    assert_eq!(
        (out.state, out.stalled),
        ("failed", Some(Stalled::RunnerStopped))
    );
    assert!(out.checks.iter().all(|c| c.status == "error"));

    let record = json!({ "not_started": "not_configured", "reason": "no token",
        "at": "2026-11-20T10:01:00Z" });
    let mut waiting = vec![
        row("loads", 1, "queued", false),
        row("changes", 1, "queued", true),
    ];
    for row in &mut waiting {
        row.runner = Some(record.clone());
    }
    let out = response(&build(), "staging", waiting, None, past(at));
    assert!(out.checks.iter().all(|c| c.status == "skipped"));
    assert_eq!((out.state, out.stalled), ("none", None));
}
