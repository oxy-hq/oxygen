//! The second-person rule (`apps.require_second_person_to_promote`) on the
//! routes that move production, in the publisher's own session: the fixture's
//! production build (`agent-prod`) and staging's (`agent-stg`) were both
//! published by the guest, whose session `person` is.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{fixture_builds, person, pointers, promote};
use crate::sandbox_agent_token::fixture::{Agent, agent};

/// `PUT …/promotion-rule`, through the real route; the app it answers says
/// the rule is now `on`.
async fn set_rule(agent: &Agent, on: bool) {
    let uri = format!("/customer-apps/{}/promotion-rule", agent.app.id);
    let body = Some(json!({ "require_second_person_to_promote": on }));
    let (status, app) = person(agent, "PUT", &uri, body).await;
    assert_eq!(status, StatusCode::OK, "{app}");
    assert_eq!(app["require_second_person_to_promote"], on, "{app}");
}

/// The console's rollback (`POST …/rollback`) to `build`.
async fn console_rollback(agent: &Agent, build: uuid::Uuid) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    person(agent, "POST", &uri, Some(json!({ "build_id": build }))).await
}

fn names_second_person(refused: &Value) -> bool {
    refused["code"] == "same_person_promote"
        || refused["blockers"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["code"] == "same_person_promote"))
}

/// The console's rollback takes any build staging served, so its rollback to
/// a build production never served is a promote by another name: refused to
/// the build's publisher while the rule is on, as Promote to production is.
/// A rollback to a build production served before stays open to them — the
/// emergency path.
#[tokio::test]
async fn the_publisher_rolls_back_only_to_a_build_production_served() {
    let agent = agent().await;
    let (prod, stg) = fixture_builds(&agent).await;
    set_rule(&agent, true).await;

    let (status, refused) = console_rollback(&agent, stg).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(names_second_person(&refused), "{refused}");
    assert_eq!(
        pointers(&agent).await.0,
        Some(prod),
        "production did not move"
    );

    let (status, refused) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(names_second_person(&refused), "{refused}");
    assert_eq!(
        pointers(&agent).await.0,
        Some(prod),
        "production did not move"
    );

    // With the rule off the publisher promotes staging's build, so production
    // has now served both.
    set_rule(&agent, false).await;
    let (status, plan) = promote(&agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(stg));

    set_rule(&agent, true).await;
    let (status, back) = console_rollback(&agent, prod).await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(pointers(&agent).await.0, Some(prod));
}
