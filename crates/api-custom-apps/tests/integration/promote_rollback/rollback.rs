//! `oxyc rollback`: the previous build by default, a build the environment
//! served by `to`, and what is never rolled back.

use axum::http::StatusCode;
use serde_json::json;

use super::{
    audit_rows, draft, events, fixture_builds, pointers, promote, rollback, sandbox_build,
};
use crate::sandbox_agent_token::fixture::{agent, published, tarball};

/// Production promoted once has a previous build: a rollback with no `to`
/// moves back to it, records `production / rollback` and `app.rolled_back`,
/// and a second rollback swaps back — undo.
#[tokio::test]
async fn production_rolls_back_to_its_previous_build_and_twice_swaps_back() {
    let agent = agent().await;
    let (prod, stg) = fixture_builds(&agent).await;
    let (status, refused) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "nothing_to_roll_back_to", "{refused}");

    let (status, _) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::OK);

    let (status, plan) = rollback(&agent, "production", json!({ "dry_run": true })).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["action"], "rollback");
    assert!(plan["from"].is_null(), "{plan}");
    assert_eq!(plan["build"]["build_id"], "agent-prod", "{plan}");
    assert_eq!(
        pointers(&agent).await.0,
        Some(stg),
        "a dry run moves nothing"
    );

    let (status, acted) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{acted}");
    assert_eq!(acted["moved"], true);
    assert_eq!(pointers(&agent).await.0, Some(prod));
    let last = events(&agent).await.pop().expect("an event");
    assert_eq!(last, ("production".into(), Some(prod), "rollback".into()));
    let rows = audit_rows(&agent, "app.rolled_back").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].metadata["build"], json!(prod));
    assert_eq!(rows[0].metadata["previous"], json!(stg));

    let (status, _) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pointers(&agent).await.0, Some(stg), "undo swaps back");
}

/// `to` names a build the environment served before — by publish id — and
/// only such a build; the build it serves now is `already_serving`.
#[tokio::test]
async fn to_names_a_build_the_environment_served() {
    let agent = agent().await;
    let (prod, _) = fixture_builds(&agent).await;
    let slug = agent.app.slug.clone();
    let (status, _) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::OK);
    draft(&agent, "stg-never-live", tarball(&slug, "draft")).await;

    let (status, refused) = rollback(&agent, "production", json!({ "to": "stg-never-live" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "build_not_served", "{refused}");
    let (status, refused) = rollback(&agent, "production", json!({ "to": "agent-stg" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "already_serving", "{refused}");

    let (status, acted) = rollback(&agent, "production", json!({ "to": "agent-prod" })).await;
    assert_eq!(status, StatusCode::OK, "{acted}");
    assert_eq!(pointers(&agent).await.0, Some(prod));
}

/// Staging rolls back as production does — its event is `staging /
/// rollback` — and a sandbox is never rolled back: it is published to again.
#[tokio::test]
async fn staging_rolls_back_and_a_sandbox_does_not() {
    let agent = agent().await;
    let (_, stg) = fixture_builds(&agent).await;
    let slug = agent.app.slug.clone();
    let newer = draft(&agent, "stg-newer", tarball(&slug, "newer")).await;
    assert_eq!(pointers(&agent).await.1, Some(newer));

    let (status, acted) = rollback(&agent, "staging", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{acted}");
    assert_eq!(acted["migrations"]["when"], "queued_after", "{acted}");
    assert_eq!(pointers(&agent).await.1, Some(stg));
    let last = events(&agent).await.pop().expect("an event");
    assert_eq!(last, ("staging".into(), Some(stg), "rollback".into()));

    sandbox_build(&agent, "r1", "sbx-r1", tarball(&slug, "sandbox"), true).await;
    let (status, refused) = rollback(&agent, "dev-r1", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "not_rollbackable", "{refused}");
}

/// The console's Make Live (`POST …/rollback` with a `build_id`) answers a
/// coded `404` the console can show: an app that does not exist is
/// `app_not_found`, and a build id that is not one of this app's — unknown
/// or another app's alike — is `build_not_found`. Production never moves.
#[tokio::test]
async fn console_rollback_names_what_it_could_not_find() {
    let agent = agent().await;
    let (prod, _) = fixture_builds(&agent).await;
    let other = published(&agent.t, "sbx-agent-other").await;
    let foreign = crate::sandbox_publish::build_pk(&agent.t.db, other.id, "agent-prod")
        .await
        .expect("the other app's build");
    let before = pointers(&agent).await.0;

    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    for build in [uuid::Uuid::new_v4(), foreign] {
        let named = Some(json!({ "build_id": build }));
        let (status, body) = super::person(&agent, "POST", &uri, named).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["code"], "build_not_found", "{body}");
        assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    let missing = format!("/customer-apps/{}/rollback", uuid::Uuid::new_v4());
    let named = Some(json!({ "build_id": prod }));
    let (status, body) = super::person(&agent, "POST", &missing, named).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "app_not_found", "{body}");

    assert_eq!(pointers(&agent).await.0, before, "production never moved");
}
