//! `POST /experiments/{name}/retrospective` — analyse the draft, whose dates are all past,
//! as the experiment's one run. Thin: parse, call `server::experiment::retrospective`,
//! serialize. FleetOk: one Postgres transaction (the run, and the design frozen); the tick and the estimate job do the rest.
//! Gated like registration: it takes the experiment's only run.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use super::designs::{RegisterBody, Registered, experiment_or_404, registered};
use super::{ApiError, connect};
use crate::server::api::middlewares::role_guards::ExperimentRegisterGuard;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::experiment::retrospective::create;

pub async fn post_retrospective(
    _: ExperimentRegisterGuard,
    WorkspaceManagerReadOnly(wm): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
    Json(body): Json<RegisterBody>,
) -> Result<(StatusCode, Json<Registered>), ApiError> {
    let db = connect().await?;
    let experiment = experiment_or_404(&db, wm.workspace_id, &name).await?;
    let hash = body.content_hash.as_deref();
    registered(
        create(&db, &experiment, hash, user.id, Utc::now()).await,
        "create the retrospective run",
    )
}
