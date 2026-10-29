//! Home widgets: each app's live cards on the Oxygen home, and their buttons.
//!
//! - `GET /api/home/widgets?org=<slug>` — every widget of every app the caller
//!   can open in that org, each with its view or why there is none.
//! - `POST /api/home/widgets/{app_id}/{widget}/actions/{action}` — press a
//!   button.
//!
//! Flat, like `GET /api/me/apps`: no tenant in the path, so every app is
//! decided here by the `AppAccess` ring, and every function then runs on the
//! caller's own credential through the app's function route. Production only.
//! Design: `internal-docs/home-widgets.md`.

pub mod load;
pub mod run;

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use futures::StreamExt;
use oxy::database::client::establish_connection;
use oxy_app_core::AppState;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::function_seam::FunctionRuntime;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_app_core::widgets::view::{MAX_INPUT_BYTES, MAX_KEY_CHARS};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::instrument;
use uuid::Uuid;

use run::{Caller, Press, Pressed, WidgetCard};

/// Widgets read at once: enough that one slow app does not hold up the home,
/// few enough that one home load is not a burst on the function fleet.
const CONCURRENT_READS: usize = 6;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub org: String,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub widgets: Vec<WidgetCard>,
}

/// `GET /api/home/widgets?org=<slug>`
#[instrument(skip_all, fields(org = %q.org))]
pub async fn list(
    State(state): State<AppState>,
    runtime: Option<Extension<Arc<dyn FunctionRuntime>>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> Response {
    let path = "/api/home/widgets";
    match oxy_app_core::custom_app_env_request::guard(&Method::GET, path, &headers) {
        Err(refused) => return *refused,
        // The home links to production; a staging or sandbox host has no widgets.
        Ok(env) if env != AppEnvironment::Production => {
            return Json(ListResponse { widgets: vec![] }).into_response();
        }
        Ok(_) => {}
    }
    let Some(Extension(runtime)) = runtime else {
        tracing::error!("home widgets asked for on a server with no function runtime");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let apps = match widget_apps(&user, &q.org).await {
        Ok(apps) => apps,
        Err(status) => return status.into_response(),
    };
    let caller = Caller {
        headers: &headers,
        preagg: preagg(&state),
        runtime: runtime.as_ref(),
    };
    // Built up front, not mapped inside the stream: a closure over borrowed
    // items in the stream's type is not `Send` for every lifetime axum asks.
    let mut reads = Vec::new();
    for app in &apps {
        for def in &app.widgets {
            reads.push(run::read(&app.app, def, &caller));
        }
    }
    let widgets: Vec<WidgetCard> = futures::stream::iter(reads)
        .buffered(CONCURRENT_READS)
        .collect()
        .await;
    Json(ListResponse { widgets }).into_response()
}

/// The apps in `org` the caller can open that declare widgets. Unknown facts
/// answer 503, never an empty home: "no widgets" on a blip would be a lie.
async fn widget_apps(
    user: &AuthenticatedUser,
    org: &str,
) -> Result<Vec<load::AppWidgets>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let caller = oxy_server_authz::Caller::from_user(user);
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(&db, &caller, false)
        .await
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let rows = load::openable_in_org(&db, &facts, org)
        .await
        .map_err(internal)?;
    load::with_widgets(&db, rows).await.map_err(internal)
}

#[derive(Debug, Deserialize)]
pub struct PressBody {
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub key: Option<String>,
}

/// `POST /api/home/widgets/{app_id}/{widget}/actions/{action}`
#[instrument(skip_all, fields(app = %app_id, widget = %widget, action = %action))]
pub async fn press(
    State(state): State<AppState>,
    runtime: Option<Extension<Arc<dyn FunctionRuntime>>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((app_id, widget, action)): Path<(Uuid, String, String)>,
    headers: HeaderMap,
    Json(body): Json<PressBody>,
) -> Response {
    // A write: production only, decided from the headers before anything is read.
    let path = format!("/api/home/widgets/{app_id}/{widget}/actions/{action}");
    if let Err(refused) =
        oxy_app_core::custom_app_env_request::guard(&Method::POST, &path, &headers)
    {
        return *refused;
    }
    if let Some(refused) = oversized(&body) {
        return refused;
    }
    let app = match pressed_app(&user, app_id).await {
        Ok(app) => app,
        Err(refused) => return refused,
    };
    let Some(def) = app.widgets.iter().find(|w| w.id == widget) else {
        return refusal(StatusCode::NOT_FOUND, "NotFound", "no such widget".into());
    };
    let Some(action_def) = def.action(&action) else {
        return refusal(
            StatusCode::CONFLICT,
            "ActionNotDeclared",
            format!("the app's live build does not declare `{action}` on `{widget}`"),
        );
    };
    let Some(Extension(runtime)) = runtime else {
        return refusal(
            StatusCode::BAD_GATEWAY,
            "FunctionFailed",
            "the function runtime is not available on this server".into(),
        );
    };
    let caller = Caller {
        headers: &headers,
        preagg: preagg(&state),
        runtime: runtime.as_ref(),
    };
    let press = Press {
        widget: def,
        action: action_def,
        input: body.input,
        key: body.key,
    };
    match run::press(&app.app, &press, &caller).await {
        Pressed::Done { message, view } => {
            Json(json!({ "ok": true, "message": message, "view": view })).into_response()
        }
        Pressed::Failed {
            status,
            error,
            message,
        } => refusal(status, &error, message),
    }
}

/// App `app_id` with its widgets, when the caller can open it. Anything else
/// is 404: an app outside their reach is not theirs to learn about.
async fn pressed_app(user: &AuthenticatedUser, app_id: Uuid) -> Result<load::AppWidgets, Response> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())?;
    let caller = oxy_server_authz::Caller::from_user(user);
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(&db, &caller, false)
        .await
        .ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())?;
    let not_found = || refusal(StatusCode::NOT_FOUND, "NotFound", "no such widget".into());
    let app = load::openable_app(&db, &facts, app_id)
        .await
        .map_err(|e| internal(e).into_response())?
        .ok_or_else(not_found)?;
    load::with_widgets(&db, vec![app])
        .await
        .map_err(|e| internal(e).into_response())?
        .pop()
        .ok_or_else(not_found)
}

fn oversized(body: &PressBody) -> Option<Response> {
    if body.input.to_string().len() > MAX_INPUT_BYTES {
        return Some(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "InputTooLarge",
            format!("an action's input is at most {MAX_INPUT_BYTES} bytes"),
        ));
    }
    if body
        .key
        .as_ref()
        .is_some_and(|k| k.chars().count() > MAX_KEY_CHARS)
    {
        return Some(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "KeyTooLong",
            format!("a button's key is at most {MAX_KEY_CHARS} characters"),
        ));
    }
    None
}

fn refusal(status: StatusCode, error: &str, message: String) -> Response {
    (
        status,
        Json(json!({ "ok": false, "error": error, "message": message })),
    )
        .into_response()
}

fn preagg(state: &AppState) -> PreaggCacheCtx {
    PreaggCacheCtx {
        cache: state.preagg_cache.clone(),
        renewal_threshold_secs: state.preagg_renewal_threshold_secs,
    }
}

fn internal(e: sea_orm::DbErr) -> StatusCode {
    tracing::error!("home widgets: database read failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests;
