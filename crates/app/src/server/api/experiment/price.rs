use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::Serialize;
use uuid::Uuid;

use super::{ApiError, connect, internal};
use crate::server::api::middlewares::role_guards::ExperimentPricer;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::enqueue::{StartError, StartInput, start_price};
use crate::server::experiment::request::PriceBody;

/// Index C5's `PriceResponse`.
#[derive(Debug, Serialize)]
pub struct PriceResponse {
    pub request_id: Uuid,
    pub proposal_ids: Vec<Uuid>,
}

/// `POST /experiments/{name}/price` — insert one `pending` proposal per
/// candidate, queue one pricing job each, and return the ids. The result — a
/// snippet, a refusal or a fault — is each row once its job settles it: read
/// them from `GET /experiments/{name}/proposals`.
///
/// `_guard` is the check: `Action::ExperimentPrice` on the `WorkspaceEdit` ring.
pub async fn post_price(
    _guard: ExperimentPricer,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<PriceBody>,
) -> Result<(StatusCode, Json<PriceResponse>), ApiError> {
    body.validate()
        .map_err(|reason| (StatusCode::BAD_REQUEST, reason))?;
    let db = connect().await?;
    let input = StartInput {
        workspace_id: workspace_manager.workspace_id,
        name,
        requested_by: user.id,
        candidates: body.candidates,
    };
    match start_price(&db, input).await {
        Ok(started) => Ok((
            StatusCode::ACCEPTED,
            Json(PriceResponse {
                request_id: started.request_id,
                proposal_ids: started.proposal_ids,
            }),
        )),
        Err(StartError::NotFound(name)) => Err((
            StatusCode::NOT_FOUND,
            format!("no experiment named '{name}'"),
        )),
        Err(e) => Err(internal("enqueue price")(e)),
    }
}
