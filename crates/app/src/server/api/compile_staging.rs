//! Branch compile into a **staging** revision — the semantic half of custom-app
//! staging (`internal-docs/customer-apps-staging.md` D4).
//!
//!   * `POST /{workspace_id}/compile/staging?branch=<b>` — IdeOnly. Resolves
//!     the branch's head commit and worktree on the IDE node, reuses a ready
//!     staging (or main) revision of that SHA, and otherwise enqueues the
//!     ordinary durable `TaskSpec::Compile` with `kind = staging`,
//!     `promote = false`.
//!   * `GET /{workspace_id}/compile/staging/status?git_sha=<sha>` — FleetOk.
//!     A pure `revisions` read: `ready` + the revision id once it has
//!     compiled.
//!
//! A staging revision is never promoted and nothing that picks a revision by
//! itself ever lands on one; the only thing that reads it is a custom-app
//! draft build that pins it (`app_builds.semantic_revision_id`).
//! Mechanics: `internal-docs/compile-boundary.md` § "Staging revisions".

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy_compile::RevisionKind;
use oxy_git::GitClient;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::middlewares::role_guards::WorkspaceEditor;

#[derive(Deserialize)]
pub struct StagingCompileQuery {
    /// The workspace branch to compile. Must exist on the IDE's repository.
    pub branch: String,
}

#[derive(Deserialize)]
pub struct StagingStatusQuery {
    pub git_sha: String,
}

/// Answer of both endpoints. `revision_id` is set exactly when `status` is
/// `ready`: that id is what `oxyc publish --semantic-branch` sends as
/// `semantic_revision_id`.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct StagingCompileResponse {
    pub workspace_id: Uuid,
    pub git_sha: String,
    /// `ready` | `compiling` | `pending` (queued, no revision row yet) |
    /// `stale` (a ready revision this server won't reuse; re-POST once) | `failed`.
    pub status: String,
    pub revision_id: Option<Uuid>,
    /// The enqueued compile task, when this call enqueued one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
) -> ApiResult<StagingCompileResponse> {
    let db = connect().await?;
    let workspace = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(workspace_id))?;
    Ok(Json(stage_branch(&db, &workspace, &q.branch).await?))
}

/// Compile `branch`'s head into a staging revision, or answer with the one that
/// already exists: a ready revision of that SHA is reused, a compile already in
/// flight is reported rather than doubled. The body of the POST above, shared
/// with the staff previews API (`server::previews`), which calls it behind its
/// own guard. IDE-only: it reads `.git` and the branch's worktree.
pub(crate) async fn stage_branch(
    db: &DatabaseConnection,
    workspace: &entity::workspaces::Model,
    branch: &str,
) -> Result<StagingCompileResponse, (StatusCode, String)> {
    let workspace_id = workspace.id;
    let branch = branch.trim().to_string();
    let git_sha = resolve_branch_head(workspace, &branch).await?;

    if let Some(revision_id) =
        oxy_compile::find_reusable_revision(db, workspace_id, RevisionKind::Staging, &git_sha)
            .await
            .map_err(internal)?
    {
        return Ok(ready(workspace_id, git_sha, revision_id));
    }
    // One in flight already: answer with it rather than compiling twice.
    if let Some(latest) = latest_for_sha(db, workspace_id, &git_sha).await?
        && latest.status == "compiling"
    {
        return Ok(from_row(workspace_id, git_sha, latest));
    }
    // Queued but not yet claimed has no revision row, so the check above cannot
    // see it; the queue can. Without this a second request before a worker
    // picks the first up compiles the same commit twice.
    if compile_task_in_flight(db, workspace_id, &git_sha)
        .await
        .map_err(internal)?
    {
        return Ok(StagingCompileResponse {
            workspace_id,
            git_sha,
            status: "pending".into(),
            revision_id: None,
            task_id: None,
            error: None,
        });
    }

    let task_id = crate::server::api::compile::enqueue_compile_task(
        db,
        workspace_id,
        &git_sha,
        &branch,
        RevisionKind::Staging,
        false,
    )
    .await?;
    tracing::info!(%workspace_id, %task_id, %branch, %git_sha, "compile: staging compile enqueued");
    Ok(StagingCompileResponse {
        workspace_id,
        git_sha,
        status: "pending".into(),
        revision_id: None,
        task_id: Some(task_id),
        error: None,
    })
}

/// GET /{workspace_id}/compile/staging/status?git_sha=<sha>
///
/// Reads only `revisions`, so it is served on any replica. Guarded like its
/// POST: the answer names a revision of the workspace's model.
pub async fn staging_compile_status(
    _: WorkspaceEditor,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<StagingStatusQuery>,
) -> ApiResult<StagingCompileResponse> {
    let db = connect().await?;
    Ok(Json(
        status_for_sha(&db, workspace_id, q.git_sha.trim()).await?,
    ))
}

/// Where the staging compile of `git_sha` stands — the body of the status route,
/// shared with the previews list. Reads only `revisions`.
pub(crate) async fn status_for_sha(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<StagingCompileResponse, (StatusCode, String)> {
    let git_sha = git_sha.to_string();
    if let Some(revision_id) =
        oxy_compile::find_reusable_revision(db, workspace_id, RevisionKind::Staging, &git_sha)
            .await
            .map_err(internal)?
    {
        return Ok(ready(workspace_id, git_sha, revision_id));
    }
    Ok(match latest_for_sha(db, workspace_id, &git_sha).await? {
        Some(row) => from_row(workspace_id, git_sha, row),
        None => StagingCompileResponse {
            workspace_id,
            git_sha,
            status: "pending".into(),
            revision_id: None,
            task_id: None,
            error: None,
        },
    })
}

/// The branch's head commit, after making sure its worktree exists on this
/// (IDE) node so the compile worker can read it.
///
/// Refuses, rather than guesses, in the cases where the SHA would not describe
/// what gets compiled: an unknown branch (`get_or_create_worktree` would
/// otherwise CREATE it from main's HEAD), a workspace with no repository, a
/// worktree with uncommitted edits (the revision would carry the branch head's
/// SHA but content that is on no commit), and the detached-HEAD label, which
/// names no branch at all.
async fn resolve_branch_head(
    workspace: &entity::workspaces::Model,
    branch: &str,
) -> Result<String, (StatusCode, String)> {
    let root = workspace
        .path
        .as_deref()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "workspace has no on-disk path — a branch cannot be compiled here".to_string(),
            )
        })?;
    let git = oxy::github::default_git_client();
    // "Stage the current branch" from a detached workspace: there is no branch
    // whose head could be staged, which is a state to fix, not a bad name.
    if let Some(detached) = oxy_git::detached_label_refusal(branch) {
        return Err((StatusCode::CONFLICT, detached.to_string()));
    }
    git.validate_branch_name(branch)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid branch: {e}")))?;
    let (sha, _subject) = git.get_branch_commit(&root, branch).await;
    if sha.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!(
                "branch {branch:?} does not exist in workspace {} — push it and pull it into the workspace first",
                workspace.id
            ),
        ));
    }
    if let Err(e) = git.get_or_create_worktree(&root, branch).await {
        return Err(worktree_add_conflict(&git, &root, branch, e).await);
    }
    let worktree = oxy::adapters::workspace::effective_workspace_path(workspace, Some(branch))
        .await
        .map_err(internal)?;
    if !oxy_git::cli::worktree::is_worktree_clean(&worktree)
        .await
        .unwrap_or(false)
    {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "branch {branch:?} has uncommitted changes in the workspace — commit them so the staging revision matches a commit"
            ),
        ));
    }
    Ok(sha)
}

/// Turns a `get_or_create_worktree` failure into a 409, not a bare 500.
///
/// `get_or_create_worktree` already reuses a branch's existing checkout when
/// one exists (see `oxy_git::cli::worktree::find_branch_checkout`), so a
/// failure here means something changed the state out from under that check
/// — most plausibly a race where another process checked the branch out
/// between the two calls. Either way this is a conflict the caller can
/// retry or resolve, not a server bug: name the checkout that is in the way
/// when we can find one, and fall back to the raw git error otherwise.
async fn worktree_add_conflict(
    git: &impl GitClient,
    root: &std::path::Path,
    branch: &str,
    err: oxy_shared::errors::OxyError,
) -> (StatusCode, String) {
    tracing::error!(%branch, error = %err, "compile_staging: worktree add failed");
    let message = match git.find_branch_checkout(root, branch).await {
        Ok(Some(path)) => format!(
            "branch {branch:?} is checked out at {}; staging compile cannot add a second worktree for it",
            path.display()
        ),
        _ => format!("could not prepare a worktree for branch {branch:?}: {err}"),
    };
    (StatusCode::CONFLICT, message)
}

/// Whether a compile of `git_sha` for this workspace is queued or running on the
/// task queue — the one state `revisions` cannot show, because a compile writes
/// its revision row only once a worker has claimed it.
pub(crate) async fn compile_task_in_flight(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<bool, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM agentic_task_queue \
             WHERE queue_status IN ('queued', 'claimed') \
               AND spec->>'type' = 'compile' \
               AND spec->>'workspace_id' = $1 \
               AND spec->>'git_sha' = $2 \
             LIMIT 1",
            [workspace_id.to_string().into(), git_sha.into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// Newest staging-or-main revision row of this SHA, whatever its status.
async fn latest_for_sha(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<Option<entity::revisions::Model>, (StatusCode, String)> {
    entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::GitSha.eq(git_sha))
        .filter(entity::revisions::Column::Kind.is_in(["staging", "main"]))
        .order_by_desc(entity::revisions::Column::StartedAt)
        .one(db)
        .await
        .map_err(internal)
}

fn ready(workspace_id: Uuid, git_sha: String, revision_id: Uuid) -> StagingCompileResponse {
    StagingCompileResponse {
        workspace_id,
        git_sha,
        status: "ready".into(),
        revision_id: Some(revision_id),
        task_id: None,
        error: None,
    }
}

/// A non-reusable row: in flight, failed, or superseded. A `ready` row that
/// [`oxy_compile::find_reusable_revision`] skipped (another compiler or schema
/// version, e.g. a deploy landed mid-poll) reports `stale`, its own status and
/// not `pending`: `pending` also means "queued, no row yet", where a re-POST
/// would enqueue a second compile. On `stale` the caller re-POSTs once, which
/// enqueues a fresh compile rather than pinning a revision this build can't read.
fn from_row(
    workspace_id: Uuid,
    git_sha: String,
    row: entity::revisions::Model,
) -> StagingCompileResponse {
    let (status, error) = match row.status.as_str() {
        "compiling" => ("compiling", None),
        "failed" => (
            "failed",
            Some(
                row.error_summary
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "compile failed".into()),
            ),
        ),
        "ready" => ("stale", None),
        _ => ("pending", None),
    };
    StagingCompileResponse {
        workspace_id,
        git_sha,
        status: status.into(),
        revision_id: None,
        task_id: None,
        error,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str) -> entity::revisions::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::revisions::Model {
            revision_id: Uuid::new_v4(),
            workspace_id: Uuid::nil(),
            git_sha: "abc".into(),
            branch: Some("feature".into()),
            schema_version: 1,
            status: status.into(),
            kind: "staging".into(),
            owner_user_id: None,
            compiler_version: "x".into(),
            started_at: now,
            finished_at: None,
            file_count_seen: 0,
            file_count_compiled: 0,
            file_count_failed: 0,
            error_summary: None,
        }
    }

    #[test]
    fn a_non_reusable_row_never_hands_out_a_revision_id() {
        for s in ["compiling", "failed", "ready", "superseded"] {
            let r = from_row(Uuid::nil(), "abc".into(), row(s));
            assert_eq!(r.revision_id, None, "status {s}");
        }
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("failed")).status,
            "failed"
        );
        // A ready row this server won't reuse is `stale` (the CLI re-POSTs);
        // `pending` stays "queued, nothing to recompile", which it must not.
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("ready")).status,
            "stale"
        );
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("superseded")).status,
            "pending"
        );
    }
}
