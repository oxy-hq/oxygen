//! Branch compile into a **staging** revision — the semantic half of custom-app
//! staging (`internal-docs/customer-apps-staging.md` D4).
//!
//!   * `POST /{workspace_id}/compile/staging?branch=<b>` — FleetOk. Finds the
//!     branch's head commit, reuses a ready staging (or main) revision of that
//!     SHA, and otherwise queues a compile with `kind = staging`, never
//!     promoted. A branch GitHub does not have is replayed to the node with
//!     the working copy when there is one, and refused by name when not.
//!   * `GET /{workspace_id}/compile/staging/status?git_sha=<sha>` — FleetOk.
//!     Where that compile stands: `ready` + the revision id once it has
//!     compiled.
//!
//! Both are the HTTP face of `server::compile_request`, which says where the
//! commit comes from (GitHub, or the working copy for a branch GitHub does not
//! have) and is shared with the staff previews API (`server::previews`).
//!
//! A staging revision is never promoted and nothing that picks a revision by
//! itself ever lands on one; the only thing that reads it is a custom-app
//! draft build that pins it (`app_builds.semantic_revision_id`).
//! Mechanics: `internal-docs/compile-boundary.md` § "Staging revisions".

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_compile::RevisionKind;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Deserialize;
use uuid::Uuid;

use crate::server::api::middlewares::role_guards::WorkspaceEditor;
use crate::server::compile_request::{self, CompileState, Refusal, Target};
use crate::server::factory_replay::Arrived;

#[derive(Deserialize)]
pub struct StagingCompileQuery {
    /// The workspace branch to compile.
    pub branch: String,
}

#[derive(Deserialize)]
pub struct StagingStatusQuery {
    pub git_sha: String,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, String)>;

/// POST /{workspace_id}/compile/staging?branch=<b>
///
/// Same guard as the Compile button (`WorkspaceEditor`): compiling a branch
/// that is never promoted is strictly weaker than shipping main.
pub async fn enqueue_staging_compile(
    _: WorkspaceEditor,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<StagingCompileQuery>,
    arrived: Arrived,
) -> Result<Response, (StatusCode, String)> {
    let db = connect().await?;
    let workspace = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(workspace_id))?;
    let target = Target::Branch(&q.branch);
    let refusal =
        match compile_request::compile(&db, &workspace, target, RevisionKind::Staging).await {
            Ok(state) => return Ok(Json(state).into_response()),
            Err(refusal) => refusal,
        };
    // Only a working copy has this branch: the node that holds one answers.
    if refusal.needs_working_copy()
        && let Some(answer) = arrived.replayed_to_factory(Bytes::new()).await
    {
        return Ok(answer);
    }
    Err(refused(refusal))
}

/// GET /{workspace_id}/compile/staging/status?git_sha=<sha>
///
/// Reads only Postgres, so it is served on any replica. Guarded like its
/// POST: the answer names a revision of the workspace's model.
pub async fn staging_compile_status(
    _: WorkspaceEditor,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<StagingStatusQuery>,
) -> ApiResult<CompileState> {
    let db = connect().await?;
    compile_request::status(&db, workspace_id, q.git_sha.trim(), RevisionKind::Staging)
        .await
        .map(Json)
        .map_err(internal)
}

/// A refusal as this route has always answered one: the status and the reason
/// as plain text.
fn refused(refusal: Refusal) -> (StatusCode, String) {
    match refusal {
        Refusal::Internal(detail) => internal(detail),
        other => (other.status(), other.to_string()),
    }
}

async fn connect() -> Result<DatabaseConnection, (StatusCode, String)> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!(?e, "compile_staging: DB connect failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "database temporarily unavailable".into(),
            )
        })
}

fn not_found(workspace_id: Uuid) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        format!("workspace {workspace_id} not found"),
    )
}

fn internal<E: std::fmt::Debug>(err: E) -> (StatusCode, String) {
    tracing::error!(?err, "compile_staging endpoint internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal server error".into(),
    )
}
