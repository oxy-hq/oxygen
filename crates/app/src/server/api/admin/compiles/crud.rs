//! Single-revision compile operations: list, detail, run-now, manual
//! single-revision promote (rollback), and the uncompiled backfill.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, Utc};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    PromoteError, connect, db_err, error_body, insert_run_and_enqueue_compile, listing_scope,
    promote_one,
};
use crate::server::api::admin::scope;

// GET /admin/compiles

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Cap on rows returned. Defaults to 50, clamped to 200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Filter to one workspace.
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
    /// Filter to every workspace in one organization (powers the org-360
    /// Compiles tab).
    #[serde(default)]
    pub org_id: Option<Uuid>,
    /// Filter by status. One of `compiling` / `ready` / `failed`.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct CompileRow {
    pub revision_id: Uuid,
    pub workspace_id: Uuid,
    pub git_sha: String,
    pub branch: Option<String>,
    pub status: String,
    pub kind: String,
    pub owner_user_id: Option<Uuid>,
    pub compiler_version: String,
    pub schema_version: i32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub file_count_seen: i32,
    pub file_count_compiled: i32,
    pub file_count_failed: i32,
    pub is_current_for_workspace: bool,
}

#[derive(Serialize, Debug)]
pub struct ListResponse {
    pub rows: Vec<CompileRow>,
    pub total_returned: usize,
}

pub async fn list_compiles(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListResponse>, Response> {
    let db = connect().await?;
    let reach = listing_scope(&db, &actor).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);

    let mut find =
        entity::revisions::Entity::find().order_by_desc(entity::revisions::Column::StartedAt);
    if let Some(orgs) = reach {
        // The caller's grant scope, as the same subquery shape the `org_id`
        // filter below uses — in the query, so `limit` applies to what the
        // caller may see. A workspace with no org is in no bounded grant.
        let reachable = entity::workspaces::Entity::find()
            .select_only()
            .column(entity::workspaces::Column::Id)
            .filter(entity::workspaces::Column::OrgId.is_in(orgs))
            .into_query();
        find = find.filter(entity::revisions::Column::WorkspaceId.in_subquery(reachable));
    }
    if let Some(ws) = query.workspace_id {
        find = find.filter(entity::revisions::Column::WorkspaceId.eq(ws));
    }
    if let Some(org) = query.org_id {
        // Scope to revisions whose workspace belongs to this org. A subquery
        // keeps it one round trip and lets `limit` apply to the joined set.
        let org_workspaces = entity::workspaces::Entity::find()
            .select_only()
            .column(entity::workspaces::Column::Id)
            .filter(entity::workspaces::Column::OrgId.eq(org))
            .into_query();
        find = find.filter(entity::revisions::Column::WorkspaceId.in_subquery(org_workspaces));
    }
    if let Some(ref status) = query.status {
        find = find.filter(entity::revisions::Column::Status.eq(status.clone()));
    }
    let rows = find.limit(limit as u64).all(&db).await.map_err(db_err)?;

    // For each revision_id seen, mark whether its workspace points at
    // it as current. Lookup is bounded by `limit` so the IN list stays
    // small.
    let revision_ids: Vec<Uuid> = rows.iter().map(|r| r.revision_id).collect();
    let workspace_ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r.workspace_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut current_for: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    if !revision_ids.is_empty() || !workspace_ids.is_empty() {
        let workspaces = entity::workspaces::Entity::find()
            .filter(entity::workspaces::Column::Id.is_in(workspace_ids))
            .all(&db)
            .await
            .map_err(db_err)?;
        for ws in workspaces {
            if let Some(rid) = ws.current_revision_id {
                current_for.insert(rid);
            }
        }
    }

    let out: Vec<CompileRow> = rows
        .into_iter()
        .map(|r| compile_row_from_model(r, &current_for))
        .collect();
    let total_returned = out.len();
    Ok(Json(ListResponse {
        rows: out,
        total_returned,
    }))
}

fn compile_row_from_model(
    r: entity::revisions::Model,
    current_for: &std::collections::HashSet<Uuid>,
) -> CompileRow {
    let started_at = r.started_at.with_timezone(&Utc);
    let finished_at = r.finished_at.map(|f| f.with_timezone(&Utc));
    let duration_ms = finished_at
        .map(|f| (f - started_at).num_milliseconds())
        .map(|d| d.max(0));
    CompileRow {
        revision_id: r.revision_id,
        workspace_id: r.workspace_id,
        git_sha: r.git_sha,
        branch: r.branch,
        status: r.status,
        kind: r.kind,
        owner_user_id: r.owner_user_id,
        compiler_version: r.compiler_version,
        schema_version: r.schema_version,
        started_at,
        finished_at,
        duration_ms,
        file_count_seen: r.file_count_seen,
        file_count_compiled: r.file_count_compiled,
        file_count_failed: r.file_count_failed,
        is_current_for_workspace: current_for.contains(&r.revision_id),
    }
}

// GET /admin/compiles/{revision_id}

/// One entity successfully written into a revision — the "which compiled" unit.
#[derive(Serialize, Debug)]
pub struct CompiledEntity {
    /// agent | view | topic | app | automation | verified_query | pipeline.
    pub kind: String,
    pub name: String,
    pub file_path: String,
}

#[derive(Serialize, Debug)]
pub struct CompileDetail {
    #[serde(flatten)]
    pub row: CompileRow,
    /// Full `error_summary` JSONB from the revisions row. Null on success — the
    /// "which did NOT compile" side: per-file `{path, kind, message}` failures.
    pub error_summary: Option<serde_json::Value>,
    /// Every entity successfully compiled into this revision (flat, by kind) —
    /// the "which DID compile" side, complementing `error_summary`.
    pub compiled_entities: Vec<CompiledEntity>,
}

pub async fn get_compile(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(revision_id): Path<Uuid>,
) -> Result<Json<CompileDetail>, Response> {
    let db = connect().await?;
    let not_found = || {
        error_body(
            StatusCode::NOT_FOUND,
            "not_found",
            Some(format!("revision {revision_id} not found")),
        )
    };
    let row = entity::revisions::Entity::find_by_id(revision_id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or_else(not_found)?;
    // A revision outside the caller's grant answers exactly what a missing one
    // does — same status, same body — so ids cannot be probed.
    scope::deny_out_of_scope_for_workspace(&db, &actor, row.workspace_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => not_found(),
            other => error_body(other, "scope_unreadable", None),
        })?;

    let workspace = entity::workspaces::Entity::find_by_id(row.workspace_id)
        .one(&db)
        .await
        .map_err(db_err)?;
    let mut current_for = std::collections::HashSet::new();
    if let Some(ws) = workspace
        && let Some(rid) = ws.current_revision_id
    {
        current_for.insert(rid);
    }

    let error_summary = row.error_summary.clone();
    let compiled_entities = collect_compiled_entities(&db, revision_id).await?;
    let cr = compile_row_from_model(row, &current_for);
    Ok(Json(CompileDetail {
        row: cr,
        error_summary,
        compiled_entities,
    }))
}

/// List every entity successfully compiled into `revision_id`, across all the
/// per-kind `*_definitions` tables — the "which compiled" side of the detail
/// view (`revisions.error_summary` carries the "which didn't").
async fn collect_compiled_entities(
    db: &sea_orm::DatabaseConnection,
    revision_id: Uuid,
) -> Result<Vec<CompiledEntity>, Response> {
    let mut out = Vec::new();
    macro_rules! collect {
        ($module:path, $kind:literal) => {{
            use $module as m;
            let rows = m::Entity::find()
                .filter(m::Column::RevisionId.eq(revision_id))
                .all(db)
                .await
                .map_err(db_err)?;
            out.extend(rows.into_iter().map(|r| CompiledEntity {
                kind: $kind.to_string(),
                name: r.name,
                file_path: r.file_path,
            }));
        }};
    }
    collect!(entity::agent_definitions, "agent");
    collect!(entity::semantic_views, "view");
    collect!(entity::semantic_topics, "topic");
    collect!(entity::app_definitions, "app");
    collect!(entity::automation_definitions, "automation");
    collect!(entity::airway_pipelines, "pipeline");

    // verified_queries has no `name` column — label it by file path.
    let vqs = entity::verified_queries::Entity::find()
        .filter(entity::verified_queries::Column::RevisionId.eq(revision_id))
        .all(db)
        .await
        .map_err(db_err)?;
    out.extend(vqs.into_iter().map(|r| CompiledEntity {
        kind: "verified_query".to_string(),
        name: r.file_path.clone(),
        file_path: r.file_path,
    }));

    Ok(out)
}

// POST /admin/compiles/run

/// Operator action: enqueue a Compile TaskSpec for the given
/// workspace. Used from the admin UI's "Run compile now" button. The
/// task lands in `agentic_task_queue` and is drained by the standard
/// worker fleet.
///
/// `promote` defaults to `false` — the operator can opt into promoting
/// the resulting revision to `workspaces.current_revision_id` (cloud
/// production uses this on the webhook path).
#[derive(Deserialize, Debug)]
pub struct RunCompileRequest {
    pub workspace_id: Uuid,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub promote: bool,
}

#[derive(Serialize, Debug)]
pub struct RunCompileResponse {
    pub task_id: String,
    pub workspace_id: Uuid,
    pub promote: bool,
}

pub async fn run_compile_now(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Json(req): Json<RunCompileRequest>,
) -> Result<Json<RunCompileResponse>, Response> {
    let db = connect().await?;

    // Make sure the workspace exists before we enqueue — otherwise
    // the worker will fail with a confusing FK violation when it
    // tries to insert the revisions row. The same read is the scope fence: a
    // workspace outside the caller's grant answers the 404 a missing one does.
    scope::deny_out_of_scope_for_workspace(&db, &actor, req.workspace_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(
                status,
                "workspace_not_found",
                Some(format!("workspace {} not found", req.workspace_id)),
            ),
            other => error_body(other, "scope_unreadable", None),
        })?;

    let task_id = insert_run_and_enqueue_compile(
        &db,
        req.workspace_id,
        req.git_sha.clone(),
        req.branch.clone(),
        req.promote,
    )
    .await
    .map_err(|e| {
        tracing::error!(?e, "admin/compiles: enqueue failed");
        error_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            "enqueue_failed",
            Some(format!("{e}")),
        )
    })?;

    Ok(Json(RunCompileResponse {
        task_id,
        workspace_id: req.workspace_id,
        promote: req.promote,
    }))
}

// Rollback: repoint a workspace at a prior good revision

#[derive(Serialize, Debug)]
pub struct PromoteResponse {
    pub revision_id: Uuid,
    pub workspace_id: Uuid,
}

/// Operator rollback lever: set `workspaces.current_revision_id` to an existing
/// `ready` `main` revision. Unlike the compile-time promote, this is
/// **unconditional** — it deliberately allows going *backwards* to a known-good
/// revision when a bad compile shipped. The target must already be compiled
/// (`ready`/`main`); its rows are retained until the retention window, so any
/// revision still in the timeline is promotable.
pub async fn promote_to_revision(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(revision_id): Path<Uuid>,
) -> Result<Json<PromoteResponse>, Response> {
    let db = connect().await?;
    match promote_one(&db, &actor, revision_id).await {
        Ok(workspace_id) => Ok(Json(PromoteResponse {
            revision_id,
            workspace_id,
        })),
        Err(PromoteError::NotFound) => Err(error_body(
            StatusCode::NOT_FOUND,
            "revision_not_found",
            Some(format!("revision {revision_id} not found")),
        )),
        Err(PromoteError::NotPromotable(msg)) => Err(error_body(
            StatusCode::BAD_REQUEST,
            "not_promotable",
            Some(msg),
        )),
        Err(PromoteError::ScopeUnreadable) => Err(error_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            "scope_unreadable",
            None,
        )),
        Err(PromoteError::Db(e)) => Err(db_err(e)),
    }
}
