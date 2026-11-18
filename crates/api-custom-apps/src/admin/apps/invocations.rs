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
//!
//! **A read can be narrowed to a window** (`?since=` / `?until=`, half-open),
//! a `?mode=` and `?outcome=failed`, all in the same `WHERE` before `LIMIT`.
//! The app-wide route then also answers how many rows the window holds in all
//! ([`WindowCounts`]): the list stays capped, the count does not, so the
//! console can say "the newest 200 of 1,412" instead of passing the cap off
//! as the total.

use std::collections::HashMap;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use entity::prelude::{AppBuilds, AppFunctionInvocations};
use entity::{app_builds, app_function_invocations, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select,
};
use serde::Serialize;
use uuid::Uuid;

use super::environment_scope::{self, Caller, ScopeError};
use crate::custom_apps_env_resolve::non_production_environment_serving;

/// Rows returned when the caller names no `limit`.
pub(crate) const DEFAULT_LIMIT: u64 = 50;
/// The most rows one read returns. The audit table accumulates across builds
/// and environments, so the cap is a hard bound, not a default.
pub(crate) const MAX_LIMIT: u64 = 200;

/// `?function=&environment=&build=&mode=&outcome=&since=&until=&limit=` on an
/// invocation listing.
#[derive(Debug, Default, serde::Deserialize)]
pub struct InvocationQuery {
    pub function: Option<String>,
    pub environment: Option<String>,
    /// The publish's build id (`app_builds.build_id`) or the build's UUID.
    pub build: Option<String>,
    pub limit: Option<u64>,
    /// The recorded `mode`, verbatim (`route`, `schedule`, `manual`, …).
    pub mode: Option<String>,
    /// `failed` — only the invocations the platform counted as failures
    /// ([`counted_as_failure`]); `all`, the default, every one. Anything else
    /// is `400 invalid_outcome`.
    pub outcome: Option<String>,
    /// RFC 3339 instants: rows created at or after `since` and before
    /// `until`. Malformed, or `since` not before `until`, is
    /// `400 invalid_window`.
    pub since: Option<String>,
    pub until: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InvocationList {
    pub invocations: Vec<InvocationSummary>,
    /// How many rows the window holds in all, when the read named a `since`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowCounts>,
}

/// Every row a windowed read matched, not only the `limit` it returned.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct WindowCounts {
    /// Rows matching every filter of the read, in its window.
    pub total: u64,
    /// Of those, the ones the platform counted as failures.
    pub failed: u64,
}

/// One recorded invocation (route / schedule / airway / manual job).
#[derive(Debug, Serialize)]
pub struct InvocationSummary {
    pub id: Uuid,
    pub function_name: String,
    /// `"route"` (HTTP) | `"schedule"` (cron fire) | `"manual"` (run-now / API
    /// job) | `"webhook"` | `"event"` | `"call"` (another app's
    /// `ctx.apps.call`, or a `ctx.apps.send` delivered) | `"airway"`.
    pub mode: String,
    /// The invocation that made this one: the caller's, for mode `call`.
    /// Absent for every other run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_invocation_id: Option<Uuid>,
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
    /// How it failed — `threw` | `internal` | `exceeded_memory` | `platform` |
    /// `timeout` | `http_5xx` | `host_call` — as stored with the failure. `None`
    /// for an invocation that did not fail, and for a failure recorded before
    /// the kind was stored, whose kind is only in the start of `error`.
    pub error_kind: Option<String>,
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
    failure_fingerprint.is_some() || FAILED_STATUSES.contains(&status)
}

/// The statuses that are failures with or without a fingerprint.
const FAILED_STATUSES: [&str; 2] = ["error", "timeout"];

/// [`counted_as_failure`] as a `WHERE` clause: `?outcome=failed` and the
/// window's `failed` count select exactly the rows the DTO marks `failed`.
fn failed_rows() -> sea_orm::Condition {
    sea_orm::Condition::any()
        .add(app_function_invocations::Column::FailureFingerprint.is_not_null())
        .add(app_function_invocations::Column::Status.is_in(FAILED_STATUSES))
}

/// What `?mode=`, `?outcome=`, `?since=` and `?until=` narrow a read to,
/// checked before anything is read.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Narrowing {
    mode: Option<String>,
    failed_only: bool,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

impl Narrowing {
    pub(crate) fn of(q: &InvocationQuery) -> Result<Self, ScopeError> {
        let failed_only = match q.outcome.as_deref().map(str::trim) {
            None | Some("") | Some("all") => false,
            Some("failed") => true,
            Some(other) => {
                return Err(ScopeError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_outcome",
                    format!("outcome is \"failed\" or \"all\"; got {other:?}"),
                ));
            }
        };
        let since = instant("since", q.since.as_deref())?;
        let until = instant("until", q.until.as_deref())?;
        if let (Some(since), Some(until)) = (since, until)
            && since >= until
        {
            return Err(window_refused(format!(
                "since ({}) must be before until ({})",
                since.to_rfc3339(),
                until.to_rfc3339()
            )));
        }
        let mode = q.mode.as_deref().map(str::trim).filter(|m| !m.is_empty());
        Ok(Self {
            mode: mode.map(str::to_owned),
            failed_only,
            since,
            until,
        })
    }
}

/// One bound of the window, when the query names it.
fn instant(name: &str, raw: Option<&str>) -> Result<Option<DateTime<Utc>>, ScopeError> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw)
        .map(|at| Some(at.with_timezone(&Utc)))
        .map_err(|e| {
            window_refused(format!(
                "{name} must be an RFC 3339 instant such as 2026-11-17T09:00:00Z; got {raw:?}: {e}"
            ))
        })
}

fn window_refused(message: String) -> ScopeError {
    ScopeError::new(StatusCode::BAD_REQUEST, "invalid_window", message)
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
        parent_invocation_id: row.parent_invocation_id,
        environment: row.environment,
        build_id: build_label,
        build_uuid: row.build_id,
        failed: counted_as_failure(&row.status, row.failure_fingerprint.as_deref()),
        status: row.status,
        result_status: row.result_status,
        duration_ms: row.duration_ms,
        error: row.error,
        error_kind: row.error_kind,
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
    // A sandbox agent token must name the environment, and it must be a
    // sandbox the token created. With none named it would otherwise fall to
    // "production's rows", and a named build would lift the environment
    // filter: neither is the token's to read.
    if super::agent_scope::is_agent(caller.user) {
        let environment = environment_scope::parse(q.environment.as_deref())?;
        environment_scope::admit_to(db, app, caller, &environment).await?;
        return Ok(Some(environment));
    }
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
/// a named build that production does not run. Decided by the resolver every
/// reader of an environment asks (`custom_apps_env_resolve`), not by a second
/// rule here — "production runs it" is the function runtime's own answer, so
/// a draft a sandbox agent token published on an app unpublished since names
/// staging, where a person's draft there is production's fallback. `None`
/// when production runs them, or no environment serves them.
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
/// `400 invalid_limit`. `mode`, `outcome`, `since`, `until`: [`Narrowing`].
pub(crate) async fn list(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    q: &InvocationQuery,
) -> Result<Vec<InvocationSummary>, ScopeError> {
    Ok(read(db, app, caller, q, false).await?.invocations)
}

/// [`list`], and — when `count` is set and the read names a `since` — how
/// many rows its window holds in all.
async fn read(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    q: &InvocationQuery,
    count: bool,
) -> Result<InvocationList, ScopeError> {
    let limit = parse_limit(q.limit)?;
    let narrowing = Narrowing::of(q)?;
    let named = match q.build.as_deref().filter(|b| !b.is_empty()) {
        Some(raw) => Some(builds_named(db, app.id, raw).await?),
        None => None,
    };
    // In the query, before `LIMIT`: a page is `limit` rows the caller may read.
    let scope = row_scope(db, app, caller, q, named.as_deref().unwrap_or_default()).await?;
    let mut rows = RowFilter::default();
    if let Some(environment) = scope {
        // A sandbox agent token reads the sandbox it has now, not an earlier
        // one that had the name (`custom_apps_sandbox_instance`).
        let agent = super::agent_scope::instance_since(db, app, caller.user, &environment);
        rows.since = agent.await?;
        rows.environment = Some(environment.name());
    }
    let windowed = count && narrowing.since.is_some();
    let Some(select) = filtered(app.id, q, &narrowing, named, rows) else {
        return Ok(InvocationList {
            invocations: Vec::new(),
            window: windowed.then(WindowCounts::default),
        });
    };
    let window = if windowed {
        Some(window_counts(db, &select).await?)
    } else {
        None
    };
    let invocations = fetch(db, app.id, select, limit).await?;
    Ok(InvocationList {
        invocations,
        window,
    })
}

/// The non-production environment a `?build=` names, when only one serves
/// it: what [`row_scope`] reads a named build as on the console, for a read
/// that must ask the caller about that environment before listing it. `None`
/// when production runs the build, or nothing serves it.
pub(crate) async fn environment_of_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    raw: &str,
) -> Result<Option<AppEnvironment>, ScopeError> {
    let builds = builds_named(db, app.id, raw).await?;
    environment_of(db, app, &builds).await
}

/// `app`'s invocations in the environment a caller was already admitted to
/// (`None`: every environment), newest first: the slug-addressed read
/// (`crate::logs::invocations`), whose access check is the logs route's and
/// was made before this call. `function`, `build` and `limit` mean what they
/// mean on [`list`]; the query's own `environment` is not read here.
pub(crate) async fn list_in_environment(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: Option<String>,
    q: &InvocationQuery,
) -> Result<Vec<InvocationSummary>, ScopeError> {
    let limit = parse_limit(q.limit)?;
    let narrowing = Narrowing::of(q)?;
    let named = match q.build.as_deref().filter(|b| !b.is_empty()) {
        Some(raw) => Some(builds_named(db, app_id, raw).await?),
        None => None,
    };
    let rows = RowFilter {
        environment,
        since: None,
    };
    match filtered(app_id, q, &narrowing, named, rows) {
        Some(select) => fetch(db, app_id, select, limit).await,
        None => Ok(Vec::new()),
    }
}

/// Which rows of the app a read may return: one environment's (`None` is
/// every environment's), from `since` on when set.
#[derive(Default)]
struct RowFilter {
    environment: Option<String>,
    since: Option<chrono::DateTime<chrono::Utc>>,
}

/// The invocation query both listings run, filtered in SQL — the caller's
/// rows, `?function=`, `?build=` and every [`Narrowing`] — before any `LIMIT`
/// or count. `named` is the builds `?build=` named (`None` when it named
/// none); `None` back when that set is empty, which matches nothing.
fn filtered(
    app_id: Uuid,
    q: &InvocationQuery,
    narrowing: &Narrowing,
    named: Option<Vec<Uuid>>,
    rows: RowFilter,
) -> Option<Select<AppFunctionInvocations>> {
    use app_function_invocations::Column;
    let mut find = AppFunctionInvocations::find().filter(Column::AppId.eq(app_id));
    if let Some(function) = q.function.as_deref().filter(|f| !f.is_empty()) {
        find = find.filter(Column::FunctionName.eq(function));
    }
    if let Some(environment) = rows.environment {
        find = find.filter(Column::Environment.eq(environment));
    }
    if let Some(since) = rows.since {
        find = find.filter(Column::CreatedAt.gte(since));
    }
    if let Some(builds) = named {
        if builds.is_empty() {
            return None;
        }
        find = find.filter(Column::BuildId.is_in(builds));
    }
    if let Some(mode) = &narrowing.mode {
        find = find.filter(Column::Mode.eq(mode.as_str()));
    }
    if narrowing.failed_only {
        find = find.filter(failed_rows());
    }
    // Half-open, so two adjacent windows never count one row twice.
    if let Some(since) = narrowing.since {
        find = find.filter(Column::CreatedAt.gte(since));
    }
    if let Some(until) = narrowing.until {
        find = find.filter(Column::CreatedAt.lt(until));
    }
    Some(find)
}

/// How many rows `select` matches, and how many of those failed: two
/// `COUNT(*)`s over the `(app_id, environment, created_at)` index.
async fn window_counts(
    db: &DatabaseConnection,
    select: &Select<AppFunctionInvocations>,
) -> Result<WindowCounts, ScopeError> {
    let counted = |e: sea_orm::DbErr| ScopeError::internal("invocation count failed", e);
    let total = select.clone().count(db).await.map_err(counted)?;
    let failed = select.clone().filter(failed_rows()).count(db);
    Ok(WindowCounts {
        total,
        failed: failed.await.map_err(counted)?,
    })
}

/// The newest `limit` rows of `find`, as DTOs.
async fn fetch(
    db: &DatabaseConnection,
    app_id: Uuid,
    find: Select<AppFunctionInvocations>,
    limit: u64,
) -> Result<Vec<InvocationSummary>, ScopeError> {
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
            Narrowing::of(q)?;
            environment_scope::parse(q.environment.as_deref())?;
            Ok(Vec::new())
        }
    }
}

/// `GET /admin/apps/{id}/invocations` — every function's invocations for the
/// app, newest first, filtered by `?function=`, `?environment=`, `?build=`,
/// `?mode=`, `?outcome=` and the window `?since=` / `?until=`, and bounded by
/// `?limit=`. A read that names `since` also answers `window`: how many rows
/// it holds past the cap. A non-production row is returned only to a caller
/// with reach ([`row_scope`]).
pub async fn list_app_invocations(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    query: Result<Query<InvocationQuery>, QueryRejection>,
) -> Result<Json<InvocationList>, ScopeError> {
    let Query(q) = query.map_err(rejected)?;
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, id).await?;
    let caller = Caller::new(&user, marker.as_ref());
    Ok(Json(read(&db, &app, &caller, &q, true).await?))
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

    fn query(pairs: &str) -> InvocationQuery {
        serde_urlencoded::from_str(pairs).expect("parses")
    }

    /// `outcome` is `failed` or `all`, and nothing else; `mode` is read
    /// verbatim, and blank means unset.
    #[test]
    fn outcome_and_mode_narrow_or_are_refused() {
        assert_eq!(Narrowing::of(&query("")).unwrap(), Narrowing::default());
        assert!(!Narrowing::of(&query("outcome=all")).unwrap().failed_only);
        assert!(Narrowing::of(&query("outcome=failed")).unwrap().failed_only);
        for refused in ["error", "FAILED", "true"] {
            let e = Narrowing::of(&query(&format!("outcome={refused}"))).expect_err(refused);
            assert_eq!(
                (e.status, e.code),
                (StatusCode::BAD_REQUEST, "invalid_outcome")
            );
        }
        let mode = Narrowing::of(&query("mode=schedule")).unwrap().mode;
        assert_eq!(mode.as_deref(), Some("schedule"));
        assert_eq!(Narrowing::of(&query("mode=")).unwrap().mode, None);
    }

    /// A window is two RFC 3339 instants, `since` strictly before `until`;
    /// either may be absent.
    #[test]
    fn a_window_is_two_ordered_instants() {
        let both = Narrowing::of(&query(
            "since=2026-11-16T09:00:00Z&until=2026-11-17T09:00:00Z",
        ))
        .unwrap();
        assert_eq!(
            both.since.map(|t| t.to_rfc3339()).as_deref(),
            Some("2026-11-16T09:00:00+00:00")
        );
        assert!(both.until.is_some());
        assert!(
            Narrowing::of(&query("since=2026-11-16T09:00:00Z"))
                .unwrap()
                .until
                .is_none()
        );
        for refused in [
            "since=yesterday",
            "until=2026-11-17",
            "since=2026-11-17T09:00:00Z&until=2026-11-17T09:00:00Z",
            "since=2026-11-18T09:00:00Z&until=2026-11-17T09:00:00Z",
        ] {
            let e = Narrowing::of(&query(refused)).expect_err(refused);
            assert_eq!(
                (e.status, e.code),
                (StatusCode::BAD_REQUEST, "invalid_window"),
                "{refused}"
            );
        }
    }

    /// Every narrowing lands in the `WHERE`, before the `LIMIT` — so a page of
    /// failures is filled with failures rather than cut from a mixed page —
    /// and `failed` is the DTO's own rule.
    #[test]
    fn narrowing_is_in_the_query() {
        let q = query(
            "function=sync&mode=schedule&outcome=failed\
             &since=2026-11-16T09:00:00Z&until=2026-11-17T09:00:00Z",
        );
        let narrowing = Narrowing::of(&q).unwrap();
        let rows = RowFilter {
            environment: Some("staging".into()),
            since: None,
        };
        let select = filtered(Uuid::nil(), &q, &narrowing, None, rows).expect("a query");
        let sql = sea_orm::QueryTrait::build(&select, sea_orm::DbBackend::Postgres).to_string();
        for clause in [
            r#""function_name" = 'sync'"#,
            r#""environment" = 'staging'"#,
            r#""mode" = 'schedule'"#,
            r#""failure_fingerprint" IS NOT NULL OR "app_function_invocations"."status" IN ('error', 'timeout')"#,
            r#""created_at" >= '2026-11-16 09:00:00.000000 +00:00'"#,
            r#""created_at" < '2026-11-17 09:00:00.000000 +00:00'"#,
        ] {
            assert!(sql.contains(clause), "{clause} missing from {sql}");
        }
        // A `?build=` that named none of this app's builds matches nothing.
        assert!(
            filtered(
                Uuid::nil(),
                &q,
                &narrowing,
                Some(vec![]),
                RowFilter::default()
            )
            .is_none()
        );
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
