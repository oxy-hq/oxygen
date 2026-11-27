//! The writers of production that never read a plan, and the rollback
//! exemption. The fixture's staging build (`agent-stg`) was queued before the
//! gate came on: its attempt has phase 1's four kinds, still open, and no
//! `smoke` row, so it is unmet (`checks_not_passed`) and running at once.

use super::*;
use crate::promote_rollback::{
    audit_rows, fixture_builds, pointers, promote, rollback, sandbox_build,
};
use crate::sandbox_agent_token::fixture::{agent, tarball};

/// The console's rollback (`POST …/rollback`) to `build`.
async fn console_rollback(agent: &Agent, body: Value) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    person(agent, "POST", &uri, Some(body)).await
}

/// `POST /customer-apps/batch/{action}` for the fixture's app: its one row.
async fn batch(agent: &Agent, action: &str, extra: Value) -> Value {
    let uri = format!("/customer-apps/batch/{action}");
    let mut body = json!({ "ids": [agent.app.id] });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    let (status, answered) = person(agent, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{action}: {answered}");
    answered["results"][0].clone()
}

#[tokio::test]
async fn every_writer_that_skips_the_plan_is_held_and_overrides_per_app() {
    let agent = agent().await;
    let (live, staged) = fixture_builds(&agent).await;
    enable_gate(&agent).await;

    // Make Live, the console's rollback to a build production never served,
    // batch publish and promote latest: the same refusal, and nothing moves.
    let (status, refused) = make_live(&agent, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "checks_not_passed", "{refused}");
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Every screen opens (not run)"),
        "{message}"
    );
    assert!(message.contains("still running"), "{message}");
    let (status, refused) = console_rollback(&agent, json!({ "build_id": staged })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "checks_not_passed", "{refused}");
    for action in ["publish", "promote-latest"] {
        let row = batch(&agent, action, json!({})).await;
        assert_eq!(row["ok"], false, "{action}: {row}");
        assert_eq!(row["code"], "checks_not_passed", "{action}: {row}");
    }
    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());

    // A build staging never served has no staging attempt either. It is
    // refused for invariant 1 first, and an override clears only the checks.
    let slug = agent.app.slug.clone();
    sandbox_build(&agent, "q1", "sbx-gate", tarball(&slug, "sandboxed"), true).await;
    let overriding_all = json!({ "override_checks": { "reason": REASON } });
    let row = batch(&agent, "promote-latest", overriding_all.clone()).await;
    assert_eq!(row["code"], "staging_never_served", "{row}");
    assert_eq!(pointers(&agent).await.0, Some(live), "production moved");
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());

    // Batch publish with the override: the draft goes live, audited per app.
    let row = batch(&agent, "publish", overriding_all).await;
    assert_eq!(row["ok"], true, "{row}");
    assert_eq!(pointers(&agent).await.0, Some(staged));
    let audited = audit_rows(&agent, OVERRIDDEN).await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    let metadata = &audited[0].metadata;
    assert_eq!(metadata["build"], json!(staged), "{metadata}");
    assert_eq!(metadata["reason"], REASON, "{metadata}");
    assert_eq!(metadata["unmet"], json!(["smoke"]), "{metadata}");
    assert_eq!(
        metadata["running"],
        json!(["functions", "loads", "design_system"]),
        "{metadata}"
    );
    assert_eq!(metadata["changes_not_seen"], false, "{metadata}");
}

/// A rollback is the emergency path: to a build production served before, it
/// is never held, by `oxyc rollback`'s route or the console's. A promote of
/// that same build is still a promote.
#[tokio::test]
async fn a_rollback_to_a_build_production_served_is_never_held() {
    let agent = agent().await;
    let (live, staged) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    let acted = overriding(json!({ "from": "staging" }), REASON);
    let (status, plan) = promote(&agent, "production", acted).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(staged));

    // Back to the build production served first, whose checks never passed.
    let (status, plan) = rollback(&agent, "production", json!({ "dry_run": true })).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert!(plan["checks"].is_null(), "{plan}");
    assert!(codes(&plan).is_empty(), "{plan}");
    let (status, plan) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert!(plan["checks"].is_null(), "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(live));

    // Promoting the staged build again is a promote, and held like one.
    let (status, refused) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "checks_not_passed", "{refused}");
    // The console's rollback to it is a rollback: production served it.
    let (status, back) = console_rollback(&agent, json!({ "build_id": staged })).await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(pointers(&agent).await.0, Some(staged));
    assert_eq!(
        audit_rows(&agent, OVERRIDDEN).await.len(),
        1,
        "the promote's"
    );
}
