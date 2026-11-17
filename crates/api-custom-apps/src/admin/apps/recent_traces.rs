//! The latest recorded call traces of every function in one environment,
//! digested to what each run touched: the right-hand side of the console's
//! app map ("what does this app reach?").
//!
//! `GET /admin/apps/{id}/recent-traces?environment=<name>&per_function=<n>` —
//! for each function of the build that environment serves (the build the
//! Functions panel lists), its newest `n` invocations **that have a recorded
//! trace** (default 5, at most 20), newest first. Each run is the distinct
//! shapes of its `ctx.*` calls — family, op, verb, target, status — with how
//! many calls had that shape. One request, where the console would otherwise
//! list each function's invocations and read up to `n` traces apiece.
//!
//! **What it may return is what a trace already holds** (`oxy_custom_apps::
//! call_trace`): never SQL, values, URL paths, bodies or secret values. The
//! timings are left out; the waterfall is the place for them.
//!
//! **Fences.** The mount's staff guards, then `environment_scope::resolve`: a
//! non-production environment needs reach and refuses a publish token. A
//! sandbox agent token, were it to reach this route, reads its own sandbox's
//! current instance only (`agent_scope::instance_since`).
//!
//! **Bounds.** One statement: a lateral probe per function, at most `n` rows
//! each, inside the trace retention window (older traces are pruned anyway).
//! DB-only (FleetOk).

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::environment_scope::{self, ScopeError};
use crate::custom_apps_call_trace::{CallRecord, CallStatus, calls_of};
use crate::custom_apps_call_trace_store as store;
use crate::custom_apps_env_resolve::{load_environment_builds, resolve_function_environment};

/// Runs per function when the caller names none.
pub(crate) const DEFAULT_PER_FUNCTION: u64 = 5;
/// The most runs per function one read returns.
pub(crate) const MAX_PER_FUNCTION: u64 = 20;

/// `?environment=&per_function=`.
#[derive(Debug, Default, Deserialize)]
pub struct RecentTracesQuery {
    pub environment: Option<String>,
    pub per_function: Option<u64>,
}

/// Every function of the environment's build, each with its latest runs.
#[derive(Debug, Serialize)]
pub struct RecentTraces {
    pub environment: String,
    /// The bound each function's `traces` was read under.
    pub per_function: u64,
    /// How long a trace is kept: nothing older can be here.
    pub retention_days: i64,
    /// In name order. A function with no recorded run in the window has an
    /// empty `traces`, so "never ran" and "not listed" cannot be confused.
    pub functions: Vec<FunctionTraces>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct FunctionTraces {
    pub function: String,
    /// Newest first.
    pub traces: Vec<TraceDigest>,
}

/// One recorded run, as the shapes of what it called.
#[derive(Debug, PartialEq, Serialize)]
pub struct TraceDigest {
    pub invocation_id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub mode: String,
    /// The pager's rule (`invocations::counted_as_failure`).
    pub failed: bool,
    /// Every call the run made, kept in its trace or not.
    pub total_calls: i32,
    /// Calls past the trace's cap, which `touches` cannot include.
    pub dropped: i32,
    pub touches: Vec<Touch>,
}

/// Calls of one run that share a shape.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Touch {
    pub family: String,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub status: CallStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    /// How many of the run's kept calls had this shape.
    pub calls: u32,
}

impl Touch {
    fn of(call: CallRecord) -> Self {
        Self {
            family: call.family,
            op: call.op,
            verb: call.verb,
            target: call.target,
            status: call.status,
            error_type: call.error_type,
            calls: 1,
        }
    }

    fn same_shape(&self, other: &Self) -> bool {
        self.family == other.family
            && self.op == other.op
            && self.verb == other.verb
            && self.target == other.target
            && self.status == other.status
            && self.error_type == other.error_type
    }
}

/// A run's calls folded to their distinct shapes, in the order each shape was
/// first called. A trace keeps at most 200 calls, so the scan is small.
pub(crate) fn touches_of(calls: Vec<CallRecord>) -> Vec<Touch> {
    let mut touches: Vec<Touch> = Vec::new();
    for call in calls {
        let touch = Touch::of(call);
        match touches.iter_mut().find(|seen| seen.same_shape(&touch)) {
            Some(seen) => seen.calls += 1,
            None => touches.push(touch),
        }
    }
    touches
}

/// `per_function`, defaulted and bounded. Out of range is an error rather than
/// a clamp, as `invocations::parse_limit` reasons.
pub(crate) fn parse_per_function(per_function: Option<u64>) -> Result<u64, ScopeError> {
    match per_function {
        None => Ok(DEFAULT_PER_FUNCTION),
        Some(n) if (1..=MAX_PER_FUNCTION).contains(&n) => Ok(n),
        Some(n) => Err(invalid_per_function(format!(
            "per_function must be between 1 and {MAX_PER_FUNCTION}; got {n}"
        ))),
    }
}

fn invalid_per_function(message: String) -> ScopeError {
    ScopeError::new(StatusCode::BAD_REQUEST, "invalid_per_function", message)
}

/// One row of [`RECENT_TRACES_SQL`]: a function, and one of its runs — or
/// none, for a function with no recorded run in the window.
#[derive(Debug, FromQueryResult)]
pub(crate) struct TraceRow {
    pub function_name: String,
    pub invocation_id: Option<Uuid>,
    pub created_at: Option<DateTime<FixedOffset>>,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub failure_fingerprint: Option<String>,
    pub total_calls: Option<i32>,
    pub dropped: Option<i32>,
    pub calls: Option<serde_json::Value>,
}

/// `$1` app, `$2` the build, `$3` the environment's name, `$4` the window
/// start, `$5` runs per function. `LEFT JOIN LATERAL … ON TRUE` keeps a
/// function with no run; `init_ms IS NOT NULL` leaves out a run that never
/// reached tenant code, whose trace holds no calls.
///
/// Each probe is served by `idx_app_function_invocations_app_fn_env_created`
/// (`m20261117_000010`): its equality columns and its `created_at DESC, id
/// DESC` order are the probe's `WHERE` and `ORDER BY`, so a probe walks one
/// function's newest rows in one environment and stops at `$5`, however many
/// invocations that function has had. Keep the two in step.
const RECENT_TRACES_SQL: &str = r#"
SELECT f.name AS function_name,
       t.id AS invocation_id, t.created_at, t.mode, t.status, t.failure_fingerprint,
       t.total_calls, t.dropped, t.calls
  FROM app_functions f
  LEFT JOIN LATERAL (
       SELECT i.id, i.created_at, i.mode, i.status, i.failure_fingerprint,
              c.total_calls, c.dropped, c.calls
         FROM app_function_invocations i
         JOIN app_function_invocation_calls c
           ON c.invocation_id = i.id AND c.app_id = i.app_id
        WHERE i.app_id = $1
          AND i.function_name = f.name
          AND i.environment = $3
          AND i.created_at >= $4
          AND c.init_ms IS NOT NULL
        ORDER BY i.created_at DESC, i.id DESC
        LIMIT $5
  ) t ON TRUE
 WHERE f.build_id = $2
 ORDER BY f.name, t.created_at DESC NULLS LAST, t.id DESC
"#;

/// Rows (ordered by function, then newest first) folded into one entry per
/// function, in the order they arrive.
pub(crate) fn assemble(rows: Vec<TraceRow>) -> Vec<FunctionTraces> {
    let mut functions: Vec<FunctionTraces> = Vec::new();
    for row in rows {
        if functions
            .last()
            .is_none_or(|f| f.function != row.function_name)
        {
            functions.push(FunctionTraces {
                function: row.function_name.clone(),
                traces: Vec::new(),
            });
        }
        if let Some(digest) = digest_of(row)
            && let Some(current) = functions.last_mut()
        {
            current.traces.push(digest);
        }
    }
    functions
}

fn digest_of(row: TraceRow) -> Option<TraceDigest> {
    let (invocation_id, created_at) = (row.invocation_id?, row.created_at?);
    let status = row.status.unwrap_or_default();
    Some(TraceDigest {
        invocation_id,
        created_at,
        mode: row.mode.unwrap_or_default(),
        failed: super::invocations::counted_as_failure(&status, row.failure_fingerprint.as_deref()),
        total_calls: row.total_calls.unwrap_or(0),
        dropped: row.dropped.unwrap_or(0),
        touches: touches_of(row.calls.as_ref().map(calls_of).unwrap_or_default()),
    })
}

/// The build whose functions the console lists for `raw`: with no environment
/// named, the app's active build, as `functions::list_functions` reads it;
/// otherwise the build the named environment runs functions from.
async fn listed_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    raw: Option<&str>,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, ScopeError> {
    let lookup = |e| ScopeError::internal("environment lookup failed", e);
    if raw.is_none_or(|r| r.trim().is_empty()) {
        return Ok(load_environment_builds(db, app)
            .await
            .map_err(lookup)?
            .active());
    }
    Ok(resolve_function_environment(db, app, environment)
        .await
        .map_err(lookup)?
        .build_id)
}

/// `GET /admin/apps/{id}/recent-traces` — each function's latest recorded runs
/// in `?environment=` (production when absent), `?per_function=` of them
/// (default 5, at most 20), each folded to the distinct `ctx.*` calls it made.
pub async fn list_recent_traces(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    query: Result<Query<RecentTracesQuery>, QueryRejection>,
) -> Result<Json<RecentTraces>, ScopeError> {
    let Query(q) = query.map_err(|e| invalid_per_function(e.to_string()))?;
    let per_function = parse_per_function(q.per_function)?;
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, id).await?;
    let marker = marker.as_ref().map(|axum::Extension(marker)| marker);
    let raw = q.environment.as_deref();
    let environment = environment_scope::resolve(&db, &app, &user, marker, raw).await?;
    let retention_days = store::retention_days();
    let mut functions = Vec::new();
    if let Some(build) = listed_build(&db, &app, raw, &environment).await? {
        let window = Utc::now() - Duration::days(retention_days);
        let since = super::agent_scope::instance_since(&db, &app, &user, &environment).await?;
        let from = since.map_or(window, |since| since.max(window));
        functions = read(&db, app.id, build, &environment, from, per_function).await?;
    }
    Ok(Json(RecentTraces {
        environment: environment.name(),
        per_function,
        retention_days,
        functions,
    }))
}

async fn read(
    db: &DatabaseConnection,
    app_id: Uuid,
    build: Uuid,
    environment: &AppEnvironment,
    from: DateTime<Utc>,
    per_function: u64,
) -> Result<Vec<FunctionTraces>, ScopeError> {
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        RECENT_TRACES_SQL,
        [
            app_id.into(),
            build.into(),
            environment.name().into(),
            from.fixed_offset().into(),
            (per_function as i64).into(),
        ],
    );
    let rows = TraceRow::find_by_statement(statement)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("recent trace query failed", e))?;
    Ok(assemble(rows))
}

#[cfg(test)]
#[path = "recent_traces_tests.rs"]
mod tests;
