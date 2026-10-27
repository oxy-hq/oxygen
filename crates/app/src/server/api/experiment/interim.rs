//! `GET` and `POST /experiments/{name}/interim`. Thin: extract, call
//! `server::experiment::interim::read`, serialize. FleetOk: Postgres reads, and the POST
//! enqueues a `TaskSpec` — the warehouse reads run on the worker.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use uuid::Uuid;

use super::connect;
use crate::server::api::middlewares::role_guards::ExperimentPricer;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::interim::read::{
    InterimError, InterimResponse, read_interim, request_interim,
};

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
