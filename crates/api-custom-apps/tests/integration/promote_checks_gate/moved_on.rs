//! A build staging moved past before its runner claimed: the checks that
//! need staging were judged by nobody, so they satisfy nothing.

use super::*;
use crate::promote_rollback::{audit_rows, draft, fixture_builds, pointers, promote};
use crate::sandbox_agent_token::fixture::{agent, tarball};

/// What needs staging, of the kinds an app requires by default.
const NEVER_JUDGED: [&str; 3] = ["functions", "loads", "smoke"];

/// One runner route of `build`, as the staff person.
async fn runner_route(agent: &Agent, build: Uuid, route: &str, body: Value) -> Value {
    let uri = format!(
        "/customer-apps/{}/builds/{build}/checks/{route}",
        agent.app.id
    );
    let (status, answer) = person(agent, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{route}: {answer}");
    answer
}

/// Gate on, default `required`. A is published, then B before A's runner
/// claims: the claim closes what needs staging without judging it, and the
/// runner reports the one check that does not. The fixture, the build
/// production serves, and A.
async fn moved_past() -> (Agent, Uuid, Uuid) {
    let agent = agent().await;
    let (live, _) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    let slug = agent.app.slug.clone();
    let a = draft(&agent, "moved-a", tarball(&slug, "moved a")).await;
    let b = draft(&agent, "moved-b", tarball(&slug, "moved b")).await;
    assert_eq!(pointers(&agent).await, (Some(live), Some(b)));

    let by_hand = json!({ "runner": { "label": "by hand" } });
    let claimed = runner_route(&agent, a, "claim", by_hand).await;
    assert_eq!(claimed["serving"], false, "{claimed}");
    assert_eq!(claimed["open"], json!(["design_system"]), "{claimed}");
    let design = json!({ "kind": "design_system", "status": "passed",
        "summary": "follows the design system", "detail": { "findings": [] } });
    let reported = runner_route(&agent, a, "report", json!({ "checks": [design] })).await;
    assert_eq!(
        reported,
        json!({ "state": "none", "open": [] }),
        "nothing judged what needs staging: never `passed`"
    );
    (agent, live, a)
}

/// The attempt never reads `passed`: its roll-up is `none`, and each check
/// nobody judged says so on its row.
#[tokio::test]
async fn a_build_staging_moved_past_before_its_checks_ran_is_not_counted_as_checked() {
    let (agent, _, a) = moved_past().await;
    let uri = format!("/customer-apps/{}/builds/{a}/checks", agent.app.id);
    let (status, shown) = person(&agent, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["state"], "none", "{shown}");
    assert_eq!(shown["attempts"][0]["state"], "none", "{shown}");
    for check in shown["checks"].as_array().expect("the checks") {
        let kind = check["kind"].as_str().unwrap_or_default();
        let closed_unjudged = NEVER_JUDGED.contains(&kind) || kind == "changes";
        let expected = if closed_unjudged { "skipped" } else { "passed" };
        assert_eq!(check["status"], expected, "{kind}: {check}");
        let marked = check["detail"]["not_checked"] == json!(true);
        assert_eq!(marked, closed_unjudged, "{kind}: {check}");
    }

    let plan = json!({ "from": "staging", "build": a, "dry_run": true });
    let (status, plan) = promote(&agent, "production", plan).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(codes(&plan), ["checks_not_passed"], "{plan}");
    assert_eq!(plan["checks"]["state"], "none", "{plan}");
    assert_eq!(plan["checks"]["unmet"], json!(NEVER_JUDGED), "{plan}");
    assert_eq!(plan["checks"]["running"], json!([]), "{plan}");
    let message = plan["blockers"][0]["message"].as_str().unwrap_or_default();
    for said in [
        "Its checks pass (not run)",
        "It loads, desk and phone (not run)",
        "Every screen opens (not run)",
    ] {
        assert!(message.contains(said), "{message}");
    }
    assert!(!message.contains("design system"), "it did pass: {message}");
}

/// Its promote is held as `checks_not_passed`, and only the audited override
/// clears it.
#[tokio::test]
async fn a_build_staging_moved_past_is_promoted_only_with_the_audited_override() {
    let (agent, live, a) = moved_past().await;
    let act = json!({ "from": "staging", "build": a });
    let (status, refused) = promote(&agent, "production", act.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "checks_not_passed", "{refused}");
    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());

    let (status, moved) = promote(&agent, "production", overriding(act, REASON)).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(pointers(&agent).await.0, Some(a));
    let audited = audit_rows(&agent, OVERRIDDEN).await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert_eq!(
        audited[0].metadata["unmet"],
        json!(NEVER_JUDGED),
        "{audited:?}"
    );
}
