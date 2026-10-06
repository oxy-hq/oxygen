//! A publish token on the app-secrets surface (review S2; plan D22: every
//! non-production operation refuses a publish token).
//!
//! A publish token authenticates as whoever minted it, and its scope
//! (`middlewares::app_publish_token_scope`) admits every `GET` under
//! `/customer-apps/`. So a token minted by staff reached `GET …/secrets` and
//! `GET …/secrets/{key}/value` with `?environment=staging` — or a sandbox's
//! name — and was answered with the values. These tests drive the two routes
//! through that scope middleware, as production mounts them.

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_secrets::{admin_list, admin_reveal};
use oxy_app::server::api::middlewares::app_publish_token_scope::app_publish_token_scope_middleware;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::types::AppPublishTokenAuth;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_app, seeded_tenant};
use crate::staging_functions::make_guest_staff;

/// What the auth middleware inserts for a request carrying `oxypublish_…`.
async fn as_publish_token(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: None,
        machine_identity: None,
    });
    next.run(request).await
}

/// The two read routes behind the token's scope middleware, signed in as the
/// guest — carrying a publish token when `token`.
fn secrets(token: bool) -> Router {
    let mut routes = Router::new()
        .route("/customer-apps/{id}/secrets", get(admin_list))
        .route("/customer-apps/{id}/secrets/{key}/value", get(admin_reveal))
        .layer(middleware::from_fn(app_publish_token_scope_middleware));
    if token {
        routes = routes.layer(middleware::from_fn(as_publish_token));
    }
    Router::new()
        .nest("/api", routes)
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(),
            auth_middleware,
        ))
}

async fn read(token: bool, path: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/customer-apps/{path}"))
        .body(Body::empty())
        .expect("request");
    let response = secrets(token).oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A published app with `QB_TOKEN` set in production, staging and the
/// sandbox `dev-a1`, each to a value that names where it came from.
async fn app_with_a_value_everywhere(t: &Tenant) -> Uuid {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app = publish_app(t, "sbx-token", demo_workspace_id(), &[noop])
        .await
        .app_id;
    crate::app_environments::seed_sandbox(&t.db, app, "dev-a1", t.guest_id).await;
    let secrets = SecretManagerService::new(demo_workspace_id());
    for (environment, value) in [
        (None, "prod-secret"),
        (Some("staging"), "stg-secret"),
        (Some("dev-a1"), "a1-secret"),
    ] {
        secrets
            .set_app_secret_in(&t.db, app, environment, "QB_TOKEN", value, t.guest_id)
            .await
            .expect("seed a secret");
    }
    make_guest_staff();
    app
}

/// A publish token cannot list or reveal the secrets of staging or of a
/// sandbox, whoever minted it: `403`, and no value in the answer. Production
/// is answered exactly as it was, and the same staff member without the token
/// reads all three.
#[tokio::test]
async fn a_publish_token_cannot_read_non_production_secrets() {
    let t = seeded_tenant().await;
    let app = app_with_a_value_everywhere(&t).await;

    for (environment, value) in [("staging", "stg-secret"), ("dev-a1", "a1-secret")] {
        let (status, body) = read(
            true,
            &format!("{app}/secrets/QB_TOKEN/value?environment={environment}"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "reveal {environment}: {body}"
        );
        assert!(!body.contains(value), "reveal {environment} leaked: {body}");
        assert!(body.contains("publish_token_refused"), "{body}");

        let (status, body) = read(true, &format!("{app}/secrets?environment={environment}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "list {environment}: {body}");
        assert!(
            !body.contains("QB_TOKEN"),
            "list {environment} leaked: {body}"
        );

        // Control: the token is what is refused, not the caller.
        let (status, body) = read(
            false,
            &format!("{app}/secrets/QB_TOKEN/value?environment={environment}"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "staff, no token, {environment}: {body}"
        );
        assert!(body.contains(value), "{body}");
    }

    // Production behaves exactly as before the token was refused elsewhere.
    for path in [
        format!("{app}/secrets/QB_TOKEN/value"),
        format!("{app}/secrets/QB_TOKEN/value?environment=production"),
    ] {
        let (status, body) = read(true, &path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(body.contains("prod-secret"), "{path}: {body}");
    }
    let (status, body) = read(true, &format!("{app}/secrets")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("QB_TOKEN"), "{body}");
}

/// The two writes are refused by each handler too — not only by the token's
/// scope, which never lets a token's `POST` or `DELETE` reach them: a staff
/// caller carrying the marker cannot set or delete a staging or sandbox
/// secret, and can still do both in production.
#[tokio::test]
async fn a_publish_token_cannot_write_non_production_secrets_either() {
    use axum::extract::{Path, Query};
    use axum::{Extension, Json};
    use entity::users::UserStatus;
    use oxy_app::server::api::custom_apps_secrets::environment::EnvironmentQuery;
    use oxy_app::server::api::custom_apps_secrets::{SetSecretRequest, admin_delete, admin_set};
    use oxy_auth::types::AuthenticatedUser;
    use oxy_auth::user::LOCAL_GUEST_EMAIL;

    let t = seeded_tenant().await;
    let app = app_with_a_value_everywhere(&t).await;
    let staff = || {
        oxy_app_core::audit::RequestActor::session(AuthenticatedUser {
            id: t.guest_id,
            email: Some(LOCAL_GUEST_EMAIL.to_string()),
            name: "Secrets".to_string(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        })
    };
    let token = || {
        Some(Extension(AppPublishTokenAuth {
            token_id: Uuid::new_v4(),
            app_id: None,
            machine_identity: None,
        }))
    };
    let set = |environment: Option<&str>| -> Json<SetSecretRequest> {
        Json(
            serde_json::from_value(
                json!({ "key": "QB_TOKEN", "value": "by-token", "environment": environment }),
            )
            .expect("request"),
        )
    };
    let named = |environment: &str| {
        Query(EnvironmentQuery {
            environment: Some(environment.to_string()),
        })
    };
    let key = || Path((app, "QB_TOKEN".to_string()));
    let stored = |environment: &'static str| async move {
        let secrets = SecretManagerService::new(demo_workspace_id());
        secrets.clear_cache().await;
        secrets
            .get_secret(&format!("apps/{app}/{environment}QB_TOKEN"))
            .await
    };

    for (environment, path, value) in [
        ("staging", "staging/", "stg-secret"),
        ("dev-a1", "dev-a1/", "a1-secret"),
    ] {
        let (status, body) = admin_set(Path(app), staff(), token(), set(Some(environment)))
            .await
            .expect_err("a publish token sets no non-production secret");
        assert_eq!(status, StatusCode::FORBIDDEN, "set {environment}: {body}");
        assert!(body.contains("publish_token_refused"), "{body}");
        let (status, _) = admin_delete(key(), named(environment), staff(), token())
            .await
            .expect_err("a publish token deletes no non-production secret");
        assert_eq!(status, StatusCode::FORBIDDEN, "delete {environment}");
        assert_eq!(stored(path).await.as_deref(), Some(value), "{environment}");
    }
    // Production: the handler decides as it did before.
    admin_set(Path(app), staff(), token(), set(None))
        .await
        .expect("production is the mount's to guard, as before");
    assert_eq!(stored("").await.as_deref(), Some("by-token"));
}
