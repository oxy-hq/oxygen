use super::*;

#[test]
fn kinds_and_statuses_round_trip_their_wire_names() {
    for kind in CheckKind::PHASE_ONE {
        assert_eq!(CheckKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(CheckKind::order_of("loads"), 2);
    assert_eq!(CheckKind::order_of("design_audit"), 4);
    assert_eq!(CheckStatus::parse("skipped"), Some(CheckStatus::Skipped));
    assert!(!CheckStatus::Running.is_terminal());
    assert!(CheckStatus::Error.is_terminal());
}

/// The gate's two kinds: on the wire, in a build's list after phase 1's
/// and before any later kind, and `changes` alone advisory.
#[test]
fn the_gates_kinds_follow_phase_ones_and_only_changes_is_advisory() {
    let gated: Vec<&str> = CheckKind::of_staging_attempt(true)
        .into_iter()
        .map(CheckKind::as_str)
        .collect();
    assert_eq!(
        gated,
        [
            "validation",
            "functions",
            "loads",
            "design_system",
            "smoke",
            "changes"
        ]
    );
    let plain = CheckKind::of_staging_attempt(false);
    assert_eq!(plain, CheckKind::PHASE_ONE.to_vec());
    for kind in CheckKind::of_staging_attempt(true) {
        assert_eq!(CheckKind::parse(kind.as_str()), Some(kind));
        assert_eq!(kind.advisory(), kind == CheckKind::Changes, "{kind:?}");
    }
    assert_eq!(CheckKind::Smoke.label(), "Every screen opens");
    assert_eq!(CheckKind::Changes.label(), "What changed");
    assert_eq!(CheckKind::label_of("design_audit"), "Design audit");
    assert_eq!(CheckKind::label_of("later_kind"), "later kind");
    assert_eq!(CheckKind::parse("design_audit"), None);

    let mut listed = vec![
        "summary",
        "design_audit",
        "changes",
        "production_smoke",
        "smoke",
        "design_system",
        "validation",
        "loads",
        "functions",
    ];
    listed.sort_by(|a, b| CheckKind::list_key(a).cmp(&CheckKind::list_key(b)));
    assert_eq!(
        listed,
        [
            "validation",
            "functions",
            "loads",
            "design_system",
            "smoke",
            "changes",
            "design_audit",
            "production_smoke",
            "summary",
        ]
    );
}

/// A production attempt has one kind, which is no staging kind, never
/// advisory, and listed after the staging ones.
#[test]
fn a_production_attempt_has_production_smoke_alone() {
    assert_eq!(CheckKind::PRODUCTION, [CheckKind::ProductionSmoke]);
    let smoke = CheckKind::ProductionSmoke;
    assert_eq!(smoke.as_str(), "production_smoke");
    assert_eq!(smoke.label(), "Production opens after promote");
    assert!(!smoke.advisory());
    assert_eq!(CheckKind::parse("production_smoke"), Some(smoke));
    assert_eq!(
        CheckKind::label_of("production_smoke"),
        "Production opens after promote"
    );
    assert!(!CheckKind::of_staging_attempt(true).contains(&smoke));
    assert!(CheckKind::list_key("production_smoke") > CheckKind::list_key("changes"));
}

#[test]
fn the_models_rows_are_the_design_audit_and_the_summary() {
    assert!(is_models("design_audit") && is_models("summary"));
    for kind in CheckKind::of_staging_attempt(true)
        .into_iter()
        .chain(CheckKind::PRODUCTION)
    {
        assert!(!is_models(kind.as_str()), "{kind:?}");
    }
    assert_eq!(
        MODEL_KINDS[1],
        crate::custom_apps_build_checks::dto::SUMMARY_KIND
    );
}

#[test]
fn an_outcome_caps_its_findings_and_says_how_many_it_left_out() {
    let many: Vec<Finding> = (0..MAX_FINDINGS + 3)
        .map(|i| Finding::new(Severity::Warning, format!("f{i}")))
        .collect();
    let out = Outcome::with(CheckStatus::Failed, "x", &many, json!({"url": "u"}));
    assert_eq!(
        out.detail["findings"].as_array().unwrap().len(),
        MAX_FINDINGS
    );
    assert_eq!(out.detail["findings_omitted"], 3);
    assert_eq!(out.detail["url"], "u");
    let long = Finding::new(Severity::Error, "e".repeat(1000));
    assert_eq!(long.message.chars().count(), MAX_MESSAGE_CHARS);
    assert!(long.message.ends_with('…'));
}
