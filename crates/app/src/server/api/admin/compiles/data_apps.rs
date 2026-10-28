//! GET `/admin/compiles/data-apps` — how many Data Apps (`.app.yml`) each
//! workspace holds, and how many of them are published.
//!
//! Exists so staff can size the removal of Data Apps without reading tenant
//! content: `GET /api/{workspace_id}/apps` answers 403 `assume_role_required`
//! for most workspaces, and an audited assume-role session is the wrong tool for
//! a headcount. This route returns the org name, the workspace name and two
//! counts per workspace, and nothing else: no app name, no file path, no
//! definition.
//!
//! The counts come from the compile boundary, from the rows of each workspace's
//! promoted revision (`workspaces.current_revision_id`) in `app_definitions`.
//! A workspace with no promoted revision has no rows to count, so it reports
//! `compiled: false` and zero; zero there means "not counted", not "none".

use axum::Json;
use axum::extract::Query;
use axum::response::Response;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{connect, db_err, listing_scope};
use crate::server::api::admin::scope::org_scope_clause;

#[derive(Debug, Default, Deserialize)]
pub struct DataAppsQuery {
    /// Keep only workspaces holding at least one Data App.
    #[serde(default)]
    pub with_apps: Option<bool>,
}

/// One row per workspace.
#[derive(Serialize, Debug, FromQueryResult)]
pub struct DataAppsRow {
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    /// `false` when the workspace has no promoted revision, so the counts below
    /// are not a measurement.
    pub compiled: bool,
    /// Data Apps in the promoted revision, published or not.
    pub data_app_count: i64,
    /// Of those, the ones with `published: true`.
    pub published_count: i64,
}

#[derive(Serialize, Debug)]
pub struct DataAppsResponse {
    /// Workspaces in the caller's grant scope, after `with_apps`.
    pub rows: Vec<DataAppsRow>,
    pub total_workspaces: usize,
    pub total_data_apps: i64,
    pub total_published: i64,
    /// Workspaces with no promoted revision: how many rows are not a measurement.
    pub uncompiled_workspaces: usize,
}

/// Every workspace in the caller's grant scope, not paged: the point is a total,
/// and a page boundary would silently drop workspaces from it. Narrowed in the
/// `WHERE` like every other listing on this surface, so a bounded grant counts
/// only the orgs it reaches and an org-less workspace is for unbounded grants only.
pub async fn list_data_apps(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(query): Query<DataAppsQuery>,
) -> Result<Json<DataAppsResponse>, Response> {
    let db = connect().await?;
    let reach = listing_scope(&db, &actor).await?;

    let mut values: Vec<sea_orm::Value> = Vec::new();
    let in_scope = org_scope_clause("w.org_id", reach.as_deref(), &mut values);
    let having = if query.with_apps.unwrap_or(false) {
        "HAVING COUNT(a.file_path) > 0"
    } else {
        ""
    };
    let sql = format!(
        "SELECT \
            w.org_id AS org_id, \
            o.name AS org_name, \
            w.id AS workspace_id, \
            w.name AS workspace_name, \
            (w.current_revision_id IS NOT NULL) AS compiled, \
            COUNT(a.file_path) AS data_app_count, \
            COUNT(a.file_path) FILTER (WHERE a.published) AS published_count \
        FROM workspaces w \
        LEFT JOIN organizations o ON o.id = w.org_id \
        LEFT JOIN app_definitions a ON a.revision_id = w.current_revision_id \
        WHERE TRUE{in_scope} \
        GROUP BY w.id, w.name, w.org_id, w.current_revision_id, o.name \
        {having} \
        ORDER BY published_count DESC, data_app_count DESC, o.name NULLS LAST, w.name"
    );

    let rows = DataAppsRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    Ok(Json(DataAppsResponse {
        total_workspaces: rows.len(),
        total_data_apps: rows.iter().map(|r| r.data_app_count).sum(),
        total_published: rows.iter().map(|r| r.published_count).sum(),
        uncompiled_workspaces: rows.iter().filter(|r| !r.compiled).count(),
        rows,
    }))
}
