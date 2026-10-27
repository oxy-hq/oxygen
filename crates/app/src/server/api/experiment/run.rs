//! `GET /experiments/{name}/run` and `POST /experiments/{name}/abort`. Thin:
//! parse, call `server::experiment::status`, serialize. Both FleetOk: one
//! reads persisted rows, the other writes one flag the tick actuates on.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use oxy_experiment::run::AbortLevers;
use serde::Deserialize;
use uuid::Uuid;

use super::{ApiError, connect, internal};
use crate::server::api::middlewares::role_guards::ExperimentAbortGuard;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::status::{
    AbortOutcome, OwnerAbort, RunStatusBody, request_abort, run_status,
};

pub async fn get_run(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<RunStatusBody>, ApiError> {
    let db = connect().await?;
    run_status(&db, workspace_manager.workspace_id, &name, Utc::now())
        .await
        .map_err(internal("read experiment run"))?
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no experiment `{name}` on main and no run"),
            )
        })
}

#[derive(Debug, Deserialize)]
pub struct AbortRequest {
    pub reason: String,
    /// `off` (the default) turns every treated unit's lever off; `keep` leaves each as it is.
    #[serde(default)]
    pub levers: AbortLevers,
}

pub async fn abort_run(
    _: ExperimentAbortGuard,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<AbortRequest>,
) -> Result<(StatusCode, Json<RunStatusBody>), ApiError> {
    let reason = body.reason.trim();
    if reason.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "an abort needs a reason: it is what the owner is told".into(),
        ));
    }
    let (db, ws, now) = (connect().await?, workspace_manager.workspace_id, Utc::now());
    let req = OwnerAbort {
        reason,
        levers: body.levers,
    };
    match request_abort(&db, ws, &name, req, now)
        .await
        .map_err(internal("record abort"))?
    {
        AbortOutcome::NoRun => Err((
            StatusCode::NOT_FOUND,
            format!("experiment `{name}` has no run"),
        )),
        AbortOutcome::NotAbortable { state } => Err((
            StatusCode::CONFLICT,
            format!("abort applies to registered or live runs; this run is {state}"),
        )),
        AbortOutcome::Requested => {
            let status = run_status(&db, ws, &name, now)
                .await
                .map_err(internal("read experiment run"))?;
            let status = status.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("experiment `{name}` has no run"),
                )
            })?;
            Ok((StatusCode::ACCEPTED, Json(status)))
        }
    }
}
