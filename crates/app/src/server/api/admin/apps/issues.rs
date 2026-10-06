//! An app's failures as **issues**: one row per distinct failure, not one per
//! invocation.
//!
//! The invocation list answers "what ran". When an app is misbehaving the
//! question is "what is wrong with it, and is it still wrong" — and the answer
//! was scattered: the pager names a fingerprint in Slack, the Functions section
//! lists fifty rows per function, and matching the two up was done by eye.
//! This reads the same rows the pager decides from and groups them the way the
//! pager groups them.
//!
//! ## One failure is `(function, fingerprint)`
//!
//! That is the pager's key (`failure_alert::FailureKey`), and it has to be
//! this module's too. A fingerprint alone is not a failure: every timeout
//! hashes the same, as does every `http status 500`, so grouping by it would
//! fold unrelated functions into one issue that the pager counts as several.
//!
//! ## Nothing here is stored
//!
//! No status, no acknowledgement, no mute. The 2026-09-16 scope decision
//! ruled those out — a way to silence a failure is a way to mis-configure
//! ourselves into silence — and this view does not bring them back by another
//! door. What a reader usually wants from "resolved" is *derived* instead:
//! [`Issue::on_live_build`] says whether the failure has happened on the build
//! production serves now. A fix that shipped shows as an issue the live build
//! has not had; one that did not work shows the opposite, and nobody had to
//! remember to reopen anything.
//!
//! ## Production only
//!
//! Like the pager. A staging or sandbox failure is somebody testing, and the
//! invocation list (with `?environment=`) is where those are read. Production
//! rows are also the ones every caller this mount admits may already read, so
//! there is no reach to decide here.
//!
//! DB-only (FleetOk by `admin::router_roles`' wildcard), bounded by the window
//! and [`MAX_ISSUES`], and served by the partial index
//! `idx_app_function_invocations_failure
//! (app_id, function_name, failure_fingerprint, created_at)`.

use std::collections::HashMap;

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::prelude::AppBuilds;
use entity::{app_builds, apps};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter,
    Statement,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::environment_scope::{self, ScopeError};

/// The window read when the caller names none: the pager's own lookback
/// (`failure_alert::LOOKBACK_DAYS`), so "new" here and "new" in a page mean
/// the same week. Restated rather than imported — that module is compiled only
/// with the functions feature, and this route exists without it.
pub const DEFAULT_DAYS: i64 = 7;
/// The widest window one read spans.
pub const MAX_DAYS: i64 = 30;
/// The most issues one read returns, newest first. Past it the answer says so
/// ([`IssueList::truncated`]) rather than passing a cut list off as the whole.
pub const MAX_ISSUES: usize = 50;

/// `?days=` on the issue listing.
#[derive(Debug, Default, Deserialize)]
pub struct IssueQuery {
    pub days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct IssueList {
    /// The window the issues were counted over, in days.
    pub window_days: i64,
    pub issues: Vec<Issue>,
    /// More distinct failures occurred in the window than `issues` holds.
    pub truncated: bool,
}

/// One distinct failure of one function, over the window.
#[derive(Debug, Serialize)]
pub struct Issue {
    pub function_name: String,
    /// The failure's fingerprint — what a page quotes.
    pub fingerprint: String,
    pub occurrences: i64,
    /// First and last time it happened **inside the window**. A failure older
    /// than the window has an earlier first occurrence than this says.
    pub first_seen: String,
    pub last_seen: String,
    /// Distinct builds it happened on.
    pub builds: i64,
    /// It has happened on the build production serves now. False when the app
    /// has no published build.
    pub on_live_build: bool,
    pub last: LastOccurrence,
}

/// The most recent invocation that failed this way.
#[derive(Debug, Serialize)]
pub struct LastOccurrence {
    pub invocation_id: Uuid,
    /// The recorded outcome. `success` here is a function that answered 5xx or
    /// caught a failed `ctx.*` call — counted as a failure all the same.
    pub status: String,
    pub result_status: Option<i16>,
    /// The error text, when one was recorded. A caught host-call failure and a
    /// 5xx the handler returned record none.
    pub error: Option<String>,
    /// The publish's build id of the build it ran on.
    pub build_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    function_name: String,
    fingerprint: String,
    occurrences: i64,
    first_seen: DateTime<FixedOffset>,
    last_seen: DateTime<FixedOffset>,
    builds: i64,
    on_live_build: bool,
    last_invocation_id: Uuid,
    last_status: String,
    last_result_status: Option<i16>,
    last_error: Option<String>,
    last_build: Uuid,
}

/// The grouped read. `$1` app, `$2` window start, `$3` the live build (NULL
/// when nothing is published — `build_id = NULL` is never true, which is the
/// right answer), `$4` row cap.
///
/// The newest row of each group is picked with a window function rather than
/// `ARRAY_AGG(.. ORDER BY ..)[1]` per column: a failure in a hot loop is tens
/// of thousands of rows, and aggregating every error text to keep one would
/// hold all of them in memory.
const ISSUES_SQL: &str = r#"
WITH failed AS (
    SELECT id, function_name, failure_fingerprint, build_id, status, result_status, error,
           created_at,
           ROW_NUMBER() OVER (
               PARTITION BY function_name, failure_fingerprint
               ORDER BY created_at DESC, id DESC
           ) AS recency
    FROM app_function_invocations
    WHERE app_id = $1
      AND environment = 'production'
      AND failure_fingerprint IS NOT NULL
      AND created_at >= $2
)
SELECT function_name,
       failure_fingerprint AS fingerprint,
       COUNT(*)::bigint AS occurrences,
       MIN(created_at) AS first_seen,
       MAX(created_at) AS last_seen,
       COUNT(DISTINCT build_id)::bigint AS builds,
       COALESCE(BOOL_OR(build_id = $3), FALSE) AS on_live_build,
       (ARRAY_AGG(id) FILTER (WHERE recency = 1))[1] AS last_invocation_id,
       MAX(status) FILTER (WHERE recency = 1) AS last_status,
       MAX(result_status) FILTER (WHERE recency = 1) AS last_result_status,
       MAX(error) FILTER (WHERE recency = 1) AS last_error,
       (ARRAY_AGG(build_id) FILTER (WHERE recency = 1))[1] AS last_build
FROM failed
GROUP BY function_name, failure_fingerprint
ORDER BY last_seen DESC, function_name, fingerprint
LIMIT $4
"#;

/// `days`, defaulted and bounded. Out of range is an error rather than a
/// clamp, for the reason `invocations::parse_limit` gives: a caller who asked
/// for a year and was shown a month would read "no failures before then" off a
/// window they did not choose.
pub(crate) fn parse_days(days: Option<i64>) -> Result<i64, ScopeError> {
    match days {
        None => Ok(DEFAULT_DAYS),
        Some(n) if (1..=MAX_DAYS).contains(&n) => Ok(n),
        Some(n) => Err(ScopeError::new(
            StatusCode::BAD_REQUEST,
            "invalid_days",
            format!("days must be between 1 and {MAX_DAYS}; got {n}"),
        )),
    }
}

fn rejected(rejection: QueryRejection) -> ScopeError {
    ScopeError::new(
        StatusCode::BAD_REQUEST,
        "invalid_days",
        format!("days must be a whole number between 1 and {MAX_DAYS}: {rejection}"),
    )
}

/// The publish's build id of each build in `ids`, scoped to `app_id`.
async fn build_labels(
    db: &DatabaseConnection,
    app_id: Uuid,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, String>, ScopeError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let builds = AppBuilds::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::Id.is_in(ids))
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("app_builds lookup failed", e))?;
    Ok(builds.into_iter().map(|b| (b.id, b.build_id)).collect())
}

/// `app`'s production failures since `now - days`, grouped per
/// `(function, fingerprint)`, most recently seen first.
pub async fn issues_of(
    db: &DatabaseConnection,
    app: &apps::Model,
    days: i64,
    now: DateTime<Utc>,
) -> Result<IssueList, ScopeError> {
    let since: DateTime<FixedOffset> = (now - Duration::days(days)).into();
    // One past the cap: the extra row is how a cut list is told from a whole one.
    let fetch = (MAX_ISSUES + 1) as i64;
    let stmt = Statement::from_sql_and_values(
        db.get_database_backend(),
        ISSUES_SQL,
        [
            app.id.into(),
            since.into(),
            app.published_build_id.into(),
            fetch.into(),
        ],
    );
    let mut rows = IssueRow::find_by_statement(stmt)
        .all(db)
        .await
        .map_err(|e| ScopeError::internal("issue query failed", e))?;
    let truncated = rows.len() > MAX_ISSUES;
    rows.truncate(MAX_ISSUES);

    let labels = build_labels(db, app.id, rows.iter().map(|r| r.last_build).collect()).await?;
    let issues = rows
        .into_iter()
        .map(|row| Issue {
            function_name: row.function_name,
            fingerprint: row.fingerprint,
            occurrences: row.occurrences,
            first_seen: row.first_seen.to_rfc3339(),
            last_seen: row.last_seen.to_rfc3339(),
            builds: row.builds,
            on_live_build: row.on_live_build,
            last: LastOccurrence {
                invocation_id: row.last_invocation_id,
                status: row.last_status,
                result_status: row.last_result_status,
                error: row.last_error,
                build_id: labels.get(&row.last_build).cloned(),
                created_at: row.last_seen.to_rfc3339(),
            },
        })
        .collect();

    Ok(IssueList {
        window_days: days,
        issues,
        truncated,
    })
}

/// `GET /admin/apps/{id}/issues` — the app's production failures of the last
/// `?days=` (1–30, default 7) as issues: one per `(function, fingerprint)`,
/// the pager's own key, with how often it happened, when, and whether it has
/// happened on the build production serves now. Derived from the invocation
/// rows on every read; nothing about an issue is stored.
pub async fn list_app_issues(
    Path(id): Path<Uuid>,
    query: Result<Query<IssueQuery>, QueryRejection>,
) -> Result<Json<IssueList>, ScopeError> {
    let Query(q) = query.map_err(rejected)?;
    let days = parse_days(q.days)?;
    let db = environment_scope::connect().await?;
    let app = environment_scope::load_app(&db, id).await?;
    Ok(Json(issues_of(&db, &app, days, Utc::now()).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window is always bounded, and one outside the bounds is refused
    /// rather than quietly clamped.
    #[test]
    fn the_window_is_bounded_and_out_of_range_is_refused() {
        assert_eq!(parse_days(None).unwrap(), DEFAULT_DAYS);
        assert_eq!(parse_days(Some(1)).unwrap(), 1);
        assert_eq!(parse_days(Some(MAX_DAYS)).unwrap(), MAX_DAYS);
        for refused in [0, -1, MAX_DAYS + 1, i64::MAX] {
            let e = parse_days(Some(refused)).expect_err("out of range");
            assert_eq!(
                (e.status, e.code),
                (StatusCode::BAD_REQUEST, "invalid_days"),
                "{refused}"
            );
        }
    }

    /// The grouping key is the pager's. Grouping by fingerprint alone would
    /// fold every function's timeouts into one issue.
    #[test]
    fn issues_are_keyed_the_way_the_pager_keys_them() {
        assert!(ISSUES_SQL.contains("GROUP BY function_name, failure_fingerprint"));
        assert!(ISSUES_SQL.contains("PARTITION BY function_name, failure_fingerprint"));
        // …over the rows the pager reads: production's, fingerprinted.
        assert!(ISSUES_SQL.contains("environment = 'production'"));
        assert!(ISSUES_SQL.contains("failure_fingerprint IS NOT NULL"));
    }
}
