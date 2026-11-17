//! `/admin/internal-jobs/failure-groups` — failed and dead jobs, grouped by what broke.
//!
//! The failure feed lists jobs one by one. On a platform that breaks in platform-shaped
//! ways that is the wrong unit: one workspace whose compile fails every few hours fills
//! the feed's 200 rows with copies of itself — on dev, 1,211 failures that were one
//! cause — and whatever else broke that week is below the fold. Workspace health learned
//! the same lesson and groups by cause; this does it for the queue.
//!
//! A group is one task type, in one workspace, with one error **signature**: the run's
//! error message with the parts that change between attempts — ids and counts — masked.
//! "compile recorded 7 failed file(s); revision_id=75fa…" and the same message with 8
//! files and another revision are one problem, not two.

use axum::Json;
use axum::extract::Query;
use axum::response::Response;
use chrono::{DateTime, FixedOffset};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::internal_jobs_reach::listing_scope;
use oxy_app_core::admin_db::{connect, db_err};
use oxy_server_authz::staff::scope::org_scope_clause;

/// How far back a group may reach. A week by default: long enough to see a nightly
/// failure as recurring, short enough that a fixed one ages off.
const DEFAULT_DAYS: i64 = 7;
const MAX_DAYS: i64 = 90;
/// Groups returned, worst-recent first. Far more than a healthy platform has.
const GROUP_LIMIT: i64 = 100;

#[derive(Deserialize, Default)]
pub struct FailureGroupsQuery {
    pub days: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct GroupRaw {
    task_type: String,
    workspace_id: Option<Uuid>,
    workspace_name: Option<String>,
    org_id: Option<Uuid>,
    org_name: Option<String>,
    signature: String,
    occurrences: i64,
    dead: i64,
    first_seen: DateTime<FixedOffset>,
    last_seen: DateTime<FixedOffset>,
    latest_task_id: String,
    latest_error: Option<String>,
}

/// One cause of failure.
#[derive(Debug, Serialize)]
pub struct FailureGroupDto {
    pub task_type: String,
    pub workspace_id: Option<Uuid>,
    pub workspace_name: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    /// The error with ids and counts masked — what the group is keyed on.
    pub signature: String,
    /// The most recent occurrence's message, unmasked, for reading.
    pub latest_error: Option<String>,
    pub occurrences: i64,
    /// How many of them exhausted every retry.
    pub dead: i64,
    pub first_seen: DateTime<FixedOffset>,
    pub last_seen: DateTime<FixedOffset>,
    /// The newest job in the group, for opening its debug panel.
    pub latest_task_id: String,
}

#[derive(Debug, Serialize)]
pub struct FailureGroupsResponse {
    pub days: i64,
    pub groups: Vec<FailureGroupDto>,
}

impl From<GroupRaw> for FailureGroupDto {
    fn from(r: GroupRaw) -> Self {
        Self {
            task_type: r.task_type,
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
            org_id: r.org_id,
            org_name: r.org_name,
            signature: r.signature,
            latest_error: r.latest_error.filter(|e| !e.is_empty()),
            occurrences: r.occurrences,
            dead: r.dead,
            first_seen: r.first_seen,
            last_seen: r.last_seen,
            latest_task_id: r.latest_task_id,
        }
    }
}

/// The masked message, in SQL: UUIDs first (they contain digits), then every run of
/// digits. Kept beside its test so the two cannot drift.
const SIGNATURE_SQL: &str = "regexp_replace(\
     regexp_replace(COALESCE(r.error_message, ''), \
       '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}', '…', 'g'), \
     '[0-9]+', '#', 'g')";

/// The statement. `$1` is the window in days, `$2` the group limit; a bounded grant's
/// org list binds after them.
fn failure_groups_sql(scope: Option<&[Uuid]>, values: &mut Vec<sea_orm::Value>) -> String {
    let in_scope = org_scope_clause("w.org_id", scope, values);
    format!(
        "WITH f AS ( \
            SELECT q.task_id, q.queue_status, q.updated_at, \
                   COALESCE(q.spec->>'type', 'unknown') AS task_type, \
                   r.workspace_id AS workspace_id, w.name AS workspace_name, \
                   w.org_id AS org_id, o.name AS org_name, \
                   r.error_message AS error_message, \
                   {SIGNATURE_SQL} AS signature \
            FROM agentic_task_queue q \
            LEFT JOIN agentic_runs r ON q.run_id = r.id \
            LEFT JOIN workspaces w ON r.workspace_id = w.id \
            LEFT JOIN organizations o ON w.org_id = o.id \
            WHERE q.queue_status IN ('failed', 'dead') \
              AND q.updated_at > now() - make_interval(days => $1){in_scope} \
        ) \
        SELECT task_type, workspace_id, workspace_name, org_id, org_name, signature, \
               COUNT(*)::bigint AS occurrences, \
               COUNT(*) FILTER (WHERE queue_status = 'dead')::bigint AS dead, \
               MIN(updated_at) AS first_seen, MAX(updated_at) AS last_seen, \
               (ARRAY_AGG(task_id ORDER BY updated_at DESC))[1] AS latest_task_id, \
               (ARRAY_AGG(error_message ORDER BY updated_at DESC))[1] AS latest_error \
        FROM f \
        GROUP BY task_type, workspace_id, workspace_name, org_id, org_name, signature \
        ORDER BY MAX(updated_at) DESC \
        LIMIT $2"
    )
}

pub async fn failure_groups(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<FailureGroupsQuery>,
) -> Result<Json<FailureGroupsResponse>, Response> {
    let db = connect().await?;
    let scope = listing_scope(&db, &actor).await?;
    let days = q.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    let mut values: Vec<sea_orm::Value> = vec![(days as i32).into(), GROUP_LIMIT.into()];
    let sql = failure_groups_sql(scope.as_deref(), &mut values);
    let rows = GroupRaw::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;
    Ok(Json(FailureGroupsResponse {
        days,
        groups: rows.into_iter().map(Into::into).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the two failure states are grouped, and the window is bound, not written in.
    #[test]
    fn reads_failed_and_dead_jobs_over_a_bound_window() {
        let sql = failure_groups_sql(None, &mut vec![7i32.into(), 100i64.into()]);
        assert!(
            sql.contains("q.queue_status IN ('failed', 'dead')"),
            "{sql}"
        );
        assert!(sql.contains("make_interval(days => $1)"), "{sql}");
        assert!(sql.contains("LIMIT $2"), "{sql}");
    }

    /// A group is task type × workspace × signature — the org columns ride along — so
    /// one cause in two workspaces is two rows, and two causes in one workspace too.
    #[test]
    fn groups_by_type_workspace_and_signature() {
        let sql = failure_groups_sql(None, &mut vec![]);
        assert!(
            sql.contains(
                "GROUP BY task_type, workspace_id, workspace_name, org_id, org_name, signature"
            ),
            "{sql}"
        );
    }

    /// A bounded grant sees its orgs' groups only, bound after the window and limit;
    /// an unbounded one gets no clause at all.
    #[test]
    fn fences_a_bounded_grant_after_the_window_and_limit() {
        let mut values: Vec<sea_orm::Value> = vec![7i32.into(), 100i64.into()];
        let sql = failure_groups_sql(Some(&[Uuid::nil()]), &mut values);
        assert!(sql.contains("AND w.org_id = ANY($3)"), "{sql}");
        assert_eq!(values.len(), 3);
        let sql = failure_groups_sql(None, &mut vec![]);
        assert!(!sql.contains("w.org_id = ANY"), "{sql}");
    }

    /// The masking that makes retries one group: ids before digits, because a UUID
    /// contains digits and masking those first would leave its letters behind.
    #[test]
    fn masks_ids_before_counts() {
        let uuid = SIGNATURE_SQL.find("{8}-").expect("uuid pattern");
        let digits = SIGNATURE_SQL.find("'[0-9]+'").expect("digit pattern");
        assert!(uuid < digits, "{SIGNATURE_SQL}");
    }
}
