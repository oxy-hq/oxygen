//! An app's recorded function invocations: the one query both listings read,
//! the DTO, and the app-wide listing.
//!
//! Two routes share [`list`]:
//!
//! - `GET /admin/apps/{id}/invocations` ([`list_app_invocations`]) — every
//!   function's invocations, which is what "what ran in my sandbox for this
//!   build" asks;
//! - `GET …/{id}/functions/{name}/invocations`
//!   (`functions::list_invocations`) — one function's, as a bare array.
//!
//! **Every read is scoped to the app in the path.** The app id is in the
//! `WHERE` of the invocation query and of the build lookup, so naming another
//! app's build — by its label or by its UUID — matches nothing; and the
//! mount's `enforce_app_scope` has already refused an app outside the
//! caller's grant. DB-only (FleetOk), bounded by `limit`.
//!
//! **A non-production row is read only with reach.** A caller who may open
//! the app's non-production environments (oxy-authz `Action::AppNonProduction`,
//! and never a publish token) reads every environment's rows. Anyone else
//! reads production's, however the request arrives at the others
//! ([`row_scope`]): naming a non-production environment is refused, naming a
//! build only such an environment serves is naming that environment, and
//! naming neither lists production's rows alone — filtered in the query, so a
//! page is filled from them rather than cut short.

use std::collections::HashMap;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::prelude::{AppBuilds, AppFunctionInvocations};
use entity::{app_builds, app_function_invocations, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use super::environment_scope::{self, Caller, ScopeError};
use crate::server::api::custom_apps_env_resolve::non_production_environment_serving;

/// Rows returned when the caller names no `limit`.
pub(crate) const DEFAULT_LIMIT: u64 = 50;
/// The most rows one read returns. The audit table accumulates across builds
/// and environments, so the cap is a hard bound, not a default.
pub(crate) const MAX_LIMIT: u64 = 200;

/// `?function=&environment=&build=&limit=` on an invocation listing.
#[derive(Debug, Default, serde::Deserialize)]
pub struct InvocationQuery {
    pub function: Option<String>,
    pub environment: Option<String>,
    /// The publish's build id (`app_builds.build_id`) or the build's UUID.
    pub build: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct InvocationList {
    pub invocations: Vec<InvocationSummary>,
}

/// One recorded invocation (route / schedule / airway / manual job).
#[derive(Debug, Serialize)]
pub struct InvocationSummary {
    pub id: Uuid,
    pub function_name: String,
    /// `"route"` (HTTP) | `"schedule"` (cron fire) | `"manual"` (run-now / API
    /// job) | `"airway"`.
    pub mode: String,
    /// The app environment it ran in: `production`, `staging` or `dev-<handle>`.
    pub environment: String,
    /// The publish's build id of the build that ran. `None` only if the build
    /// row is gone, which the cascade on `app_builds` should make impossible.
    pub build_id: Option<String>,
    /// `app_builds.id` of the build that ran — what log lines carry as `build_id`.
    pub build_uuid: Uuid,
    /// `"running"` | `"success"` | `"error"` | `"cancelled"` | `"timeout"` |
    /// `"shed"` (the platform declined to start it — no concurrency permit).
    /// The recorded outcome, verbatim: `success` means only that the handler
    /// returned without throwing, so read `failed` for whether it worked.
    pub status: String,
    /// The platform counted this invocation as a failure — see
    /// `counted_as_failure`. True for a `success` that answered 5xx or caught
    /// a failed `ctx.*` call, which `status` alone reports as a success.
    pub failed: bool,
    /// The HTTP status the function answered. Stored only beside a kept
    /// response body (keyed route calls), so usually `None`.
    pub result_status: Option<i16>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    /// A stored response body is available (kept for keyed route calls).
    pub has_result: bool,
}

/// Whether the platform counted an invocation as a failure: the rule
/// `custom_apps_functions::failure_signal::Failure::of` applies at
/// finalization, read back from the row it finalized. Every invocation that
/// rule counts gets a `failure_fingerprint` — including a `success` that
/// answered 5xx or caught a failed host call — so the fingerprint is the
/// signal; `error` / `timeout` also count on their own, for rows written
/// before the column existed. `result_status` can't stand in for it: it is
/// only stored for keyed calls. `cancelled`, `shed` and `running` are not
/// failures.
pub(crate) fn counted_as_failure(status: &str, failure_fingerprint: Option<&str>) -> bool {
    failure_fingerprint.is_some() || matches!(status, "error" | "timeout")
}

/// A query string that does not deserialize. `limit` is the only typed
/// parameter, so a malformed one (`limit=abc`, `limit=-1`) is what this is.
pub(crate) fn rejected(rejection: QueryRejection) -> ScopeError {
    ScopeError::new(
        StatusCode::BAD_REQUEST,
        "invalid_limit",
        format!("limit must be a whole number between 1 and {MAX_LIMIT}: {rejection}"),
    )
}

/// `limit`, defaulted and bounded. Out of range is an error rather than a
/// clamp: a caller who asked for 5000 rows and got 200 would read a truncated
/// history as the whole one.
pub(crate) fn parse_limit(limit: Option<u64>) -> Result<u64, ScopeError> {
    match limit {
        None => Ok(DEFAULT_LIMIT),
        Some(n) if (1..=MAX_LIMIT).contains(&n) => Ok(n),
        Some(n) => Err(ScopeError::new(
            StatusCode::BAD_REQUEST,
            "invalid_limit",
            format!("limit must be between 1 and {MAX_LIMIT}; got {n}"),
        )),
    }
}

/// Which builds of `app_id` `raw` names: the build with that publish id, and,
/// when `raw` is a UUID, the build with that id. Empty when it names none —
/// and always scoped to `app_id`, so another app's build is none.
async fn builds_named(
    db: &DatabaseConnection,
    app_id: Uuid,
    raw: &str,
) -> Result<Vec<Uuid>, ScopeError> {
    let mut named = sea_orm::Condition::any().add(app_builds::Column::BuildId.eq(raw));
    if let Ok(uuid) = raw.parse::<Uuid>() {
        named = named.add(app_builds::Column::Id.eq(uuid));
    }
    let builds = AppBuilds::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(named)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("app_builds lookup failed", e))?;
    Ok(builds.into_iter().map(|b| b.id).collect())
}

/// The publish's build id of each build `rows` ran, in one query.
async fn build_labels(
    db: &DatabaseConnection,
    app_id: Uuid,
    rows: &[app_function_invocations::Model],
) -> Result<HashMap<Uuid, String>, ScopeError> {
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: std::collections::BTreeSet<Uuid> = rows.iter().map(|r| r.build_id).collect();
    let builds = AppBuilds::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("app_builds lookup failed", e))?;
    Ok(builds.into_iter().map(|b| (b.id, b.build_id)).collect())
}

pub(crate) fn summary(
    row: app_function_invocations::Model,
    build_label: Option<String>,
) -> InvocationSummary {
    InvocationSummary {
        id: row.id,
        function_name: row.function_name,
        mode: row.mode,
        environment: row.environment,
        build_id: build_label,
        build_uuid: row.build_id,
        failed: counted_as_failure(&row.status, row.failure_fingerprint.as_deref()),
        status: row.status,
        result_status: row.result_status,
        duration_ms: row.duration_ms,
        error: row.error,
        created_at: row.created_at.to_rfc3339(),
        has_result: row.result_body.is_some(),
    }
}

/// The one environment whose rows the listing returns to `caller`, or `None`
/// for every environment's.
///
/// - **An environment is named.** A name `AppEnvironment::parse` rejects is
///   `400 invalid_environment`. Production passes; anything else is
///   `environment_scope::admit_to`: `403 publish_token_refused` for a publish
///   token, `403 non_production_refused` for a caller without reach.
/// - **A build is named** (`builds`) that only a non-production environment
///   serves ([`environment_of`]). That names the environment: the same two
///   refusals, and for a caller with reach every row of the build.
/// - **Neither is named.** A caller with reach reads every environment's
///   rows; a publish token and a caller without reach read production's.
async fn row_scope(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    q: &InvocationQuery,
    builds: &[Uuid],
) -> Result<Option<AppEnvironment>, ScopeError> {
    if let Some(raw) = q.environment.as_deref().filter(|e| !e.trim().is_empty()) {
        let environment = environment_scope::parse(Some(raw))?;
        environment_scope::admit_to(db, app, caller, &environment).await?;
        return Ok(Some(environment));
    }
    if let Some(environment) = environment_of(db, app, builds).await? {
        environment_scope::admit_to(db, app, caller, &environment).await?;
        return Ok(None);
    }
    if caller.has_reach(db, app).await {
        Ok(None)
    } else {
        Ok(Some(AppEnvironment::Production))
    }
}

/// The non-production environment `builds` name: staging or a sandbox serving
/// a named build that production does not serve. Decided by the resolver
/// every reader of an environment asks (`custom_apps_env_resolve`), not by a
/// second rule here. `None` when production serves them, or no environment
/// does.
async fn environment_of(
    db: &DatabaseConnection,
    app: &apps::Model,
    builds: &[Uuid],
) -> Result<Option<AppEnvironment>, ScopeError> {
    non_production_environment_serving(db, app, builds)
        .await
        .map_err(|e| ScopeError::internal("environment lookup failed", e))
}

/// `app`'s invocations matching `q`, newest first, for `caller`.
///
/// `environment`: see [`row_scope`]. `build`: an unknown one is an empty
/// list, not an error. `limit`: 1–200, default 50; out of range is
/// `400 invalid_limit`.
pub(crate) async fn list(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    q: &InvocationQuery,
) -> Result<Vec<InvocationSummary>, ScopeError> {
    let app_id = app.id;
    let limit = parse_limit(q.limit)?;
    let mut find =
        AppFunctionInvocations::find().filter(app_function_invocations::Column::AppId.eq(app_id));
    if let Some(function) = q.function.as_deref().filter(|f| !f.is_empty()) {
        find = find.filter(app_function_invocations::Column::FunctionName.eq(function));
    }
    let named = match q.build.as_deref().filter(|b| !b.is_empty()) {
        Some(raw) => Some(builds_named(db, app_id, raw).await?),
        None => None,
    };
    // In the query, before `LIMIT`: a page is `limit` rows the caller may read.
    let scope = row_scope(db, app, caller, q, named.as_deref().unwrap_or_default()).await?;
    if let Some(environment) = scope {
        find = find.filter(app_function_invocations::Column::Environment.eq(environment.name()));
    }
    if let Some(builds) = named {
        if builds.is_empty() {
            return Ok(Vec::new());
        }
        find = find.filter(app_function_invocations::Column::BuildId.is_in(builds));
    }
    // `find()` selects every column, so `failure_fingerprint` and
    // `result_status` are on each row for `failed` / `result_status`.
    let rows = find
        .order_by_desc(app_function_invocations::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("invocation query failed", e))?;
    let labels = build_labels(db, app_id, &rows).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let label = labels.get(&row.build_id).cloned();
            summary(row, label)
        })
        .collect())
}

/// [`list`] for the app `app_id` names. An app that does not exist has no
/// invocations: the empty list the per-function route has always answered
/// for one, once the query itself is well-formed.
pub(crate) async fn list_of(
    db: &DatabaseConnection,
    app_id: Uuid,
    caller: &Caller<'_>,
    q: &InvocationQuery,
) -> Result<Vec<InvocationSummary>, ScopeError> {
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| ScopeError::internal("app lookup failed", e))?;
    match app {
        Some(app) => list(db, &app, caller, q).await,
        None => {
            parse_limit(q.limit)?;
            environment_scope::parse(q.environment.as_deref())?;
            Ok(Vec::new())
        }
    }
}

/// `GET /admin/apps/{id}/invocations` — every function's invocations for the
/// app, newest first, filtered by `?function=`, `?environment=`, `?build=`
/// and bounded by `?limit=`. A non-production row is returned only to a
/// caller with reach ([`row_scope`]).
pub async fn list_app_invocations(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    query: Result<Query<InvocationQuery>, QueryRejection>,
) -> Result<Json<InvocationList>, ScopeError> {
    let Query(q) = query.map_err(rejected)?;
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, id).await?;
    let invocations = list(&db, &app, &Caller::new(&user, marker.as_ref()), &q).await?;
    Ok(Json(InvocationList { invocations }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Sep 2026 warehouse incident: a function that caught every refused
    /// write and answered its own 500 was recorded `success`, and the history
    /// showed a week of green. `failed` must agree with the pager instead.
    #[test]
    fn failed_mirrors_what_the_pager_counted() {
        // A clean success is not a failure.
        assert!(!counted_as_failure("success", None));
        // A success the pager fingerprinted (answered 5xx, or caught a failed
        // ctx call) is — the case `status` alone reports as green.
        assert!(counted_as_failure("success", Some("af597dfd3a7536b8")));
        // Errors and timeouts count with or without a fingerprint (rows from
        // before the column carry none).
        assert!(counted_as_failure("error", None));
        assert!(counted_as_failure("error", Some("af597dfd3a7536b8")));
        assert!(counted_as_failure("timeout", None));
        // Cancelled, shed and in-flight invocations are not failures.
        assert!(!counted_as_failure("cancelled", None));
        assert!(!counted_as_failure("shed", None));
        assert!(!counted_as_failure("running", None));
    }

    /// A read is always bounded, and an out-of-range bound is refused rather
    /// than quietly clamped.
    #[test]
    fn invocation_limit_is_bounded_and_out_of_range_is_refused() {
        assert_eq!(parse_limit(None).unwrap(), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some(1)).unwrap(), 1);
        assert_eq!(parse_limit(Some(MAX_LIMIT)).unwrap(), MAX_LIMIT);
        for refused in [0, MAX_LIMIT + 1, u64::MAX] {
            let e = parse_limit(Some(refused)).expect_err("out of range");
            assert_eq!(
                (e.status, e.code),
                (StatusCode::BAD_REQUEST, "invalid_limit"),
                "{refused}"
            );
        }
    }
}
