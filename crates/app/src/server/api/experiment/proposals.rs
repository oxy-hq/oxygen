use axum::Json;
use axum::extract::Path;
use uuid::Uuid;

use super::{ApiError, connect, internal};
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::summary::{Proposal, list_proposal_views};

/// `GET /experiments/{name}/proposals` — every candidate ever priced for this
/// experiment, newest first, whatever its status: `pending` while the job runs,
/// then `done` (with a snippet), `refused` (with what was tested) or `failed`
/// (with the reason). Rows sharing a `request_id` were compared side by side;
/// within one, `created_at` ascends in candidate order. The `done` count is what
/// a report states as "drawn before the registered one".
pub async fn list_proposals(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<Vec<Proposal>>, ApiError> {
    let db = connect().await?;
    list_proposal_views(&db, workspace_manager.workspace_id, &name)
        .await
        .map(Json)
        .map_err(internal("list proposals"))
}
