//! Row E3: the token diffs a sandbox it created (`GET …/environments/{name}/diff`)
//! and nothing else. The route is the environment `show` door, so production,
//! staging without the grant, a colleague's sandbox and another app are the
//! `404`s of the other environment routes.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::agent;
use crate::app_environments::seed_sandbox;

#[tokio::test]
async fn a_token_diffs_its_own_sandbox_and_nothing_else() {
    let a = agent().await;
    let (status, body) = a.create("dev-own").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    seed_sandbox(&a.t.db, a.app.id, "dev-col", a.t.guest_id).await;
    let diff = |app: Uuid, environment: &str| {
        format!("/customer-apps/{app}/environments/{environment}/diff")
    };

    // On `schema` (the default) there is no branch to compare: no plane.
    let (status, body) = a.get(&diff(a.app.id, "dev-own")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"environment": "dev-own", "planes": {}}));

    for environment in ["production", "staging", "dev-col"] {
        let (status, body) = a.get(&diff(a.app.id, environment)).await;
        assert_eq!(
            (status, &body["code"]),
            (StatusCode::NOT_FOUND, &Value::from("environment_not_found")),
            "{environment}: {body}"
        );
    }
    let (status, body) = a.get(&diff(Uuid::new_v4(), "dev-own")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}
