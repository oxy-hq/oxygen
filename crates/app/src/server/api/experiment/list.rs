use axum::Json;

use super::{ApiError, connect, internal};
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::list::list_items;
use crate::server::experiment::summary::ExperimentListItem;

/// `GET /experiments` — the workspace's experiments, each with its status, the
/// version it shows, its latest proposal and how many were drawn.
pub async fn list_experiments(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<Vec<ExperimentListItem>>, ApiError> {
    let db = connect().await?;
    list_items(&db, workspace_manager.workspace_id, chrono::Utc::now())
        .await
        .map(Json)
        .map_err(internal("list experiments"))
}
