//! Read side of the custom-app **Oxy Functions** admin surface: list an app's
//! functions + their manifest config, their recent invocation history, and a
//! single function-job run's status + persisted logs. Powers the AppDetail
//! "Functions" section (manage / debug). The write side — triggering a job — is
//! `handlers::run_function_job` (`POST .../functions/{name}/runs`).
//!
//! All handlers are admin-gated at the router layer and DB-only (FleetOk): they
//! read `app_functions` (the per-build registry), `app_function_invocations`
//! (the invocation audit), and — for a job run — `agentic_runs` +
//! `agentic_run_events` (the `function_log` lines Slice A persists).
//! See internal-docs/customer-apps-functions.md.

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::prelude::AppFunctions;
use entity::{app_functions, apps};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use uuid::Uuid;

use super::environment_scope::{self, Caller, EnvironmentQuery, RouteError, ScopeError};
use super::function_run;
pub use super::function_run::{FunctionRunDetail, FunctionRunLogLine};
pub use super::invocations::InvocationSummary;
use super::invocations::{self, InvocationQuery};

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// One function in the app's active build, projected from its manifest.
#[derive(Debug, Serialize)]
pub struct FunctionSummary {
    pub name: String,
    /// Whether the function is HTTP-invocable as the runtime serves it — true
    /// unless the manifest sets `route: false` (even a scheduled function is
    /// callable unless it opts out).
    pub route: bool,
    /// Cron expression when the function declares a schedule.
    pub schedule: Option<String>,
    pub timezone: Option<String>,
    /// The function is wired as an Airway pipeline transform step.
    pub airway: bool,
    pub timeout_seconds: Option<u32>,
    /// Marked "check": true — run by `oxyc checks run`.
    pub check: bool,
    /// Background-run retry policy, when declared (`maxAttempts > 1`).
    pub retries: Option<RetriesSummary>,
    /// The function may write app-scoped secrets via `ctx.secrets.set`.
    pub secrets_write: bool,
    /// Databases the function may write to via `ctx.warehouse`.
    pub destinations: Vec<String>,
    /// Author-declared example input (manifest `inputExample`) — a sample JSON
    /// body the "Run now" surface prefills so an operator knows what params the
    /// function expects. `None` when the function declares none.
    pub input_example: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct RetriesSummary {
    pub max_attempts: u32,
    pub min_timeout_ms: Option<u64>,
    pub max_timeout_ms: Option<u64>,
}

/// A single function-job run's status + persisted logs, for the trigger-and-watch
/// debug loop. Reads the run row + its `function_log` events.
// ── Manifest projection (read-only) ──────────────────────────────────────────
//
// A decoupled view of the per-function `manifest_json` — just the fields the
// admin surface displays. The authoritative parse lives in
// `custom_apps_functions`; this projection keeps the admin module independent
// of the runtime host.

#[derive(Debug, serde::Deserialize, Default)]
struct ManifestView {
    #[serde(default)]
    route: Option<bool>,
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default, rename = "timeoutSeconds")]
    timeout_seconds: Option<u32>,
    #[serde(default)]
    check: Option<bool>,
    #[serde(default, rename = "airwayStep")]
    airway_step: Option<serde_json::Value>,
    #[serde(default)]
    retries: Option<RetriesView>,
    #[serde(default)]
    secrets: Option<SecretsView>,
    #[serde(default)]
    destinations: Option<Vec<String>>,
    #[serde(default, rename = "inputExample")]
    input_example: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct RetriesView {
    #[serde(rename = "maxAttempts")]
    max_attempts: Option<u32>,
    #[serde(rename = "minTimeoutMs")]
    min_timeout_ms: Option<u64>,
    #[serde(rename = "maxTimeoutMs")]
    max_timeout_ms: Option<u64>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct SecretsView {
    #[serde(default)]
    write: Option<bool>,
}

fn to_summary(name: String, manifest: Option<&serde_json::Value>) -> FunctionSummary {
    let m: ManifestView = manifest
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let has_airway = m.airway_step.is_some();
    // Reflect what the runtime actually serves, not just the author's intent: the
    // `/fn/<name>` handler rejects only an explicit `route: false`, so a function
    // is HTTP-invocable unless it opts out — even a schedule-only one. Showing the
    // badge on that case is the honest signal for a debug surface (the operator
    // sees the function is publicly callable). Distinct from the SDK validator's
    // "effective route" (intent), which defaults off when another surface exists.
    let route = m.route != Some(false);
    let retries = m.retries.and_then(|r| {
        r.max_attempts.filter(|&a| a > 1).map(|a| RetriesSummary {
            max_attempts: a,
            min_timeout_ms: r.min_timeout_ms,
            max_timeout_ms: r.max_timeout_ms,
        })
    });
    FunctionSummary {
        name,
        route,
        schedule: m.schedule,
        timezone: m.timezone,
        airway: has_airway,
        timeout_seconds: m.timeout_seconds,
        check: m.check.unwrap_or(false),
        retries,
        secrets_write: m.secrets.and_then(|s| s.write).unwrap_or(false),
        destinations: m.destinations.unwrap_or_default(),
        input_example: m.input_example,
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// The build whose functions `?environment=<raw>` asks for, for a caller who
/// may name that environment: the build it serves now, or `None` when it
/// serves nothing.
async fn environment_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    marker: Option<&AppPublishTokenAuth>,
    raw: &str,
) -> Result<Option<Uuid>, ScopeError> {
    let environment = environment_scope::resolve(db, app, user, marker, Some(raw)).await?;
    let resolved = crate::server::api::custom_apps_env_resolve::resolve_function_environment(
        db,
        app,
        &environment,
    )
    .await
    .map_err(|e| ScopeError::internal("environment lookup failed", e))?;
    Ok(resolved.build_id)
}

/// `GET /admin/apps/{id}/functions` — the app's functions in its active build
/// (published, else draft), with each one's manifest config. Empty when the app
/// has no build or ships no functions.
///
/// `?environment=<name>` lists the functions of the build **that environment**
/// serves instead — what a check run or a call there would actually execute.
/// Naming a non-production environment needs reach and refuses a publish
/// token (`environment_scope::resolve`).
pub async fn list_functions(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Query(q): Query<EnvironmentQuery>,
) -> Result<Json<Vec<FunctionSummary>>, RouteError> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_functions DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let app = apps::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let build_id = match q.environment.as_deref() {
        None => app.published_build_id.or(app.draft_build_id),
        Some(raw) => {
            let marker = marker.as_ref().map(|axum::Extension(marker)| marker);
            environment_build(&db, &app, &user, marker, raw).await?
        }
    };
    let Some(build_id) = build_id else {
        return Ok(Json(vec![]));
    };
    let rows = AppFunctions::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .order_by_asc(app_functions::Column::Name)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("list_functions query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let out = rows
        .into_iter()
        .map(|r| to_summary(r.name, r.manifest_json.as_ref()))
        .collect();
    Ok(Json(out))
}

/// `GET /admin/apps/{id}/functions/{name}/invocations` — the most recent
/// invocations of one function across all its builds (newest first), for the
/// debug history table. `?environment=`, `?build=` and `?limit=` narrow it as
/// they do the app-wide listing (`invocations::list`), which this is with the
/// path's function; it keeps answering a bare array. A non-production row is
/// returned only to a caller with reach, as it is there.
pub async fn list_invocations(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
    query: Result<Query<InvocationQuery>, QueryRejection>,
) -> Result<Json<Vec<InvocationSummary>>, RouteError> {
    let Query(q) = query.map_err(invocations::rejected)?;
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_invocations DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let q = InvocationQuery {
        function: Some(name),
        ..q
    };
    let caller = Caller::new(&user, marker.as_ref());
    Ok(Json(invocations::list_of(&db, id, &caller, &q).await?))
}

/// `GET /admin/apps/{id}/function-runs/{run_id}` — a single function-job run's
/// status + persisted `function_log` output, for watching a just-triggered run.
/// Verifies the run is an `app_function` run for this app before returning
/// anything, so the app-scoped path can't read an unrelated run; a run queued
/// outside production is read only by a caller with reach (`function_run`).
pub async fn get_function_run(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path((id, run_id)): Path<(Uuid, String)>,
) -> Result<Json<FunctionRunDetail>, RouteError> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("get_function_run DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let caller = Caller::new(&user, marker.as_ref());
    Ok(Json(function_run::detail(&db, id, &run_id, &caller).await?))
}

#[cfg(test)]
#[path = "functions_tests.rs"]
mod tests;
