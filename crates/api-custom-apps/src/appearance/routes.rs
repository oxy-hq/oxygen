//! The four appearance routes. Thin: authenticate, refuse a frontline
//! identity, parse, then one `store` call. See the module docs in `mod.rs`.

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::DbErr;
use serde_json::{Value, json};
use tracing::instrument;
use uuid::Uuid;

use super::{ColorMode, answer, has_mode, parse_body, store};
use crate::custom_apps_gates::check_custom_app_gates;

/// Oxygen web's path, as the environment guard is asked about it.
pub const MINE_PATH: &str = "/api/me/appearance";

/// The bundle's path for one project, as the environment guard is asked
/// about it.
pub fn app_path(project_id: Uuid) -> String {
    format!("/api/projects/{project_id}/shell-context/mode")
}

/// `GET /api/me/appearance` — `{ "mode": … }` for the signed-in person.
#[instrument(skip_all)]
pub async fn get_mine(AuthenticatedUserExtractor(user): AuthenticatedUserExtractor) -> Response {
    if !has_mode(&user) {
        return no_mode();
    }
    match establish_connection().await {
        Ok(db) => respond(store::load(&db, user.id).await),
        Err(e) => unavailable(e),
    }
}

/// `PUT /api/me/appearance` — body `{ "mode": … }`; answers what was stored.
#[instrument(skip_all)]
pub async fn put_mine(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // A write: decided from the headers before anything is read.
    if let Err(refused) =
        oxy_app_core::custom_app_env_request::guard(&Method::PUT, MINE_PATH, &headers)
    {
        return *refused;
    }
    if !has_mode(&user) {
        return no_mode();
    }
    let mode = match parse(&body) {
        Ok(mode) => mode,
        Err(refused) => return refused,
    };
    match establish_connection().await {
        Ok(db) => respond(store::save(&db, user.id, mode).await.map(|()| Some(mode))),
        Err(e) => unavailable(e),
    }
}

/// `GET /api/projects/{project_id}/shell-context/mode` — the same value for a
/// bundle, re-read when its tab is shown again. The gate chain first.
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn get_for_app(Path(project_id): Path<Uuid>, headers: HeaderMap) -> Response {
    let ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(ctx) => ctx,
        Err(refused) => return refused,
    };
    if !has_mode(&ctx.user) {
        return no_mode();
    }
    respond(store::load(&ctx.db, ctx.user.id).await)
}

/// `PUT /api/projects/{project_id}/shell-context/mode` — the shell's switch.
/// The gate chain first; the body is read only after it.
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn put_for_app(
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(ctx) => ctx,
        Err(refused) => return refused,
    };
    if let Err(refused) =
        oxy_app_core::custom_app_env_request::guard(&Method::PUT, &app_path(project_id), &headers)
    {
        return *refused;
    }
    if !has_mode(&ctx.user) {
        return no_mode();
    }
    let mode = match parse(&body) {
        Ok(mode) => mode,
        Err(refused) => return refused,
    };
    respond(
        store::save(&ctx.db, ctx.user.id, mode)
            .await
            .map(|()| Some(mode)),
    )
}

/// A write's body as a mode, or the 422 that says what to send.
fn parse(body: &[u8]) -> Result<ColorMode, Response> {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    parse_body(&value).map_err(|message| {
        private(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": "mode_invalid", "message": message }),
        )
    })
}

/// The answer, never cached: it is one person's (see
/// `custom_apps_shell_context` for why even `private, max-age` would replay it
/// to the next account on a shared browser).
fn private(status: StatusCode, body: Value) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(body),
    )
        .into_response()
}

fn respond(result: Result<Option<ColorMode>, DbErr>) -> Response {
    match result {
        Ok(mode) => private(StatusCode::OK, answer(mode)),
        Err(e) => {
            tracing::error!("appearance: database call failed: {e}");
            private(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "appearance_unavailable" }),
            )
        }
    }
}

fn unavailable(e: impl std::fmt::Display) -> Response {
    tracing::error!("appearance: database connection failed: {e}");
    private(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "appearance_unavailable" }),
    )
}

/// A crew identity: there is no Oxygen account, so there is no mode. 404, as
/// for a route that does not exist for it, and never a toast in Oxygen web.
fn no_mode() -> Response {
    private(
        StatusCode::NOT_FOUND,
        json!({
            "error": "no_appearance",
            "message": "a crew sign-in has no Oxygen appearance; the app keeps its own",
        }),
    )
}
