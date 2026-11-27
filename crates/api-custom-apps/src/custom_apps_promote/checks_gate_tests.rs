//! The gate's pure half: which rows satisfy a requirement, which leave it
//! unmet or running, what `seen_required` asks, and the plan's shape.

use chrono::{Duration, TimeZone};
use serde_json::json;

use super::*;
use crate::custom_apps_build_checks::stalled::NotStarted;

fn queued_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap()
}

/// A minute after the attempt was queued: nothing has stalled.
fn soon() -> DateTime<Utc> {
    queued_at() + Duration::minutes(1)
}

fn row(kind: &str, status: &str) -> app_build_checks::Model {
    app_build_checks::Model {
        id: Uuid::new_v4(),
        app_id: Uuid::from_u128(1),
        build_id: Uuid::from_u128(10),
        environment: "staging".into(),
        kind: kind.into(),
        attempt: 1,
        status: status.into(),
        advisory: kind == "changes" || kind == "design_audit" || kind == "summary",
        summary: Some(format!("{kind} is {status}")),
        detail: None,
        notes: json!([]),
        run_id: None,
        runner: None,
        created_at: queued_at().fixed_offset(),
        started_at: None,
        finished_at: None,
    }
}

/// A gated attempt with every check passed, and `changes` skipped.
fn all_passed() -> Vec<app_build_checks::Model> {
    let mut rows: Vec<_> = ["validation", "functions", "loads", "design_system", "smoke"]
        .into_iter()
        .map(|kind| row(kind, "passed"))
        .collect();
    rows.push(row("changes", "skipped"));
    rows
}

/// [`all_passed`] with `kind` in `status`.
fn with(kind: &str, status: &str) -> Vec<app_build_checks::Model> {
    let mut rows = all_passed();
    for row in rows.iter_mut().filter(|row| row.kind == kind) {
        row.status = status.into();
    }
    rows
}

fn read(rows: Vec<app_build_checks::Model>) -> Checks {
    evaluate(&Settings::default(), rows, soon())
}

fn codes(checks: &Checks) -> Vec<&'static str> {
    checks.blockers("b-7").iter().map(|b| b.code).collect()
}

/// `passed` and `skipped` satisfy; `queued` and `running` are still open;
/// `failed`, `error` and a status nobody knows leave the check unmet. A kind
/// is never in both lists.
#[test]
fn every_status_of_a_required_check_is_satisfied_open_or_unmet() {
    for kind in ["functions", "loads", "design_system", "smoke"] {
        for (status, unmet, running) in [
            ("passed", false, false),
            ("skipped", false, false),
            ("queued", false, true),
            ("running", false, true),
            ("failed", true, false),
            ("error", true, false),
            ("exploded", true, false),
        ] {
            let checks = read(with(kind, status));
            let expect = |held: bool| if held { vec![kind.to_string()] } else { vec![] };
            assert_eq!(checks.unmet, expect(unmet), "{kind} {status}");
            assert_eq!(checks.running, expect(running), "{kind} {status}");
            assert_eq!(checks.holds(), unmet || running, "{kind} {status}");
        }
    }
    let passed = read(all_passed());
    assert_eq!((passed.state, passed.attempt), ("passed", Some(1)));
    assert!(codes(&passed).is_empty());
}

/// `validation` is not the gate's: its failure is `validation_not_passed`,
/// asked elsewhere, and the row is listed as not required.
#[test]
fn validation_is_listed_and_never_required() {
    let checks = read(with("validation", "failed"));
    assert!(checks.unmet.is_empty() && checks.running.is_empty());
    assert_eq!(checks.state, "failed");
    let listed: Vec<(&str, bool)> = checks
        .rows
        .iter()
        .map(|row| (row.kind.as_str(), row.required))
        .collect();
    assert_eq!(
        listed,
        [
            ("validation", false),
            ("functions", true),
            ("loads", true),
            ("design_system", true),
            ("smoke", true),
            ("changes", false),
        ]
    );
}

/// A check the attempt left open reads `error` once the attempt stalls, as
/// `GET …/checks` answers it: unmet, not running.
#[test]
fn a_stalled_attempts_open_checks_are_unmet_not_running() {
    let rows = with("loads", "queued");
    let fresh = evaluate(&Settings::default(), rows.clone(), soon());
    assert_eq!(fresh.running, ["loads"]);
    assert_eq!(codes(&fresh), [CHECKS_RUNNING]);

    let later = queued_at() + Duration::minutes(11);
    let stalled = evaluate(&Settings::default(), rows, later);
    assert_eq!(stalled.unmet, ["loads"]);
    assert!(stalled.running.is_empty());
    assert_eq!(stalled.state, "failed");
    let loads = stalled.rows.iter().find(|row| row.kind == "loads").unwrap();
    assert_eq!(loads.status, "error");
    let blockers = stalled.blockers("b-7");
    assert_eq!(blockers[0].code, CHECKS_NOT_PASSED);
    assert!(
        blockers[0]
            .message
            .contains("It loads, desk and phone (error)"),
        "{}",
        blockers[0].message
    );
}

/// A build never checked satisfies nothing: no attempt at all, and an attempt
/// nothing was started for, whose rows read `skipped`.
#[test]
fn an_attempt_that_reads_none_leaves_every_required_check_unmet() {
    let required = ["functions", "loads", "design_system", "smoke"];
    let never = read(Vec::new());
    assert_eq!((never.attempt, never.state), (None, "none"));
    assert_eq!(never.unmet, required);
    assert!(never.rows.is_empty() && never.running.is_empty());

    let marker = NotStarted::not_configured("OXY_APP_CHECKS_GITHUB_TOKEN is not set", queued_at());
    let stored = serde_json::to_value(marker).unwrap();
    let mut rows: Vec<_> = required
        .iter()
        .chain(&["validation", "changes"])
        .map(|kind| row(kind, "queued"))
        .collect();
    for row in &mut rows {
        row.runner = Some(stored.clone());
    }
    let long_after = queued_at() + Duration::days(2);
    let not_started = evaluate(&Settings::default(), rows, long_after);
    assert_eq!((not_started.attempt, not_started.state), (Some(1), "none"));
    let required_rows = not_started.rows.iter().filter(|row| row.required);
    assert!(
        required_rows.clone().count() == 4 && required_rows.clone().all(|r| r.status == "skipped"),
        "{:?}",
        not_started.rows
    );
    assert_eq!(not_started.unmet, required, "a skip nobody ran is no pass");
    assert!(not_started.running.is_empty());
    let message = &not_started.blockers("b-7")[0].message;
    assert!(
        message.contains("Every screen opens (not run)"),
        "{message}"
    );
}

/// A kind the attempt was not queued with (the gate came on later) has no row
/// to satisfy the requirement.
#[test]
fn a_required_kind_with_no_row_is_unmet() {
    let phase_one: Vec<_> = ["validation", "functions", "loads", "design_system"]
        .into_iter()
        .map(|kind| row(kind, "passed"))
        .collect();
    let checks = read(phase_one.clone());
    assert_eq!(checks.unmet, ["smoke"]);
    assert_eq!(checks.state, "passed", "the attempt's own roll-up");
    // An app that does not require it is not held by its absence.
    let chosen = Settings {
        required: Some(vec!["functions".into(), "loads".into()]),
        ..Settings::default()
    };
    let checks = evaluate(&chosen, phase_one, soon());
    assert_eq!(checks.required, ["functions", "loads"]);
    assert!(!checks.holds());
}

/// An advisory row decides nothing: not failed, not open, and never the
/// answer of a required kind that shares its name.
#[test]
fn advisory_rows_are_ignored() {
    for status in ["queued", "running", "failed", "error"] {
        let mut rows = with("changes", status);
        rows.push(row("design_audit", status));
        let checks = read(rows);
        assert!(!checks.holds(), "an advisory row in {status} held the move");
        assert_eq!(checks.rows.len(), 7, "it is still listed");
    }
    let mut rows = all_passed();
    for row in rows.iter_mut().filter(|row| row.kind == "smoke") {
        row.advisory = true;
    }
    assert_eq!(read(rows).unmet, ["smoke"]);
}

#[test]
fn only_the_newest_attempt_is_read() {
    let mut rows = with("loads", "failed");
    let mut second = all_passed();
    for row in &mut second {
        row.attempt = 2;
    }
    rows.extend(second);
    let checks = read(rows.clone());
    assert_eq!(checks.attempt, Some(2));
    assert!(!checks.holds());
    assert_eq!(checks.rows.len(), 6);
    // And the other way round: a pass an older attempt gave does not count.
    for row in &mut rows {
        row.attempt = 3 - row.attempt;
    }
    assert_eq!(read(rows).unmet, ["loads"]);
}

fn changed(status: &str, changed: u64, seen: bool) -> Vec<app_build_checks::Model> {
    let mut rows = with("changes", status);
    for row in rows.iter_mut().filter(|row| row.kind == "changes") {
        let seen = seen.then(|| json!({ "by": "ana@oxy.tech", "at": "2026-11-22T10:05:00Z" }));
        row.detail = Some(json!({
            "findings": [], "compared": 28, "changed": changed, "seen": seen,
        }));
    }
    rows
}

/// `seen_required` holds a promote while changed screens are unseen, while
/// the comparison still runs, and when it produced nothing to look at.
#[test]
fn seen_required_asks_a_person_to_have_seen_what_changed() {
    let asking = Settings {
        seen_required: true,
        ..Settings::default()
    };
    let held = |rows| codes(&evaluate(&asking, rows, soon()));
    assert_eq!(held(changed("passed", 4, false)), [CHANGES_NOT_SEEN]);
    assert!(held(changed("passed", 4, true)).is_empty(), "seen");
    assert!(
        held(changed("passed", 0, false)).is_empty(),
        "nothing changed"
    );
    assert!(held(all_passed()).is_empty(), "nothing to compare with");
    assert_eq!(held(with("changes", "running")), [CHECKS_RUNNING]);
    assert_eq!(held(with("changes", "error")), [CHANGES_NOT_SEEN]);
    // Left open by a runner that stopped: nothing to wait for, nothing to see.
    let later = queued_at() + Duration::minutes(11);
    let abandoned = evaluate(&asking, with("changes", "running"), later);
    assert!(abandoned.running.is_empty(), "{:?}", abandoned.running);
    assert_eq!(codes(&abandoned), [CHANGES_NOT_SEEN]);
    assert_eq!(
        abandoned.state, "passed",
        "an advisory row stalls no attempt"
    );
    assert_eq!(
        held(Vec::new()),
        [CHECKS_NOT_PASSED, CHANGES_NOT_SEEN],
        "never checked"
    );

    let unseen = evaluate(&asking, changed("passed", 4, false), soon());
    assert_eq!(
        serde_json::to_value(&unseen.changes).unwrap(),
        json!({ "changed": 4, "compared": 28, "seen": false, "seen_required": true })
    );
    let message = &unseen.blockers("b-7")[0].message;
    assert!(message.contains("4 of 28 screens"), "{message}");
    let running = evaluate(&asking, with("changes", "running"), soon());
    assert_eq!(running.running, ["changes"]);

    // Without the setting the same rows inform and hold nothing.
    for rows in [changed("passed", 4, false), with("changes", "error")] {
        assert!(!read(rows).holds());
    }
    assert!(!read(changed("passed", 4, false)).changes.seen_required);
}

/// The three blockers come in one order and name the checks by label.
#[test]
fn the_blockers_come_in_order_and_name_the_checks_by_label() {
    let asking = Settings {
        seen_required: true,
        ..Settings::default()
    };
    let mut rows = changed("passed", 2, false);
    for row in &mut rows {
        match row.kind.as_str() {
            "design_system" => row.status = "failed".into(),
            "loads" => row.status = "running".into(),
            _ => {}
        }
    }
    let checks = evaluate(&asking, rows, soon());
    let blockers = checks.blockers("b-7");
    let listed: Vec<&str> = blockers.iter().map(|b| b.code).collect();
    assert_eq!(
        listed,
        [CHECKS_NOT_PASSED, CHECKS_RUNNING, CHANGES_NOT_SEEN]
    );
    assert!(
        blockers[0]
            .message
            .contains("Follows the design system (failed)"),
        "{}",
        blockers[0].message
    );
    assert!(
        blockers[1]
            .message
            .contains("It loads, desk and phone (running)"),
        "{}",
        blockers[1].message
    );
    for blocker in &blockers {
        assert!(blocker.message.contains("\"b-7\""), "{}", blocker.message);
        assert!(blocker.message.contains("override"), "{}", blocker.message);
    }
}

/// The plan's `checks`, as the contract writes it.
#[test]
fn the_plans_checks_read_as_the_contract_writes_them() {
    let note = json!({
        "author": "model", "model": "m-1", "text": "Changes the stores screen.",
        "refs": [], "created_at": "2026-11-22T10:06:00Z",
    });
    let mut rows = changed("passed", 4, false);
    for row in &mut rows {
        row.attempt = 2;
        if row.kind == "design_system" {
            row.status = "failed".into();
            row.summary = Some("2 findings".into());
        }
    }
    let mut summary = row("summary", "passed");
    summary.attempt = 2;
    summary.notes = json!([note]);
    rows.push(summary);
    let mut checks = read(rows);
    checks.overridable = true;
    let shown = serde_json::to_value(&checks).unwrap();
    assert_eq!(
        shown["required"],
        json!(["functions", "loads", "design_system", "smoke"])
    );
    assert_eq!(shown["attempt"], 2);
    assert_eq!(shown["state"], "failed");
    assert_eq!(
        shown["rows"][3],
        json!({
            "kind": "design_system", "label": "Follows the design system",
            "status": "failed", "summary": "2 findings", "required": true,
        })
    );
    assert_eq!(shown["rows"].as_array().unwrap().len(), 6, "no summary row");
    assert_eq!(shown["unmet"], json!(["design_system"]));
    assert_eq!(shown["running"], json!([]));
    assert_eq!(
        shown["changes"],
        json!({ "changed": 4, "compared": 28, "seen": false, "seen_required": false })
    );
    assert_eq!(shown["summary"], note);
    assert_eq!(shown["overridable"], true);
    assert_eq!(shown["overridden"], json!(null));
    let mut keys: Vec<&str> = shown
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "attempt",
            "changes",
            "overridable",
            "overridden",
            "required",
            "rows",
            "running",
            "state",
            "summary",
            "unmet",
        ]
    );

    // A summary row that carries its note as its detail reads the same; one
    // with neither is no summary.
    let mut as_detail = row("summary", "passed");
    as_detail.detail = Some(note.clone());
    assert_eq!(summary_note(&as_detail), Some(note));
    assert_eq!(summary_note(&row("summary", "skipped")), None);
    assert_eq!(read(all_passed()).summary, None);
}

#[path = "checks_gate_not_checked_tests.rs"]
mod not_checked;
