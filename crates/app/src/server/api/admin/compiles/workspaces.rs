//! GET `/admin/compiles/workspaces` — one row per workspace, the compile
//! boundary's operator overview, folding per-workspace revision history into a
//! single aggregated record.

use axum::Json;
use axum::extract::Query;
use axum::response::Response;
use chrono::{DateTime, Utc};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{connect, db_err, listing_scope};
use crate::server::api::admin::scope::org_scope_clause;

#[derive(Debug, Default, Deserialize)]
pub struct WorkspacesQuery {
    /// Cap on rows returned. Defaults to 50, clamped to 1..=200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Page offset. Defaults to 0.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Case-insensitive substring over workspace_id text OR workspace name.
    #[serde(default)]
    pub q: Option<String>,
    /// Keep only workspaces whose CURRENT revision has this status
    /// (`compiling` / `ready` / `failed`).
    #[serde(default)]
    pub status: Option<String>,
}

/// One row per workspace — the compile boundary's operator overview. Folds the
/// per-workspace revision history (counts, latest, last-ready) plus the
/// promoted current revision into a single record so the admin UI doesn't have
/// to stitch scattered revisions back together.
#[derive(Serialize, Debug)]
pub struct WorkspaceRow {
    pub workspace_id: Uuid,
    pub workspace_name: Option<String>,
    pub workspace_path: Option<String>,
    pub current_revision_id: Option<Uuid>,
    pub current_status: Option<String>,
    pub current_git_sha: Option<String>,
    pub latest_revision_id: Option<Uuid>,
    pub latest_status: Option<String>,
    pub latest_started_at: Option<DateTime<Utc>>,
    pub last_ready_at: Option<DateTime<Utc>>,
    pub revision_count: i64,
    pub ready_count: i64,
    pub failed_count: i64,
    /// `false` when a newer ready `main` revision exists that is NOT the
    /// promoted current — i.e. a good revision sitting un-promoted.
    pub current_is_latest_ready: bool,
}

#[derive(Serialize, Debug)]
pub struct WorkspacesResponse {
    pub rows: Vec<WorkspaceRow>,
    pub total_returned: usize,
}

/// Raw projection of the aggregation query. `latest_ready_*` describes the most
/// recent `ready`/`main` revision, which drives `current_is_latest_ready`.
#[derive(Debug, FromQueryResult)]
struct WorkspaceAggRow {
    workspace_id: Uuid,
    workspace_name: Option<String>,
    workspace_path: Option<String>,
    current_revision_id: Option<Uuid>,
    current_status: Option<String>,
    current_git_sha: Option<String>,
    latest_revision_id: Option<Uuid>,
    latest_status: Option<String>,
    latest_started_at: Option<DateTime<Utc>>,
    last_ready_at: Option<DateTime<Utc>>,
    latest_ready_revision_id: Option<Uuid>,
    revision_count: i64,
    ready_count: i64,
    failed_count: i64,
}

impl From<WorkspaceAggRow> for WorkspaceRow {
    fn from(r: WorkspaceAggRow) -> Self {
        // Up-to-date iff there is no newer ready main revision, OR the latest
        // ready main revision IS the promoted current one. A workspace with no
        // ready revision at all is trivially "up to date" (nothing to promote).
        let current_is_latest_ready = match r.latest_ready_revision_id {
            None => true,
            Some(latest_ready) => r.current_revision_id == Some(latest_ready),
        };
        WorkspaceRow {
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
            workspace_path: r.workspace_path,
            current_revision_id: r.current_revision_id,
            current_status: r.current_status,
            current_git_sha: r.current_git_sha,
            latest_revision_id: r.latest_revision_id,
            latest_status: r.latest_status,
            latest_started_at: r.latest_started_at,
            last_ready_at: r.last_ready_at,
            revision_count: r.revision_count,
            ready_count: r.ready_count,
            failed_count: r.failed_count,
            current_is_latest_ready,
        }
    }
}

/// One row per workspace. A single aggregation over `revisions` (GROUP BY
/// workspace_id) is joined to `workspaces` for name/path/current pointer, and
/// LATERAL sub-selects pull the current revision's status/sha and the latest
/// (and latest-ready) revision per workspace — no N+1 across workspaces.
///
/// Narrowed to the caller's grant scope in the `WHERE`, ahead of `LIMIT`/`OFFSET`.
pub async fn list_workspaces(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(query): Query<WorkspacesQuery>,
) -> Result<Json<WorkspacesResponse>, Response> {
    let db = connect().await?;
    let reach = listing_scope(&db, &actor).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let q_param: Option<String> = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let status_param: Option<String> = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // All user input is bound ($1..$4) — never interpolated. `q` is wrapped in
    // %..% inside SQL via concat so the bound value stays a plain string. The
    // grant scope, when the caller is bounded, is bound after them.
    let mut values: Vec<sea_orm::Value> = vec![
        limit.into(),
        offset.into(),
        q_param.into(),
        status_param.into(),
    ];
    let in_scope = org_scope_clause("w.org_id", reach.as_deref(), &mut values);
    let sql = format!(
        "SELECT \
            w.id AS workspace_id, \
            w.name AS workspace_name, \
            w.path AS workspace_path, \
            w.current_revision_id AS current_revision_id, \
            cur.status AS current_status, \
            cur.git_sha AS current_git_sha, \
            latest.revision_id AS latest_revision_id, \
            latest.status AS latest_status, \
            latest.started_at AS latest_started_at, \
            lr.last_ready_at AS last_ready_at, \
            lr.latest_ready_revision_id AS latest_ready_revision_id, \
            COALESCE(agg.revision_count, 0) AS revision_count, \
            COALESCE(agg.ready_count, 0) AS ready_count, \
            COALESCE(agg.failed_count, 0) AS failed_count \
        FROM workspaces w \
        LEFT JOIN ( \
            SELECT workspace_id, \
                COUNT(*) AS revision_count, \
                COUNT(*) FILTER (WHERE status = 'ready') AS ready_count, \
                COUNT(*) FILTER (WHERE status = 'failed') AS failed_count \
            FROM revisions GROUP BY workspace_id \
        ) agg ON agg.workspace_id = w.id \
        LEFT JOIN revisions cur ON cur.revision_id = w.current_revision_id \
        LEFT JOIN LATERAL ( \
            SELECT revision_id, status, started_at \
            FROM revisions r WHERE r.workspace_id = w.id \
            ORDER BY started_at DESC LIMIT 1 \
        ) latest ON true \
        LEFT JOIN LATERAL ( \
            SELECT revision_id AS latest_ready_revision_id, started_at AS last_ready_at \
            FROM revisions r \
            WHERE r.workspace_id = w.id AND r.status = 'ready' AND r.kind = 'main' \
            ORDER BY started_at DESC LIMIT 1 \
        ) lr ON true \
        WHERE (w.path IS NOT NULL OR agg.revision_count IS NOT NULL) \
            AND ($3::text IS NULL OR w.id::text ILIKE '%' || $3 || '%' \
                 OR w.name ILIKE '%' || $3 || '%') \
            AND ($4::text IS NULL OR cur.status = $4){in_scope} \
        ORDER BY latest.started_at DESC NULLS LAST \
        LIMIT $1 OFFSET $2"
    );

    let rows = WorkspaceAggRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    let out: Vec<WorkspaceRow> = rows.into_iter().map(WorkspaceRow::from).collect();
    let total_returned = out.len();
    Ok(Json(WorkspacesResponse {
        rows: out,
        total_returned,
    }))
}
