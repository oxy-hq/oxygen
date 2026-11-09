//! `GET` and `POST /experiments/{name}/interim`. Thin: extract, call
//! `experiment::interim::read`, serialize. FleetOk: Postgres reads, and the POST
//! enqueues a `TaskSpec` — the warehouse reads run on the worker.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use uuid::Uuid;

use super::connect;
use crate::experiment::interim::read::{
    InterimError, InterimResponse, read_interim, request_interim,
};
use oxy_app::surface::WorkspaceManagerReadOnly;
use oxy_server_authz::role_guards::ExperimentPricer;

pub async fn get_interim(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<InterimResponse>, InterimError> {
    let db = connect().await.map_err(|(_, e)| InterimError::Db(e))?;
    read_interim(&db, workspace_manager.workspace_id, &name, Utc::now())
        .await
        .map(Json)
}

/// Like pricing, a job that reads the warehouse and changes nothing, so it takes pricing's
/// guard (`Action::ExperimentPrice`, the `WorkspaceEdit` ring). One job in flight per run.
pub async fn post_interim(
    _guard: ExperimentPricer,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<InterimResponse>), InterimError> {
    let db = connect().await.map_err(|(_, e)| InterimError::Db(e))?;
    request_interim(&db, workspace_manager.workspace_id, &name, Utc::now())
        .await
        .map(|r| (StatusCode::ACCEPTED, Json(r)))
}
