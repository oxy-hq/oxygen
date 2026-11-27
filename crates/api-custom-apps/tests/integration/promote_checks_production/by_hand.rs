//! A run by hand leaves a trace: the platform names its runner from who
//! called, never from what the caller sent, and its report is audited. The
//! platform checker's run is named as it names itself and writes no row.

use super::*;
use crate::promote_rollback::audit_rows;

/// The audit action of verdicts reported by anyone but the platform checker.
const REPORTED_BY_HAND: &str = "app.build_checks_reported_by_hand";

/// Every runner check of a staging attempt, reported as passed.
fn all_passed() -> Value {
    json!({ "attempt": 1, "environment": "staging", "checks": [
        passed("loads", json!({})),
        passed("design_system", json!({})),
        passed("smoke", json!({ "screens": [] })),
        passed("changes", changes_detail(json!([]))),
    ] })
}

/// The `runner` of `build`'s newest staging attempt, as the console reads it.
async fn runner_shown(agent: &Agent, build: Uuid) -> Value {
    let (status, shown) = person(agent, "GET", &checks(agent, build), None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    shown["runner"].clone()
}

/// A staff person claims with the label and link of a GitHub Actions run and
/// reports every required check as passed.
#[tokio::test]
async fn a_hand_run_is_named_by_the_platform_and_its_report_is_audited() {
    let agent = gated().await;
    let built = draft(&agent, "hand-1", None).await;
    let base = checks(&agent, built);
    let dressed_up = Some(claim_body("staging"));
    let (status, claimed) = person(&agent, "POST", &format!("{base}/claim"), dressed_up).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let who = claimed["identity"].as_str().expect("the person's label");
    assert_ne!(who, "app_checker");

    let runner = runner_shown(&agent, built).await;
    assert_eq!(runner["label"], format!("{who}, by hand"), "{runner}");
    assert_eq!(runner["url"], json!(null), "no link the caller chose");
    assert_eq!(runner["by"], who, "{runner}");

    let report = format!("{base}/report");
    let (status, reported) = person(&agent, "POST", &report, Some(all_passed())).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "passed", "open": [] }));
    let audited = audit_rows(&agent, REPORTED_BY_HAND).await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    let row = &audited[0];
    assert_eq!(row.actor_user_id, Some(agent.t.guest_id));
    assert_eq!(row.actor_email, who);
    assert_eq!(row.org_id, Some(agent.app.org_id));
    assert_eq!(row.workspace_id, Some(agent.app.project_id));
    assert_eq!(row.target_type.as_deref(), Some("custom_app"));
    assert_eq!(row.target_id, Some(agent.app.id.to_string()));
    assert_eq!(
        row.metadata,
        json!({
            "build": "hand-1", "build_pk": built, "environment": "staging", "attempt": 1,
            "checks": [
                { "kind": "loads", "status": "passed" },
                { "kind": "design_system", "status": "passed" },
                { "kind": "smoke", "status": "passed" },
                { "kind": "changes", "status": "passed" },
            ],
        })
    );
    // The same report again writes no verdict, and no second row.
    let (status, _) = person(&agent, "POST", &report, Some(all_passed())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audit_rows(&agent, REPORTED_BY_HAND).await.len(), 1);
}

/// The platform checker's run keeps the name and link it sends, and its
/// report writes no audit row: it is the platform's own.
#[tokio::test]
async fn the_checkers_run_is_named_as_it_names_itself_and_is_not_audited() {
    let agent = gated().await;
    let built = draft(&agent, "hand-2", None).await;
    let base = checks(&agent, built);
    let claim = Some(claim_body("staging"));
    let (status, claimed) = checker("POST", &format!("{base}/claim"), claim).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["identity"], "app_checker");
    let runner = runner_shown(&agent, built).await;
    assert_eq!(runner["label"], "GitHub Actions run #1482", "{runner}");
    assert_eq!(
        runner["url"],
        "https://github.com/oxy-hq/oxygen-internal/actions/runs/123"
    );
    assert!(runner.get("by").is_none(), "{runner}");
    let (status, reported) = checker("POST", &format!("{base}/report"), Some(all_passed())).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert!(audit_rows(&agent, REPORTED_BY_HAND).await.is_empty());
}
