//! `GET /api/customer-apps/{org}/{app}/health` — ferret's `app-health` check.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`** (`checkAppHealth`,
//! `healthVerdict`, `failingRung` in `src/uptime/checks.ts`).
//!
//! ferret reads the status and the body together:
//!
//! - **pass** — `200` and `oxy_app_health == "pass"`;
//! - **fail** — any other answer with a JSON body, named by the first
//!   `checks[]` entry whose `result` is `"fail"` (`name: detail`), else by
//!   `error`. A `404` for an app that is gone is a fail;
//! - **unknown** — `401`/`403` (the key, not the app), a body that is not
//!   JSON, and a `5xx` with no failing rung (the platform, not the app).
//!
//! So a rung renamed or a `result` respelled turns a broken app into an
//! unknown one, and a status that moves into 401/403 hides it entirely.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::environments::{EnvAction, record_move};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::Value;
use uuid::Uuid;

use super::super::stack::published_app;
use super::{Field, assert_fields, ferret_get, staff_fixture, staff_keys};

/// Every rung. `detail` is read off a failing rung only, so it is pinned there.
const RUNG: &[(&str, Field)] = &[("name", Field::Str), ("result", Field::Str)];

/// `/customer-apps/{org}/{app}/health`, as ferret builds it from the list's slugs.
pub(super) fn health_uri(org_id: Uuid, app: &apps::Model) -> String {
    format!(
        "/customer-apps/acme-{}/{}/health",
        org_id.simple(),
        app.slug
    )
}

/// A published app whose production serves a build with its `index.html` in
/// the build store: every rung of the ladder passes.
async fn live_app(db: &DatabaseConnection, org_id: Uuid, workspace_id: Uuid) -> apps::Model {
    let app = published_app(db, org_id, workspace_id).await;
    let build = "contract-b1";
    let index =
        b"<!doctype html><script type=\"module\" src=\"./assets/index-a1b2c3d4.js\"></script>";
    let prefix = oxy_custom_apps::build_store::put_build(
        app.id,
        build,
        vec![("index.html".to_string(), index.to_vec())],
    )
    .await
    .expect("store the build");
    let build_pk = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(build_pk),
        app_id: ActiveValue::Set(app.id),
        build_id: ActiveValue::Set(build.to_string()),
        s3_prefix: ActiveValue::Set(prefix),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        validation_status: ActiveValue::Set("passed".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed the build row");
    let production = AppEnvironment::Production;
    record_move(
        db,
        app.id,
        &production,
        Some(build_pk),
        EnvAction::Promote,
        None,
    )
    .await
    .expect("point production at the build");
    app
}

/// The first failing rung, as ferret's `failingRung` finds it.
fn failing_rung(body: &Value) -> Option<&Value> {
    body["checks"]
        .as_array()?
        .iter()
        .find(|c| c["result"] == "fail")
}

#[tokio::test]
async fn a_serving_app_is_200_pass() {
    let fx = staff_fixture().await;
    let app = live_app(&fx.db, fx.org_id, fx.workspace_id).await;

    for (shape, key) in staff_keys(&fx).await {
        let answer = ferret_get(Some(&key), &health_uri(fx.org_id, &app)).await;
        answer.assert_json_200(&format!("a serving app's health, {shape}"));
        assert_eq!(answer.body["oxy_app_health"], "pass", "{}", answer.body);
        assert_fields(&answer.body, &[("checks", Field::Array)], "the health body");
        for rung in answer.body["checks"].as_array().unwrap() {
            assert_fields(rung, RUNG, "a health rung");
        }
        assert!(failing_rung(&answer.body).is_none(), "{}", answer.body);
    }
}

#[tokio::test]
async fn a_published_app_with_no_build_is_503_fail_naming_its_rung() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let (_, key) = staff_keys(&fx).await.remove(1);

    let answer = ferret_get(Some(&key), &health_uri(fx.org_id, &app)).await;
    assert_eq!(answer.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        answer.content_type.contains("json"),
        "{}",
        answer.content_type
    );
    assert_eq!(answer.body["oxy_app_health"], "fail", "{}", answer.body);
    // A 5xx is a fail for ferret only when a rung says so; without one it
    // reads as the platform's fault and is UNKNOWN.
    let rung = failing_rung(&answer.body).unwrap_or_else(|| {
        panic!(
            "a 503 with no failing rung reads as UNKNOWN: {}",
            answer.body
        )
    });
    assert_fields(rung, RUNG, "the failing rung");
    assert_fields(rung, &[("detail", Field::Str)], "the failing rung");
}

#[tokio::test]
async fn an_app_that_is_gone_is_a_json_404_fail_with_an_error() {
    let fx = staff_fixture().await;
    let (_, key) = staff_keys(&fx).await.remove(1);

    let uri = format!(
        "/customer-apps/acme-{}/no-such-app/health",
        fx.org_id.simple()
    );
    let answer = ferret_get(Some(&key), &uri).await;
    assert_eq!(
        answer.status,
        axum::http::StatusCode::NOT_FOUND,
        "{}",
        answer.body
    );
    // Under 500 and JSON: ferret raises a fail named by `error`.
    assert!(
        answer.content_type.contains("json"),
        "{}",
        answer.content_type
    );
    assert_eq!(answer.body["oxy_app_health"], "fail");
    assert_fields(&answer.body, &[("error", Field::Str)], "the 404 body");
}
