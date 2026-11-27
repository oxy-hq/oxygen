//! The flag, the kinds an attempt is queued with, and the three blockers on
//! the promote route and on Make Live.

use oxy_api_custom_apps::custom_apps_build_checks::queue;
use oxy_api_custom_apps::custom_apps_publish::{PublishError, publish};

use super::*;
use crate::promote_rollback::{audit_rows, draft, fixture_builds, pointers, promote};
use crate::sandbox_agent_token::fixture::{agent, app_model, tarball};
use crate::sandbox_publish::{build_pk, input};

const REQUIRED: [&str; 4] = ["functions", "loads", "design_system", "smoke"];

async fn dry_run(agent: &Agent) -> Value {
    let body = json!({ "from": "staging", "dry_run": true });
    let (status, plan) = promote(agent, "production", body).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    plan
}

/// Both the promote route and Make Live refuse with `code`, and production
/// stays on `live`.
async fn both_refuse(agent: &Agent, code: &str, live: Uuid) {
    let (status, refused) = promote(agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], code, "{refused}");
    assert_eq!(refused["error"], code, "{refused}");
    assert_eq!(codes(&refused), vec![code.to_string()], "{refused}");
    let (status, refused) = make_live(agent, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "Make Live: {refused}");
    assert_eq!(refused["code"], code, "Make Live: {refused}");
    assert_eq!(pointers(agent).await.0, Some(live), "production moved");
}

/// Off, phase 1 exactly: four kinds, no `checks` in the plan, nothing held
/// while every check is still queued, and an override is not even read.
#[tokio::test]
async fn with_the_flag_off_the_plan_has_no_checks_and_nothing_is_held() {
    let agent = agent().await;
    let slug = agent.app.slug.clone();
    let built = draft(&agent, "gate-off", tarball(&slug, "gate off")).await;
    let queued: Vec<String> = attempt(&agent, built, 1)
        .await
        .into_iter()
        .map(|(kind, _, _)| kind)
        .collect();
    assert_eq!(
        queued,
        ["design_system", "functions", "loads", "validation"]
    );

    let plan = dry_run(&agent).await;
    assert!(plan["checks"].is_null(), "{plan}");
    assert!(plan.as_object().unwrap().contains_key("checks"), "{plan}");
    assert!(codes(&plan).is_empty(), "{plan}");
    let uri = format!("/customer-apps/{}", agent.app.id);
    let (_, app) = person(&agent, "GET", &uri, None).await;
    assert_eq!(app["promotion_checks"]["enabled"], false, "{app}");

    // A reason too short to be one, from anybody: with the gate off nobody reads it.
    let acted = overriding(json!({ "from": "staging" }), "x");
    let (status, plan) = promote(&agent, "production", acted).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert!(plan["checks"].is_null(), "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(built));
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());
}

/// On: a staging attempt has six kinds, and a promote waits for the four the
/// app requires by default, on the route with a plan and on the one without.
#[tokio::test]
async fn a_promote_waits_for_the_required_checks_of_the_newest_attempt() {
    let agent = agent().await;
    let (live, _) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    let slug = agent.app.slug.clone();
    let built = draft(&agent, "gate-1", tarball(&slug, "gate one")).await;
    let queued = attempt(&agent, built, 1).await;
    let kinds: Vec<(&str, &str, bool)> = queued
        .iter()
        .map(|(kind, status, advisory)| (kind.as_str(), status.as_str(), *advisory))
        .collect();
    assert_eq!(
        kinds,
        [
            ("changes", "queued", true),
            ("design_system", "queued", false),
            ("functions", "queued", false),
            ("loads", "queued", false),
            ("smoke", "queued", false),
            ("validation", "queued", false),
        ]
    );

    // Still queued: every required check is open.
    let plan = dry_run(&agent).await;
    let checks = &plan["checks"];
    assert_eq!(checks["required"], json!(REQUIRED), "{plan}");
    assert_eq!(checks["attempt"], 1, "{plan}");
    assert_eq!(checks["state"], "running", "{plan}");
    assert_eq!(checks["running"], json!(REQUIRED), "{plan}");
    assert_eq!(checks["unmet"], json!([]), "{plan}");
    assert_eq!(checks["rows"].as_array().unwrap().len(), 6, "{plan}");
    assert_eq!(checks["rows"][4]["kind"], "smoke", "{plan}");
    assert_eq!(checks["rows"][4]["label"], "Every screen opens", "{plan}");
    assert_eq!(
        checks["changes"],
        json!({ "changed": 0, "compared": 0, "seen": false, "seen_required": false })
    );
    assert_eq!(checks["summary"], json!(null), "{plan}");
    assert_eq!(checks["overridable"], true, "the org's owner: {plan}");
    assert_eq!(checks["overridden"], json!(null), "{plan}");
    assert_eq!(codes(&plan), ["checks_running"], "{plan}");
    both_refuse(&agent, "checks_running", live).await;

    // One required check failed: named by its label, and the build stays put.
    settle(&agent, built, 1, &[("design_system", "failed")]).await;
    let plan = dry_run(&agent).await;
    assert_eq!(plan["checks"]["state"], "failed", "{plan}");
    assert_eq!(plan["checks"]["unmet"], json!(["design_system"]), "{plan}");
    assert_eq!(plan["checks"]["running"], json!([]), "{plan}");
    assert_eq!(codes(&plan), ["checks_not_passed"], "{plan}");
    let message = plan["blockers"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Follows the design system (failed)"),
        "{message}"
    );
    both_refuse(&agent, "checks_not_passed", live).await;

    // The checks run again and pass: the newest attempt is the one read.
    let app = app_model(&agent.t.db, agent.app.id).await;
    let build = entity::app_builds::Entity::find_by_id(built)
        .one(&agent.t.db)
        .await
        .unwrap()
        .expect("the build");
    let again = queue::rerun(&agent.t.db, &app, &build)
        .await
        .expect("a rerun");
    assert_eq!(again.attempt, 2);
    settle(&agent, built, 2, &[]).await;
    let plan = dry_run(&agent).await;
    assert_eq!(plan["checks"]["attempt"], 2, "{plan}");
    assert_eq!(plan["checks"]["state"], "passed", "{plan}");
    assert!(codes(&plan).is_empty(), "{plan}");
    assert_eq!(plan["checks"]["overridden"], json!(null), "{plan}");
    // An override sent with nothing left to clear is accepted and audits nothing.
    let acted = overriding(json!({ "from": "staging" }), REASON);
    let (status, acted) = promote(&agent, "production", acted).await;
    assert_eq!(status, StatusCode::OK, "{acted}");
    assert_eq!(acted["moved"], true, "{acted}");
    assert_eq!(acted["checks"]["overridden"], json!({ "reason": REASON }));
    assert_eq!(pointers(&agent).await.0, Some(built));
    assert!(audit_rows(&agent, OVERRIDDEN).await.is_empty());
}

/// `seen_required`: screens that differ from production hold the promote
/// until a person has marked them seen.
#[tokio::test]
async fn changed_screens_hold_a_promote_until_a_person_has_seen_them() {
    let agent = agent().await;
    let (live, _) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    let uri = format!("/customer-apps/{}/promotion-checks", agent.app.id);
    let asked = Some(json!({ "required": null, "seen_required": true }));
    let (status, app) = person(&agent, "PUT", &uri, asked).await;
    assert_eq!(status, StatusCode::OK, "{app}");
    assert_eq!(app["promotion_checks"]["seen_required"], true, "{app}");
    assert_eq!(app["promotion_checks"]["enabled"], true, "{app}");

    let slug = agent.app.slug.clone();
    let built = draft(&agent, "gate-seen", tarball(&slug, "gate seen")).await;
    settle(&agent, built, 1, &[("changes", "passed")]).await;
    changes(&agent, built, 1, 4, false).await;
    let plan = dry_run(&agent).await;
    assert_eq!(
        plan["checks"]["changes"],
        json!({ "changed": 4, "compared": 28, "seen": false, "seen_required": true }),
        "{plan}"
    );
    assert_eq!(plan["checks"]["state"], "passed", "{plan}");
    assert_eq!(codes(&plan), ["changes_not_seen"], "{plan}");
    both_refuse(&agent, "changes_not_seen", live).await;

    changes(&agent, built, 1, 4, true).await;
    let plan = dry_run(&agent).await;
    assert_eq!(plan["checks"]["changes"]["seen"], true, "{plan}");
    assert!(codes(&plan).is_empty(), "{plan}");
    let (status, acted) = make_live(&agent, None).await;
    assert_eq!(status, StatusCode::OK, "{acted}");
    assert_eq!(pointers(&agent).await.0, Some(built));
}

/// A promoting publish makes a build nothing has checked, so with the gate on
/// it is refused whole, staging included, as the four-eyes rule refuses it.
#[tokio::test]
async fn a_promoting_publish_is_refused_whole_while_the_gate_is_on() {
    let agent = agent().await;
    let (live, staged) = fixture_builds(&agent).await;
    enable_gate(&agent).await;
    let slug = agent.app.slug.clone();
    let mut promoting = input(&agent.t, &slug, "gate-pp", tarball(&slug, "promoting"));
    promoting.promote = true;
    let refused = publish(promoting).await.expect_err("held by the checks");
    let PublishError::ProductionBlocked(blockers) = &refused else {
        panic!("not a production refusal: {refused}");
    };
    let listed: Vec<&str> = blockers.iter().map(|blocker| blocker.code).collect();
    assert_eq!(listed, ["checks_not_passed"], "{refused}");
    assert_eq!(build_pk(&agent.t.db, agent.app.id, "gate-pp").await, None);
    assert_eq!(pointers(&agent).await, (Some(live), Some(staged)));
}
