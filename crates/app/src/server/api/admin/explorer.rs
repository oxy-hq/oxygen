//! `/admin/explorer/*` — cross-tenant, read-only search over the DB resources
//! operators reach for when debugging: **threads** and **runs**. Each row is
//! enriched with the workspace / org / user it belongs to and deep-link ids,
//! so an operator can go from "this conversation looks broken" straight to the
//! owning tenant.
//!
//! Gated on `Action::PlatformExplorer` (`view_tenants`) by `admin::router`.
//! Read-only — no mutations live here.
//!
//! **Narrowed by the caller's grant scope, in the query.** "Cross-tenant" is what an
//! unbounded grant sees. `view_tenants` is held by every Global Admin whatever their
//! bound and the capability gate cannot see scope, so both searches add
//! `w.org_id = ANY(<the grant's orgs>)` for a bounded caller — ahead of `LIMIT` and
//! inside the `COUNT(*) OVER()` window, so neither a page nor the total includes a
//! row from an org the grant does not name. A thread or run whose workspace has no
//! org (or that has no workspace) is platform-level: unbounded grants and the Global
//! Owner only. See `admin::scope::list_scope`.
//!
//! Results are paginated (`page`, `page_size`, 1-indexed) and can be narrowed
//! with `status` and `source_type` filters on top of the free-text `search`.
//! An optional `org_id` scopes results to a single tenant — this is what powers
//! the org-360 Activity tab (vs. the cross-tenant Explorer page, which omits
//! it). Each response carries a `total` row count via a `COUNT(*) OVER()`
//! window function so the frontend can render pagination controls without a
//! second round trip.
//!
//! Perf note: the search uses leading-wildcard `ILIKE '%term%'`, which can't
//! use a btree index and falls back to a sequential scan across every tenant's
//! `threads` / `agentic_runs`. The empty-term default is cheap (`ORDER BY
//! created_at DESC LIMIT`); only an actual term triggers the scan, and this is
//! an infrequent operator path. If it ever becomes hot, add a `pg_trgm` GIN
//! index on the searched columns — deferred for now to avoid the extension +
//! migration surface.

use axum::Json;
use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, FixedOffset};
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::internal_jobs::{connect, db_err};
use super::scope::{list_scope, org_scope_clause};
use crate::server::router::AppState;
use axum::response::{IntoResponse, Response};
use oxy_auth::extractor::AuthenticatedUserExtractor;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/explorer/threads", get(search_threads))
        .route("/explorer/runs", get(search_runs))
}

#[derive(Deserialize, Default)]
pub struct SearchQuery {
    pub search: Option<String>,
    pub status: Option<String>,
    pub source_type: Option<String>,
    /// Scope results to one tenant's workspaces. Omitted/empty = every tenant the
    /// caller's grant reaches.
    pub org_id: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

/// Validate and normalize the optional `org_id` filter.
///
/// `None` / blank means "all tenants" and maps to `None`, which the SQL treats
/// as a passthrough (`$n IS NULL OR …`). A non-blank value MUST be a
/// well-formed UUID — anything else is a 400 rather than a silent all-tenant
/// scan, so a typo'd id can never widen an org-scoped query back to the whole
/// fleet. Returns a native `Uuid` so the predicate compares against the indexed
/// `workspaces.org_id` column directly, with no per-row `::text` cast.
fn normalize_org_id(raw: Option<String>) -> Result<Option<Uuid>, StatusCode> {
    match raw {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => Uuid::parse_str(s.trim())
            .map(Some)
            .map_err(|_| StatusCode::BAD_REQUEST),
    }
}

/// A page of rows plus enough metadata to render pagination controls without
/// a separate `COUNT(*)` round trip.
#[derive(Serialize, Debug)]
pub struct PagedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: u64,
    pub page_size: u64,
}

struct Pagination {
    page: u64,
    limit: i64,
    offset: i64,
}

/// 1-indexed page, clamped to a sane row count per page.
fn paginate(page: Option<u64>, page_size: Option<u64>) -> Pagination {
    let page = page.unwrap_or(1).max(1);
    let limit = page_size.unwrap_or(25).clamp(1, 100) as i64;
    let offset = (page - 1) as i64 * limit;
    Pagination {
        page,
        limit,
        offset,
    }
}

// Threads

#[derive(Serialize, Debug, FromQueryResult)]
pub struct ThreadRow {
    pub id: Uuid,
    pub title: String,
    pub input_snippet: String,
    pub source_type: String,
    pub is_processing: bool,
    pub created_at: DateTime<FixedOffset>,
    pub user_email: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub workspace_name: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub org_slug: Option<String>,
    #[serde(skip)]
    pub total_count: i64,
}

pub async fn search_threads(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<SearchQuery>,
) -> Result<Json<PagedResponse<ThreadRow>>, Response> {
    let db = connect().await?;
    let scope = list_scope(&db, &actor)
        .await
        .map_err(IntoResponse::into_response)?;
    let org_id = normalize_org_id(q.org_id).map_err(IntoResponse::into_response)?;
    let pagination = paginate(q.page, q.page_size);
    let search = q.search.unwrap_or_default();

    // $1 = raw term (exact-id match + empty-term passthrough), $2 = ILIKE
    // pattern, $3 = source_type filter, $4 = status filter ("live" / "done" /
    // ""), $5 = limit, $6 = offset, $7 = org_id filter (NULL = no filter), then
    // the grant-scope array when the caller is bounded. Threads link to a
    // workspace via `project_id`; they have no `status` column, so the status
    // filter maps onto `is_processing`.
    let mut values: Vec<sea_orm::Value> = vec![
        search.clone().into(),
        format!("%{search}%").into(),
        q.source_type.unwrap_or_default().into(),
        q.status.unwrap_or_default().into(),
        pagination.limit.into(),
        pagination.offset.into(),
        org_id.into(),
    ];
    let in_scope = org_scope_clause("w.org_id", scope.as_deref(), &mut values);
    let sql = format!(
        "SELECT t.id AS id, t.title AS title, left(t.input, 200) AS input_snippet, \
                t.source_type AS source_type, t.is_processing AS is_processing, \
                t.created_at AS created_at, u.email AS user_email, \
                t.project_id AS workspace_id, w.name AS workspace_name, \
                w.org_id AS org_id, o.name AS org_name, o.slug AS org_slug, \
                COUNT(*) OVER() AS total_count \
         FROM threads t \
         LEFT JOIN users u ON t.user_id = u.id \
         LEFT JOIN workspaces w ON t.project_id = w.id \
         LEFT JOIN organizations o ON w.org_id = o.id \
         WHERE ($1 = '' OR t.title ILIKE $2 OR t.input ILIKE $2 OR t.output ILIKE $2 \
                OR t.id::text = $1) \
           AND ($3 = '' OR t.source_type = $3) \
           AND ($4 = '' \
                OR ($4 = 'live' AND t.is_processing) \
                OR ($4 = 'done' AND NOT t.is_processing)) \
           AND ($7 IS NULL OR w.org_id = $7){in_scope} \
         ORDER BY t.created_at DESC \
         LIMIT $5 OFFSET $6"
    );
    let rows = ThreadRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    Ok(Json(into_page(rows, &pagination)))
}

// Runs

#[derive(Serialize, Debug, FromQueryResult)]
pub struct RunRow {
    pub id: String,
    pub question_snippet: String,
    pub task_status: Option<String>,
    pub source_type: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<FixedOffset>,
    pub thread_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub workspace_name: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub org_slug: Option<String>,
    pub user_email: Option<String>,
    #[serde(skip)]
    pub total_count: i64,
}

pub async fn search_runs(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<SearchQuery>,
) -> Result<Json<PagedResponse<RunRow>>, Response> {
    let db = connect().await?;
    let scope = list_scope(&db, &actor)
        .await
        .map_err(IntoResponse::into_response)?;
    let org_id = normalize_org_id(q.org_id).map_err(IntoResponse::into_response)?;
    let pagination = paginate(q.page, q.page_size);
    let search = q.search.unwrap_or_default();

    // $1 = raw term, $2 = ILIKE pattern, $3 = task_status filter,
    // $4 = source_type filter, $5 = limit, $6 = offset, $7 = org_id filter
    // (NULL = no filter), then the grant-scope array when the caller is
    // bounded. Originating user comes via the run's thread.
    let mut values: Vec<sea_orm::Value> = vec![
        search.clone().into(),
        format!("%{search}%").into(),
        q.status.unwrap_or_default().into(),
        q.source_type.unwrap_or_default().into(),
        pagination.limit.into(),
        pagination.offset.into(),
        org_id.into(),
    ];
    let in_scope = org_scope_clause("w.org_id", scope.as_deref(), &mut values);
    let sql = format!(
        "SELECT ar.id AS id, left(ar.question, 200) AS question_snippet, \
                ar.task_status AS task_status, ar.source_type AS source_type, \
                ar.error_message AS error_message, ar.created_at AS created_at, \
                ar.thread_id AS thread_id, ar.workspace_id AS workspace_id, \
                w.name AS workspace_name, w.org_id AS org_id, \
                o.name AS org_name, o.slug AS org_slug, u.email AS user_email, \
                COUNT(*) OVER() AS total_count \
         FROM agentic_runs ar \
         LEFT JOIN workspaces w ON ar.workspace_id = w.id \
         LEFT JOIN organizations o ON w.org_id = o.id \
         LEFT JOIN threads th ON ar.thread_id = th.id \
         LEFT JOIN users u ON th.user_id = u.id \
         WHERE ($1 = '' OR ar.question ILIKE $2 OR ar.error_message ILIKE $2 OR ar.id = $1) \
           AND ($3 = '' OR ar.task_status = $3) \
           AND ($4 = '' OR ar.source_type = $4) \
           AND ($7 IS NULL OR w.org_id = $7){in_scope} \
         ORDER BY ar.created_at DESC \
         LIMIT $5 OFFSET $6"
    );
    let rows = RunRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    Ok(Json(into_page(rows, &pagination)))
}

/// Reads the window-function `total_count` off the first row and wraps the
/// rows into a [`PagedResponse`].
///
/// `COUNT(*) OVER()` rides along on each row, so an empty page carries no
/// count and we report `total = 0`. This only happens when the requested
/// `OFFSET` lands past the last matching row — i.e. an out-of-range page,
/// reachable if the match set shrinks (concurrent deletion) while the client
/// is paging deep. The client clamps back into range on `total = 0`, which
/// re-fetches a valid page and restores the true count, so the transient
/// zero never sticks. A separate `SELECT COUNT(*)` would keep the count
/// accurate on the empty page too, but at the cost of duplicating both WHERE
/// clauses — not worth it for a self-healing edge case.
fn into_page<T>(rows: Vec<T>, pagination: &Pagination) -> PagedResponse<T>
where
    T: HasTotalCount,
{
    let total = rows.first().map(HasTotalCount::total_count).unwrap_or(0);
    PagedResponse {
        items: rows,
        total,
        page: pagination.page,
        page_size: pagination.limit as u64,
    }
}

trait HasTotalCount {
    fn total_count(&self) -> i64;
}

impl HasTotalCount for ThreadRow {
    fn total_count(&self) -> i64 {
        self.total_count
    }
}

impl HasTotalCount for RunRow {
    fn total_count(&self) -> i64 {
        self.total_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_org_id_treats_none_and_blank_as_all_tenants() {
        assert_eq!(normalize_org_id(None).unwrap(), None);
        assert_eq!(normalize_org_id(Some(String::new())).unwrap(), None);
        assert_eq!(normalize_org_id(Some("   ".to_string())).unwrap(), None);
    }

    #[test]
    fn normalize_org_id_accepts_a_uuid_trimming_whitespace() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            normalize_org_id(Some(format!(" {id} "))).unwrap(),
            Some(Uuid::parse_str(id).unwrap())
        );
    }

    #[test]
    fn normalize_org_id_rejects_non_uuid_with_bad_request() {
        // A typo'd id must 400, never silently fall back to an all-tenant scan.
        assert_eq!(
            normalize_org_id(Some("not-a-uuid".to_string())),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            normalize_org_id(Some("123".to_string())),
            Err(StatusCode::BAD_REQUEST)
        );
    }
}
