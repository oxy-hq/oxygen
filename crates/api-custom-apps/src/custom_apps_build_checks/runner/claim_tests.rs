use super::*;

fn named(label: &str, url: Option<&str>) -> Result<(String, Option<String>), ChecksError> {
    runner_named(&RunnerRequest {
        label: label.into(),
        url: url.map(str::to_string),
    })
}

#[test]
fn a_runner_has_a_label_and_an_http_link_or_none() {
    let run = "https://github.com/oxy-hq/oxygen-internal/actions/runs/123";
    assert_eq!(
        named(" GitHub Actions run #1482 ", Some(run)).unwrap(),
        (
            "GitHub Actions run #1482".to_string(),
            Some(run.to_string())
        )
    );
    assert_eq!(named("by hand", None).unwrap().1, None);
    assert_eq!(named("by hand", Some("  ")).unwrap().1, None);
    let long = named(&"x".repeat(500), None).unwrap().0;
    assert_eq!(long.chars().count(), MAX_LABEL_CHARS);
    for (label, url) in [
        ("", None),
        ("   ", None),
        ("run", Some("javascript:alert(1)")),
        ("run", Some("not a url")),
        ("run", Some("ftp://example.com/x")),
    ] {
        let refused = named(label, url).unwrap_err();
        assert_eq!(refused.code(), Some("invalid_runner"), "{label:?} {url:?}");
    }
    let too_long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
    assert!(named("run", Some(&too_long)).is_err());
}

#[test]
fn the_apps_url_is_its_path_url_on_the_origin() {
    assert_eq!(
        app_url("https://app.example.com", "acme", "store"),
        "https://app.example.com/customer-apps/acme/store/"
    );
    assert_eq!(
        app_url("http://127.0.0.1:3100/", "acme", "store"),
        "http://127.0.0.1:3100/customer-apps/acme/store/"
    );
}

/// Only the platform checker names its own runner. Anyone else's claim is a
/// run by hand, named from who called: the label and link they sent are
/// dropped, so it never reads as a GitHub Actions run.
#[test]
fn only_the_checker_names_its_runner_and_a_run_by_hand_is_named_by_the_platform() {
    let at = Utc::now();
    let run = "https://github.com/oxy-hq/oxygen-internal/actions/runs/123";
    let sent = || {
        (
            "GitHub Actions run #1482".to_string(),
            Some(run.to_string()),
        )
    };
    let claimant = |identity, checker| Claimant {
        identity,
        checker,
        origin: "https://app.example.com",
    };
    let checkers = claimant("app_checker", true).runner(sent(), at);
    assert_eq!(
        serde_json::to_value(&checkers).unwrap(),
        serde_json::json!({ "label": "GitHub Actions run #1482", "url": run,
            "started_at": at.fixed_offset() }),
        "as it names itself, and no `by`"
    );
    let by_hand = claimant("ana@oxy.tech", false).runner(sent(), at);
    assert_eq!(
        serde_json::to_value(&by_hand).unwrap(),
        serde_json::json!({ "label": "ana@oxy.tech, by hand", "url": null,
            "started_at": at.fixed_offset(), "by": "ana@oxy.tech" })
    );
    // A long name is cut, and the label still says it was by hand.
    let long = claimant(&"n".repeat(400), false).runner(sent(), at);
    assert_eq!(long.label.chars().count(), MAX_LABEL_CHARS);
    assert!(long.label.ends_with(", by hand"));
    // A row written before `by` existed reads as a runner with none.
    let stored = serde_json::json!({ "label": "run #1", "url": null, "started_at": at });
    let old: Runner = serde_json::from_value(stored).unwrap();
    assert_eq!(old.by, None);
}
