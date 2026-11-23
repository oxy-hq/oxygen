//! A person's light/dark mode (`appearance`) against a real database, the
//! bundle routes' refusals before any read, and the environment guard in front
//! of the shell's switch: a staging or sandbox preview never changes it.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use oxy_api_custom_apps::appearance::{ColorMode, store};
use oxy_api_custom_apps::public_routes;
use oxy_app_core::app_state::bare_app_state;
use oxy_app_core::custom_app_env_request::{
    NoWorkspaceActions, WorkspaceActions, environment_guard_middleware,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use super::custom_app_visibility::seed_user;
use crate::common::test_db;
use crate::custom_app_procedure_run_fixture::fixture;

#[tokio::test]
async fn no_row_is_not_chosen_and_a_save_is_read_back_then_replaced() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let (other, _) = seed_user(&conn).await;
    assert_eq!(store::load(&conn, user).await.unwrap(), None);

    store::save(&conn, user, ColorMode::Dark).await.unwrap();
    assert_eq!(
        store::load(&conn, user).await.unwrap(),
        Some(ColorMode::Dark)
    );
    store::save(&conn, user, ColorMode::System).await.unwrap();
    assert_eq!(
        store::load(&conn, user).await.unwrap(),
        Some(ColorMode::System)
    );
    // One person's mode is not another's.
    assert_eq!(store::load(&conn, other).await.unwrap(), None);
}

#[tokio::test]
async fn the_column_refuses_a_fourth_mode_and_the_row_goes_with_its_person() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let sepia = conn
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO user_ui_preferences (user_id, color_mode) VALUES ($1, 'sepia')",
            [user.into()],
        ))
        .await;
    assert!(sepia.is_err(), "the CHECK keeps a fourth mode out");

    store::save(&conn, user, ColorMode::Light).await.unwrap();
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM users WHERE id = $1",
        [user.into()],
    ))
    .await
    .unwrap();
    let left = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM user_ui_preferences WHERE user_id = $1",
            [user.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(left.try_get::<i64>("", "n").unwrap(), 0);
}

/// Auth first: with no credential the bundle's mode routes answer 401 before
/// they read a body or a row, so a malformed write learns nothing either.
#[tokio::test]
async fn the_bundle_routes_refuse_an_anonymous_caller_before_anything_else() {
    // A server with a sign-in configured, as every cloud one is; without one the
    // caller is the zero-config guest.
    oxy_auth::built_in::set_auth_configured(true);
    let app = public_routes().with_state(bare_app_state());
    let path = format!("/projects/{}/shell-context/mode", Uuid::new_v4());
    for (method, body) in [
        ("GET", ""),
        ("PUT", "not json"),
        ("PUT", r#"{"mode":"dark"}"#),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path} with {body:?}"
        );
    }
}

/// `/api` behind the environment guard, as `oxy serve` wraps it
/// (`wrap_outer_service`): the guard decides before any handler runs.
fn guarded_api() -> Router {
    Router::new()
        .nest("/api", public_routes().with_state(bare_app_state()))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(NoWorkspaceActions) as Arc<dyn WorkspaceActions>,
            environment_guard_middleware,
        ))
}

/// The shell's switch: `PUT …/shell-context/mode` from a page on `host` (with
/// the session cookie and that host as its origin, as a browser sends it), or
/// from the admin host's path URL (production) when `host` is `None`.
async fn put_mode(host: Option<&str>, project: Uuid, mode: &str) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("PUT")
        .uri(format!("/api/projects/{project}/shell-context/mode"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(host) = host {
        request = request
            .header(header::HOST, host)
            .header(header::COOKIE, "oxy_session=x")
            .header(header::ORIGIN, format!("https://{host}"));
    }
    let response = guarded_api()
        .oneshot(
            request
                .body(Body::from(format!(r#"{{"mode":"{mode}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Mars, 16 Nov: a mode chosen on a staging or sandbox host must not write the
/// person's real one. The guard refuses the write there like every other write,
/// so the stored mode is unchanged; the same write from production lands.
#[tokio::test]
async fn a_staging_or_sandbox_switch_never_changes_the_stored_mode_and_production_does() {
    let f = fixture().await;
    store::save(&f.t.db, f.t.guest_id, ColorMode::Dark)
        .await
        .unwrap();
    let staging = format!(
        "staging--{}--store.customer-apps.oxygen-hq.com",
        f.t.org_slug
    );
    let sandbox = format!(
        "dev-mars--{}--store.customer-apps.oxygen-hq.com",
        f.t.org_slug
    );
    for host in [staging.as_str(), sandbox.as_str()] {
        let (status, body) = put_mode(Some(host), f.workspace_id, "light").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{host}: {body}");
        assert_eq!(body["error"], "EnvironmentRefused", "{host}: {body}");
        assert_eq!(
            store::load(&f.t.db, f.t.guest_id).await.unwrap(),
            Some(ColorMode::Dark),
            "{host}: the stored mode is unchanged"
        );
    }
    let (status, body) = put_mode(None, f.workspace_id, "light").await;
    assert_eq!(status, StatusCode::OK, "production: {body}");
    assert_eq!(body["mode"], "light");
    assert_eq!(
        store::load(&f.t.db, f.t.guest_id).await.unwrap(),
        Some(ColorMode::Light),
        "production writes the person's mode"
    );
}
