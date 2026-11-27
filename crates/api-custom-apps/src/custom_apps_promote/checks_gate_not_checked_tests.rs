//! The gate over checks nobody judged: a row the platform closed without a
//! verdict of its own (`detail.not_checked`) satisfies nothing, while a skip
//! the check decided itself still does.

use super::*;

/// [`all_passed`] with `kinds` closed as a claim closes what needs staging
/// once staging has moved on.
fn moved_on(kinds: &[&str]) -> Vec<app_build_checks::Model> {
    let mut rows = all_passed();
    for row in rows
        .iter_mut()
        .filter(|row| kinds.contains(&row.kind.as_str()))
    {
        row.status = "skipped".into();
        row.summary = Some("staging no longer serves build b-7".into());
        row.detail = Some(json!({ "findings": [], "not_checked": true }));
    }
    rows
}

#[test]
fn a_required_check_nobody_judged_is_unmet_and_named_as_not_run() {
    let needs_staging = ["functions", "loads", "smoke", "changes"];
    let checks = read(moved_on(&needs_staging));
    assert_eq!(checks.unmet, ["functions", "loads", "smoke"]);
    assert!(checks.running.is_empty());
    assert_eq!(checks.state, "none", "never `passed`");
    assert_eq!(codes(&checks), [CHECKS_NOT_PASSED]);
    let message = &checks.blockers("b-7")[0].message;
    for said in [
        "Its checks pass (not run)",
        "It loads, desk and phone (not run)",
        "Every screen opens (not run)",
    ] {
        assert!(message.contains(said), "{message}");
    }
    assert!(!message.contains("design system"), "it passed: {message}");
    // The plan still lists the row as the wire has it.
    let loads = checks.rows.iter().find(|row| row.kind == "loads").unwrap();
    assert_eq!((loads.status.as_str(), loads.required), ("skipped", true));

    // An app that requires none of them is not held by them.
    let chosen = Settings {
        required: Some(vec!["design_system".into()]),
        ..Settings::default()
    };
    assert!(!evaluate(&chosen, moved_on(&needs_staging), soon()).holds());
}

/// A skip the check decided itself (no check functions, no recorded SDK
/// version) satisfies the requirement, as it always did.
#[test]
fn a_skip_the_check_decided_itself_still_satisfies() {
    let mut rows = all_passed();
    for row in &mut rows {
        if row.kind == "functions" || row.kind == "design_system" {
            row.status = "skipped".into();
            row.detail = Some(json!({ "findings": [] }));
        }
    }
    let checks = read(rows);
    assert!(!checks.holds(), "{:?}", checks.unmet);
    assert_eq!(checks.state, "passed");
}

/// With `seen_required`, a comparison nobody ran has nothing a person can
/// have seen.
#[test]
fn a_comparison_nobody_ran_cannot_have_been_seen() {
    let asking = Settings {
        required: Some(Vec::new()),
        seen_required: true,
        ..Settings::default()
    };
    let checks = evaluate(&asking, moved_on(&["changes"]), soon());
    assert_eq!(codes(&checks), [CHANGES_NOT_SEEN]);
    let message = &checks.blockers("b-7")[0].message;
    assert!(message.contains("(not run)"), "{message}");
}

/// An attempt nothing was started for, with `seen_required`: its comparison
/// was never run, so there is nothing to wait for (`checks_running` would
/// never clear) and nothing a person can have seen.
#[test]
fn the_comparison_of_an_attempt_nothing_was_started_for_is_not_run_not_running() {
    let asking = Settings {
        seen_required: true,
        ..Settings::default()
    };
    let marker = NotStarted::not_configured("OXY_APP_CHECKS_GITHUB_TOKEN is not set", queued_at());
    let stored = serde_json::to_value(marker).unwrap();
    let mut rows = all_passed();
    for row in &mut rows {
        row.status = "queued".into();
        row.runner = Some(stored.clone());
    }
    let checks = evaluate(&asking, rows, queued_at() + Duration::days(2));
    assert!(checks.running.is_empty(), "{:?}", checks.running);
    assert_eq!(codes(&checks), [CHECKS_NOT_PASSED, CHANGES_NOT_SEEN]);
    let message = &checks.blockers("b-7")[1].message;
    assert!(message.contains("(not run)"), "{message}");
}
