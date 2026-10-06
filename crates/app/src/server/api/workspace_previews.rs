//! Workspace previews API — staff only (`WorkspacePreviewer`). Mounted under
//! `/api/{workspace_id}/previews` behind the workspace access check.
//!
//! * `GET    /previews`                    → `{"items":[…]}` — FleetOk
//! * `POST   /previews` `{"branch":"x"}`   → 202 `{"item":{…}}` — IdeOnly (reads `.git`)
//! * `POST   /previews/refresh?branch=x`   → 202 `{"item":{…}}` — IdeOnly (reads `.git`)
//! * `DELETE /previews?branch=x`           → 204 — FleetOk
//! * `GET    /previews/checks?branch=x`    → `{branch, revision_id, status, error, pipelines}`
//!   — FleetOk: the Airway change check of the preview's current revision
//! * `POST   /previews/runs` `{branch, kind, ref, variables | window, resources}` → 202
//!   `{run_id, state}` — a procedure dry run or an Airway sample
//! * `GET    /previews/runs?branch=x`      → `[RunSummary]`, newest first
//! * `GET    /previews/runs/{run_id}`      → `RunSummary` + `{agentic_run_id, error, steps}`
//!   — all FleetOk, all `404 preview_runs_disabled` without `OXY_PREVIEW_RUNS`
//!   (`server::previews::runs`)
//! * `GET    /previews/sources`            → `[SourceItem]`
//! * `PUT    /previews/sources` `{pipeline, environment, overrides}` → `SourceItem` — the
//!   sandbox company an Airway sample of a QuickBooks pipeline runs against
//!   (`server::previews::sources`); FleetOk, not gated by the flag
//!
//! A refused request answers `{"code":…,"message":…}`: `400` for a bad name,
//! the default branch or an unknown branch; `409 cannot_compile` when the
//! staging compile refuses (uncommitted changes in the branch's worktree; or a
//! branch that is not on GitHub, asked of a pod with no working copy — then
//! with a `"reason"` beside the code, such as `branch_not_pushed`);
//! `503 github_unavailable` when GitHub did not say where the branch is;
//! `404 preview_not_found` for refreshing one that does not exist.

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::{Deserialize, Serialize};

use crate::server::api::middlewares::role_guards::WorkspacePreviewer;
use crate::server::previews::checks::ChecksResponse;
use crate::server::previews::runs::{
    self as runs, RunDetail, RunRequestError, RunSummary, SubmitRun, Submitted,
};
use crate::server::previews::service::{self, PreviewItem, PreviewRequest, PreviewRequestError};
use crate::server::previews::sources::{self, PutSource, SourceItem, SourceRequestError};
use crate::server::router::IdeState;

#[derive(Serialize)]
pub struct PreviewList {
    pub items: Vec<PreviewItem>,
}

#[derive(Serialize)]
pub struct PreviewResponse {
    pub item: PreviewItem,
}

#[derive(Deserialize)]
pub struct CreatePreviewBody {
    pub branch: String,
}

#[derive(Deserialize)]
pub struct BranchParam {
    pub branch: String,
}

/// A refusal in the contract's shape.
pub struct PreviewApiError(StatusCode, &'static str, String, Option<&'static str>);

impl From<PreviewRequestError> for PreviewApiError {
    fn from(e: PreviewRequestError) -> Self {
        let status = e.status();
        if status.is_server_error() {
            tracing::error!(error = %e, "previews API failed");
        }
        Self(status, e.code(), e.to_string(), e.reason())
    }
}

impl From<RunRequestError> for PreviewApiError {
    fn from(e: RunRequestError) -> Self {
        let status = e.status();
        if status.is_server_error() {
            tracing::error!(error = %e, "previews runs API failed");
        }
        Self(status, e.code(), e.to_string(), None)
    }
}

impl From<SourceRequestError> for PreviewApiError {
    fn from(e: SourceRequestError) -> Self {
        let status = e.status();
        if status.is_server_error() {
            tracing::error!(error = %e, "previews sources API failed");
        }
        Self(status, e.code(), e.to_string(), None)
    }
}

impl IntoResponse for PreviewApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "code": self.1, "message": self.2 });
        // Only where one code covers several causes a client would act on
        // differently: why this pod cannot compile the branch.
        if let Some(reason) = self.3 {
            body["reason"] = reason.into();
        }
        (self.0, Json(body)).into_response()
    }
}

async fn db() -> Result<sea_orm::DatabaseConnection, PreviewApiError> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "previews API: database unavailable");
            PreviewApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "database temporarily unavailable".into(),
                None,
            )
        })
}

async fn one(
    db: &sea_orm::DatabaseConnection,
    row: entity::workspace_previews::Model,
) -> Result<PreviewResponse, PreviewApiError> {
    let item = service::items(db, vec![row])
        .await?
        .pop()
        .ok_or_else(|| PreviewRequestError::Internal("no preview to serve".into()))?;
    Ok(PreviewResponse { item })
}

/// List the branches staff are previewing, most recently touched first, each
/// with where its staging revision stands.
pub async fn list_previews(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
) -> Result<Json<PreviewList>, PreviewApiError> {
    let db = db().await?;
    let rows = crate::server::previews::store::list(&db, ws.id)
        .await
        .map_err(PreviewRequestError::from)?;
    Ok(Json(PreviewList {
        items: service::items(&db, rows).await?,
    }))
}

/// Preview a branch: compile its head into a staging revision (or reuse the
/// ready one for that commit).
pub async fn create_preview(
    State(_ide): State<IdeState>,
    _: WorkspacePreviewer,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Extension(ws): Extension<entity::workspaces::Model>,
    Json(body): Json<CreatePreviewBody>,
) -> Result<(StatusCode, Json<PreviewResponse>), PreviewApiError> {
    let db = db().await?;
    let row = service::create(&PreviewRequest {
        db: &db,
        workspace: &ws,
        branch: body.branch.trim(),
        requested_by: user.id,
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(one(&db, row).await?)))
}

/// Move a preview to its branch's current head.
pub async fn refresh_preview(
    State(_ide): State<IdeState>,
    _: WorkspacePreviewer,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Extension(ws): Extension<entity::workspaces::Model>,
    Query(q): Query<BranchParam>,
) -> Result<(StatusCode, Json<PreviewResponse>), PreviewApiError> {
    let db = db().await?;
    let row = service::refresh(&PreviewRequest {
        db: &db,
        workspace: &ws,
        branch: q.branch.trim(),
        requested_by: user.id,
    })
    .await?;
    Ok((StatusCode::ACCEPTED, Json(one(&db, row).await?)))
}

/// Stop listing a preview. Its revisions age out under staging retention.
pub async fn delete_preview(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
    Query(q): Query<BranchParam>,
) -> Result<StatusCode, PreviewApiError> {
    let db = db().await?;
    service::delete(&db, ws.id, q.branch.trim()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The Airway change check of a preview: each `.airway.yml` the branch changed,
/// and whether merging it needs a Reset schema. `pending` with no pipelines
/// until the check of the preview's current revision has run.
pub async fn get_checks(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
    Query(q): Query<BranchParam>,
) -> Result<Json<ChecksResponse>, PreviewApiError> {
    let db = db().await?;
    Ok(Json(service::checks(&db, ws.id, q.branch.trim()).await?))
}

#[derive(Deserialize)]
pub struct RunPath {
    pub run_id: String,
}

/// Every runs route answers 404 while `OXY_PREVIEW_RUNS` is off.
fn runs_gate() -> Result<(), PreviewApiError> {
    if runs::runs_enabled() {
        Ok(())
    } else {
        Err(RunRequestError::Disabled.into())
    }
}

/// Queue a held dry run of a procedure on a previewed branch.
pub async fn start_run(
    _: WorkspacePreviewer,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Extension(ws): Extension<entity::workspaces::Model>,
    Json(body): Json<SubmitRun>,
) -> Result<(StatusCode, Json<Submitted>), PreviewApiError> {
    runs_gate()?;
    let db = db().await?;
    let submitted = runs::submit(&db, ws.id, user.id, body).await?;
    Ok((StatusCode::ACCEPTED, Json(submitted)))
}

/// A branch's held runs, newest first.
pub async fn list_runs(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
    Query(q): Query<BranchParam>,
) -> Result<Json<Vec<RunSummary>>, PreviewApiError> {
    runs_gate()?;
    let db = db().await?;
    Ok(Json(runs::list(&db, ws.id, q.branch.trim()).await?))
}

/// One held run, with each step and what it would have written.
pub async fn get_run(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
    Path(p): Path<RunPath>,
) -> Result<Json<RunDetail>, PreviewApiError> {
    runs_gate()?;
    let db = db().await?;
    Ok(Json(runs::get(&db, ws.id, &p.run_id).await?))
}

/// The sandbox companies registered for Airway samples of rotate-on-use
/// (QuickBooks) pipelines.
pub async fn list_sources(
    _: WorkspacePreviewer,
    Extension(ws): Extension<entity::workspaces::Model>,
) -> Result<Json<Vec<SourceItem>>, PreviewApiError> {
    let db = db().await?;
    Ok(Json(sources::list(&db, ws.id).await?))
}

/// Register (or replace) a pipeline's sandbox company. Refuses production's
/// vars and realm, and a var another pipeline's sandbox already rotates.
pub async fn put_source(
    _: WorkspacePreviewer,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Extension(ws): Extension<entity::workspaces::Model>,
    Json(body): Json<PutSource>,
) -> Result<Json<SourceItem>, PreviewApiError> {
    let db = db().await?;
    Ok(Json(sources::save(&db, ws.id, user.id, body).await?))
}
