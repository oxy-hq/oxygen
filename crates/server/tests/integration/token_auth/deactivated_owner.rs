//! A deactivated user's key or token is refused where it is admitted, so the
//! refusal holds on every entry point.
//!
//! The status check in `auth_middleware` covers `/api` alone. The custom-app
//! paths authenticate inside their handlers — `/fn` and `/logs` both through
//! `authenticate_and_authorize`, which also keeps the user row for 60 s — so
//! while admission did not read `users.status`, a deactivated minter's token
//! kept invoking functions and reading logs.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{any, get};
use entity::org_members::OrgRole;
use entity::organizations;
use entity::users::{self, UserStatus};
use oxy_app::server::api::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_app::server::api::{custom_apps_logs, custom_apps_serve};
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use super::service_accounts::{admin_fixture, create_account, mint_account_token, with_token};
use super::stack::{flat_api, join_org, published_app};
use super::{Fixture, call, fixture, legacy_key, minted_pat};

/// `POST /customer-apps/<org>/<app>/fn/<name>`, mounted as `serve.rs` mounts
/// it: the dispatcher refuses every function without the query executor.
fn function_surface() -> Router {
    Router::new().route(
        "/customer-apps/{*path}",
        any(custom_apps_serve::serve_dispatch)
            .layer(axum::Extension(
                Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>
            )),
    )
}

/// `GET /customer-apps/<org>/<app>/logs`, as `router/public.rs` mounts it.
fn logs_surface() -> Router {
    Router::new().route(
        "/customer-apps/{org_slug}/{app_slug}/logs",
        get(custom_apps_logs::get_logs),
    )
}

/// The two custom-app paths of one app.
pub(super) struct Paths {
    invoke: String,
    logs: String,
}

/// The fixture user as Owner of an org with one published app.
pub(super) async fn app_fixture() -> (Fixture, Paths) {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let org = organizations::Entity::find_by_id(fx.org_id)
        .one(&fx.db)
        .await
        .expect("read the org")
        .expect("the fixture org");
    let base = format!("/customer-apps/{}/{}", org.slug, app.slug);
    let paths = Paths {
        invoke: format!("{base}/fn/ping"),
        logs: format!("{base}/logs"),
    };
    (fx, paths)
}

/// A credential as the header it travels in.
pub(super) type Credential = (&'static str, String);

pub(super) fn bearer(secret: &str) -> Credential {
    ("authorization", format!("Bearer {secret}"))
}

pub(super) fn api_key(secret: &str) -> Credential {
    ("x-api-key", secret.to_string())
}

/// What `/fn` and `/logs` answer `credential`, in that order.
pub(super) async fn fn_and_logs(paths: &Paths, credential: &Credential) -> [StatusCode; 2] {
    let headers = [(credential.0, credential.1.as_str())];
    let (invoke, _) = call(
        function_surface(),
        "POST",
        &paths.invoke,
        &headers,
        Some(json!({})),
    )
    .await;
    let (logs, _) = call(logs_surface(), "GET", &paths.logs, &headers, None).await;
    [invoke, logs]
}

async fn set_status(fx: &Fixture, status: UserStatus) {
    let mut row: users::ActiveModel = fx.user.clone().into();
    row.status = ActiveValue::Set(status);
    row.update(&fx.db).await.expect("set the user's status");
}

#[tokio::test]
async fn a_deactivated_owners_token_is_refused_on_the_next_fn_and_logs() {
    let (fx, paths) = app_fixture().await;
    let (_, pat) = minted_pat(&fx, None).await;
    let (_, key) = legacy_key(&fx, None).await;
    let credentials = [bearer(&pat), api_key(&key)];

    // Admitted while the owner is active. These calls also fill the credential
    // cache and the custom-app path's 60 s user cache.
    for credential in &credentials {
        for status in fn_and_logs(&paths, credential).await {
            assert_ne!(
                status,
                StatusCode::UNAUTHORIZED,
                "{}: admitted while the owner is active",
                credential.0
            );
        }
    }

    set_status(&fx, UserStatus::Deleted).await;
    for credential in &credentials {
        assert_eq!(
            fn_and_logs(&paths, credential).await,
            [StatusCode::UNAUTHORIZED; 2],
            "{}: refused on the next request, with both caches still warm",
            credential.0
        );
    }

    // The token was never revoked, so an owner who is active again is admitted.
    set_status(&fx, UserStatus::Active).await;
    for credential in &credentials {
        for status in fn_and_logs(&paths, credential).await {
            assert_ne!(
                status,
                StatusCode::UNAUTHORIZED,
                "{}: admitted once the owner is active again",
                credential.0
            );
        }
    }
}

#[tokio::test]
async fn a_deactivated_owners_token_is_refused_when_first_presented() {
    let (fx, paths) = app_fixture().await;
    let (_, pat) = minted_pat(&fx, None).await;
    let (_, key) = legacy_key(&fx, None).await;
    set_status(&fx, UserStatus::Deleted).await;

    // Never presented before, so nothing is cached: the database path refuses.
    for credential in [bearer(&pat), api_key(&key)] {
        assert_eq!(
            fn_and_logs(&paths, &credential).await,
            [StatusCode::UNAUTHORIZED; 2],
            "{}",
            credential.0
        );
        // `/api` too: refused at admission, ahead of the middleware's own
        // status check.
        let headers = [(credential.0, credential.1.as_str())];
        let (status, _) = call(flat_api(), "GET", "/orgs", &headers, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{} on /api", credential.0);
    }
}

/// A service account's `users` row can be switched off while its
/// `service_accounts` row stands (deleting the account removes that row; the
/// staff users console does not). The cached path asks about both rows, as the
/// database path does.
#[tokio::test]
async fn a_service_account_whose_user_row_is_deactivated_is_refused_cached_or_not() {
    let fx = admin_fixture().await;
    let account = create_account(&fx, "deployer", "member").await;
    let (_, secret) = mint_account_token(&fx, account, json!({ "name": "t" })).await;
    let (status, _) = with_token(&secret, "GET", "/orgs", None).await;
    assert_eq!(status, StatusCode::OK, "admitted, and now cached");

    let retired = users::Entity::update_many()
        .col_expr(users::Column::Status, Expr::value(UserStatus::Deleted))
        .filter(users::Column::Id.eq(account))
        .exec(&fx.db)
        .await
        .expect("deactivate the account's user row");
    assert_eq!(retired.rows_affected, 1, "the account's user row");

    let (status, _) = with_token(&secret, "GET", "/orgs", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "refused from the cache");
    oxy_auth::token::cache::clear();
    let (status, _) = with_token(&secret, "GET", "/orgs", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "and from the database");
}
