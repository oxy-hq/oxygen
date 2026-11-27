use super::*;
use crate::custom_apps_build_checks::kind::MAX_MESSAGE_CHARS;

fn reported(kind: &str, status: &str, detail: Value) -> ReportedCheck {
    ReportedCheck {
        kind: kind.into(),
        status: status.into(),
        summary: Some("phone never became ready".into()),
        detail: Some(detail),
    }
}

fn finding(severity: &str, message: &str) -> Value {
    json!({ "severity": severity, "message": message, "at": null, "rule": null })
}

fn code(refused: Result<(CheckKind, Outcome), ChecksError>) -> Option<&'static str> {
    refused.expect_err("a refusal").code()
}

#[test]
fn a_runners_verdict_is_stored_with_its_own_evidence() {
    let detail = json!({
        "findings": [{
            "severity": "error",
            "message": "restates --card",
            "at": "assets/index-abc.css:1",
            "rule": "restates-sdk-token",
        }],
        "sdk_version": "2.31.0",
        "sdk_floor": "2.30.0",
        "files": ["index.html", "assets/index-abc.css"],
    });
    let (kind, outcome) = sanitized(&reported("design_system", "failed", detail.clone())).unwrap();
    assert_eq!(kind, CheckKind::DesignSystem);
    assert_eq!(outcome.status, CheckStatus::Failed);
    assert_eq!(outcome.summary, "phone never became ready");
    assert_eq!(
        outcome.detail, detail,
        "nothing is added to a detail in bounds"
    );

    // Keys the platform does not know are the runner's evidence, and kept.
    let viewports = json!({ "findings": [], "identity": "app_checker", "viewports": [{
        "name": "desk", "ready": true, "page_errors": [], "failed_requests": [],
        "refused_writes": ["POST /customer-apps/acme/store/fn/save"],
    }] });
    let (kind, outcome) = sanitized(&reported("loads", "passed", viewports.clone())).unwrap();
    assert_eq!(outcome.detail, viewports);
    assert_eq!(
        (kind, outcome.status),
        (CheckKind::Loads, CheckStatus::Passed)
    );
    assert_eq!(outcome.detail["viewports"][0]["name"], "desk");
    for status in ["error", "skipped"] {
        let ok = sanitized(&reported("loads", status, json!({ "findings": [] })));
        assert_eq!(ok.unwrap().1.status.as_str(), status);
    }
}

#[test]
fn only_the_runners_kinds_and_a_verdict_are_taken() {
    let empty = || json!({ "findings": [] });
    for kind in ["validation", "functions", "summary", "design_audit", ""] {
        assert_eq!(
            code(sanitized(&reported(kind, "passed", empty()))),
            Some("kind_not_reportable"),
            "{kind:?}"
        );
    }
    for status in ["queued", "running", "ok", ""] {
        assert_eq!(
            code(sanitized(&reported("loads", status, empty()))),
            Some("invalid_status"),
            "{status:?}"
        );
    }
}

/// The promote checks gate's two kinds, and a production attempt's one, are
/// the runner's to report too, with their evidence kept as sent; a model's
/// kinds are not.
#[test]
fn the_gates_kinds_and_productions_are_the_runners_to_report() {
    let screens = json!({ "findings": [], "discovered": true, "screens": [{ "path": "stores" }] });
    let (kind, outcome) = sanitized(&reported("smoke", "failed", screens.clone())).unwrap();
    assert_eq!(
        (kind, outcome.status),
        (CheckKind::Smoke, CheckStatus::Failed)
    );
    assert_eq!(outcome.detail, screens);
    let differ = json!({ "findings": [], "compared": 28, "changed": 4, "seen": null });
    let (kind, outcome) = sanitized(&reported("changes", "passed", differ.clone())).unwrap();
    assert_eq!(
        (kind, outcome.status),
        (CheckKind::Changes, CheckStatus::Passed)
    );
    assert_eq!(outcome.detail, differ);
    let opened = json!({ "findings": [], "baseline_at": null, "discovered": true,
        "screens_omitted": 0, "screens": [{ "path": "stores", "viewports": [] }],
        "new_errors": [{ "path": "stores", "viewport": "desk", "error": "TypeError: x" }],
        "new_errors_omitted": 2, "served_build_id": "b", "lists_capped_at": 5,
        "screens_unvisited": 1 });
    let (kind, outcome) =
        sanitized(&reported("production_smoke", "failed", opened.clone())).unwrap();
    assert_eq!(
        (kind, outcome.status),
        (CheckKind::ProductionSmoke, CheckStatus::Failed)
    );
    assert_eq!(
        outcome.detail, opened,
        "every key the runner writes is kept"
    );
    for models in ["design_audit", "summary"] {
        let refused = sanitized(&reported(models, "passed", json!({ "findings": [] })));
        assert_eq!(code(refused), Some("kind_not_reportable"), "{models}");
    }
}

/// What the platform writes into a detail after the verdict is never taken
/// from a report: a runner cannot mark the changed screens seen, nor say a
/// rollback happened.
#[test]
fn a_report_cannot_set_what_the_platform_writes_after_the_verdict() {
    let forged = json!({ "findings": [], "compared": 28, "changed": 4,
        "seen": { "by": "ana@oxy.tech", "at": "2026-11-22T10:00:00Z" },
        "move": { "by": null }, "rollback": { "rolled_back": true } });
    let (_, outcome) = sanitized(&reported("changes", "passed", forged)).unwrap();
    assert_eq!(
        outcome.detail,
        json!({ "findings": [], "compared": 28, "changed": 4, "seen": null })
    );
    // A `changes` report that names no `seen` gets the `null` it starts with.
    let bare = json!({ "findings": [], "compared": 1, "changed": 0 });
    let (_, outcome) = sanitized(&reported("changes", "passed", bare)).unwrap();
    assert!(outcome.detail["seen"].is_null() && outcome.detail.get("seen").is_some());

    let claimed = json!({ "findings": [], "new_errors": [],
        "move": { "previous": { "build_id": "x" } },
        "rollback": { "rolled_back": true, "to": { "build_id": "x" } },
        "seen": { "by": "someone" } });
    let (_, outcome) = sanitized(&reported("production_smoke", "failed", claimed)).unwrap();
    assert_eq!(outcome.detail, json!({ "findings": [], "new_errors": [] }));

    // That nobody judged a check is a claim's to say, never a report's.
    let unjudged = json!({ "findings": [], "not_checked": true });
    let (_, outcome) = sanitized(&reported("loads", "skipped", unjudged)).unwrap();
    assert_eq!(outcome.detail, json!({ "findings": [] }));
}

#[test]
fn a_detail_is_an_object_with_a_findings_array_of_findings() {
    let mut no_detail = reported("loads", "passed", json!(null));
    no_detail.detail = None;
    assert_eq!(code(sanitized(&no_detail)), Some("invalid_detail"));
    for detail in [
        json!(null),
        json!([]),
        json!("text"),
        json!({}),
        json!({ "findings": {} }),
        json!({ "findings": ["a string"] }),
        json!({ "findings": [{ "severity": "fatal", "message": "x" }] }),
        json!({ "findings": [{ "severity": "error" }] }),
        json!({ "findings": [{ "severity": "error", "message": 7 }] }),
    ] {
        assert_eq!(
            code(sanitized(&reported("loads", "failed", detail.clone()))),
            Some("invalid_detail"),
            "{detail}"
        );
    }
}

#[test]
fn findings_are_cut_to_fifty_and_their_texts_capped() {
    let many: Vec<Value> = (0..MAX_FINDINGS + 10)
        .map(|n| finding("warning", &format!("f{n}")))
        .collect();
    let (_, cut) = sanitized(&reported("loads", "failed", json!({ "findings": many }))).unwrap();
    let kept = cut.detail["findings"].as_array().unwrap();
    assert_eq!(kept.len(), MAX_FINDINGS);
    assert_eq!(kept[MAX_FINDINGS - 1]["message"], "f49", "the first fifty");
    assert_eq!(cut.detail["findings_omitted"], 10);

    // What the runner left out itself is counted with what is cut here.
    let counted = json!({ "findings": many, "findings_omitted": 5 });
    let (_, both) = sanitized(&reported("loads", "failed", counted)).unwrap();
    assert_eq!(both.detail["findings_omitted"], 15);

    let long = json!({ "findings": [{
        "severity": "error",
        "message": "m".repeat(1000),
        "at": "a".repeat(1000),
        "rule": "r".repeat(1000),
        "body": "<html>a whole page</html>",
    }] });
    let mut check = reported("design_system", "failed", long);
    check.summary = Some("s".repeat(1000));
    let (_, capped) = sanitized(&check).unwrap();
    let only = &capped.detail["findings"][0];
    let chars = |name: &str| only[name].as_str().unwrap().chars().count();
    assert_eq!(chars("message"), MAX_MESSAGE_CHARS);
    assert_eq!(chars("at"), MAX_MESSAGE_CHARS);
    assert_eq!(chars("rule"), 100);
    assert!(
        only.get("body").is_none(),
        "a finding is rebuilt, not copied"
    );
    assert_eq!(capped.summary.chars().count(), 200);
    assert!(capped.summary.ends_with('…'));
}

#[test]
fn a_detail_past_the_bound_is_refused_whole() {
    let page = "x".repeat(MAX_DETAIL_BYTES);
    let huge = json!({ "findings": [], "viewports": [{ "name": "desk", "html": page }] });
    assert_eq!(
        code(sanitized(&reported("loads", "failed", huge))),
        Some("detail_too_large")
    );
    let inside = json!({ "findings": [], "note": "x".repeat(MAX_DETAIL_BYTES - 64) });
    assert!(sanitized(&reported("loads", "failed", inside)).is_ok());
}

#[test]
fn a_report_reads_as_the_contract_writes_it() {
    let body = json!({
        "attempt": 2,
        "checks": [{
            "kind": "loads",
            "status": "failed",
            "summary": "phone never became ready",
            "detail": { "findings": [] },
        }],
    });
    let request: ReportRequest = serde_json::from_value(body).unwrap();
    assert_eq!(request.attempt, Some(2));
    assert_eq!(request.checks.len(), 1);
    let newest: ReportRequest = serde_json::from_value(json!({ "attempt": null })).unwrap();
    assert_eq!((newest.attempt, newest.checks.len()), (None, 0));
}
