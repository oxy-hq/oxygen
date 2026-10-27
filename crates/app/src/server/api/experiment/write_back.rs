//! `POST /experiments/{name}/write-back/apply`. Thin: guard, resolve the
//! working copy, call `server::experiment::apply`. IdeOnly: it creates a git
//! worktree and branch, and `WorkspaceRootWorkingCopy` resolves only from `IdeState`.

use axum::Json;
use axum::extract::Path;
use chrono::Utc;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use super::connect;
use crate::server::api::middlewares::role_guards::ExperimentWriteBackApplier;
use crate::server::api::middlewares::workspace_context::WorkspaceRootWorkingCopy;
use crate::server::experiment::apply::{
    ApplyCtx, ApplyError, ApplyResponse, apply_write_back as apply, user_slug,
};

pub async fn apply_write_back(
    _: ExperimentWriteBackApplier,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    root: WorkspaceRootWorkingCopy,
    Path((_workspace_id, name)): Path<(Uuid, String)>,
) -> Result<Json<ApplyResponse>, ApplyError> {
    let ws = root.workspace.as_ref().ok_or(ApplyError::NoSuchWorkspace)?;
    let path = root.root_path().ok_or(ApplyError::NoWorkingCopy)?;
    let db = connect().await.map_err(|(_, e)| ApplyError::Db(e))?;
    apply(ApplyCtx {
        db: &db,
        workspace: Some(ws),
        workspace_id: ws.id,
        root: &path,
        name: &name,
        user_id: Some(user.id),
        user_slug: &user_slug(user.email.as_deref(), &user.name),
        now: Utc::now(),
    })
    .await
    .map(Json)
}
