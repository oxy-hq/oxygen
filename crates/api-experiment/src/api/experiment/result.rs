//! `GET /experiments/{name}/result`. Thin: extract, call
//! `experiment::results_api`, serialize. FleetOk: two Postgres reads.
//! The 409 (sealed / pending) is decided there, on the run's state.

use axum::Json;
use axum::extract::Path;
use uuid::Uuid;

use super::connect;
use crate::experiment::results_api::{ResultError, ResultResponse, read_result};
use oxy_app::surface::WorkspaceManagerReadOnly;

pub async fn get_result(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<ResultResponse>, ResultError> {
    // The workspace comes from the extractor, never the URL segment: the URL is caller-supplied.
    let db = connect().await.map_err(|(_, e)| ResultError::Db(e))?;
    read_result(&db, workspace_manager.workspace_id, &name)
        .await
        .map(Json)
}
