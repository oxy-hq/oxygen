//! What the previews API does, behind its thin handlers
//! (`server::api::workspace_previews`). Compiling goes through the call
//! custom-app staging uses (`compile_request::compile`, `kind = staging`);
//! status is read back off `revisions` and the task queue
//! (`compile_request::status`), never stored twice.

use axum::http::StatusCode;
use chrono::SecondsFormat;
use entity::workspace_previews::Model;
use oxy_compile::RevisionKind;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait};
use serde::Serialize;
use uuid::Uuid;

use super::checks::{CheckSummary, ChecksResponse};
use crate::server::compile_request::{self, CompileState, Refusal, Target};

/// Why a preview request was refused. `InvalidBranch`, `DefaultBranch` and
/// `UnknownBranch` are the contract's 400s; `CannotCompile` is the staging
/// compile's own refusal (uncommitted changes in the branch's worktree, or a
/// workspace with no checkout) and stays a 409. `NeedsWorkingCopy` is that
/// same 409 with a `reason` beside it: the branch is not on GitHub and this
/// pod has no working copy to read it from.
#[derive(Debug, thiserror::Error)]
pub enum PreviewRequestError {
    #[error("{0}")]
    InvalidBranch(String),
    #[error("{0} is the workspace's default branch; it is already what the workspace serves")]
    DefaultBranch(String),
    #[error("{0}")]
    UnknownBranch(String),
    #[error("{0}")]
    CannotCompile(String),
    #[error("{message}")]
    NeedsWorkingCopy {
        reason: &'static str,
        message: String,
    },
    /// GitHub did not say where the branch is; asking again may work.
    #[error("{0}")]
    GitHubUnavailable(String),
    #[error("there is no preview of branch {0}")]
    NotFound(String),
    #[error("{0}")]
    Internal(String),
}

impl PreviewRequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidBranch(_) => "invalid_branch",
            Self::DefaultBranch(_) => "default_branch",
            Self::UnknownBranch(_) => "unknown_branch",
            Self::CannotCompile(_) | Self::NeedsWorkingCopy { .. } => "cannot_compile",
            Self::GitHubUnavailable(_) => "github_unavailable",
            Self::NotFound(_) => "preview_not_found",
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidBranch(_) | Self::DefaultBranch(_) | Self::UnknownBranch(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::CannotCompile(_) | Self::NeedsWorkingCopy { .. } => StatusCode::CONFLICT,
            Self::GitHubUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Whether the node that holds the workspace's files could answer what
    /// this process refused — the one refusal a route sends on to it. Asked
    /// separately from [`Self::reason`], so a refusal that gains a reason
    /// later is not replayed by accident.
    pub fn needs_working_copy(&self) -> bool {
        matches!(self, Self::NeedsWorkingCopy { .. })
    }

    /// Why this pod cannot compile the branch, when that is the refusal: the
    /// stable name a client branches on (`branch_not_pushed`, …).
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::NeedsWorkingCopy { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// The compile call's refusals, under the names this API has always used.
impl From<Refusal> for PreviewRequestError {
    fn from(refusal: Refusal) -> Self {
        let message = refusal.to_string();
        match refusal {
            Refusal::InvalidBranch(_) => Self::InvalidBranch(message),
            Refusal::UnknownBranch(_) => Self::UnknownBranch(message),
            Refusal::Conflict(_) => Self::CannotCompile(message),
            Refusal::NeedsWorkingCopy { why, .. } => Self::NeedsWorkingCopy {
                reason: why.code(),
                message,
            },
            Refusal::GitHubUnavailable { .. } => Self::GitHubUnavailable(message),
            // A preview asks for a branch as a staging revision, so neither of
            // the first two can come back.
            Refusal::NotACommit(_) | Refusal::UnsupportedKind(_) | Refusal::Internal(_) => {
                Self::Internal(message)
            }
        }
    }
}

impl From<sea_orm::DbErr> for PreviewRequestError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Internal(format!("database error: {e}"))
    }
}

/// Syntax only — no git, so a fleet route can refuse a bad name without a disk.
pub fn validate_branch_name(branch: &str) -> Result<(), PreviewRequestError> {
    oxy_git::cli::branch::validate_branch_name(branch)
        .map_err(|e| PreviewRequestError::InvalidBranch(e.to_string()))
}

/// Everything the request side needs about the caller and workspace.
pub struct PreviewRequest<'a> {
    pub db: &'a DatabaseConnection,
    pub workspace: &'a entity::workspaces::Model,
    pub branch: &'a str,
    pub requested_by: Uuid,
}

impl PreviewRequest<'_> {
    /// Compile the branch head (or reuse its ready revision) and record the
    /// preview at that commit.
    async fn stage(&self) -> Result<Model, PreviewRequestError> {
        // Previewing "the current branch" of a detached workspace: no branch
        // to preview, which is the compile's kind of refusal (409), not a
        // malformed name (400).
        if let Some(detached) = oxy_git::detached_label_refusal(self.branch) {
            return Err(PreviewRequestError::CannotCompile(detached.to_string()));
        }
        validate_branch_name(self.branch)?;
        // Local git where there is a checkout, the recorded branch where there
        // is none (`server::default_branch`).
        let default =
            crate::server::default_branch::resolve_default_branch(self.db, self.workspace.id).await;
        if default.as_deref() == Some(self.branch) {
            return Err(PreviewRequestError::DefaultBranch(self.branch.to_string()));
        }
        let target = Target::Branch(self.branch);
        let staged =
            compile_request::compile(self.db, self.workspace, target, RevisionKind::Staging)
                .await?;
        let row = self.record(&staged.git_sha).await?;
        queue_check(self.db, self.workspace.id, self.branch, &staged).await;
        Ok(row)
    }

    /// Point the preview at `git_sha`, then release the commit it was at before
    /// when that changed (a refresh, or a create of a branch whose head moved):
    /// from the upsert on, the pin no longer serves that commit's revision.
    async fn record(&self, git_sha: &str) -> Result<Model, PreviewRequestError> {
        let ws = self.workspace.id;
        let previous = super::store::find(self.db, ws, self.branch).await?;
        let row =
            super::store::upsert(self.db, ws, self.branch, git_sha, self.requested_by).await?;
        if let Some(previous) = previous.filter(|p| p.git_sha != git_sha) {
            self.release_superseded(&previous.git_sha).await;
        }
        Ok(row)
    }

    /// Cancel this preview's queued runs of the superseded commit, and delete its
    /// staging revision when nothing else uses it. Best-effort: the preview has
    /// already moved (so the pin has stopped serving the old revision), and
    /// whatever is left here, retention reclaims.
    async fn release_superseded(&self, old_sha: &str) {
        let ws = self.workspace.id;
        let key = super::namespace::preview_key(ws, self.branch);
        let result = async {
            super::runs::cancel_queued_at(self.db, ws, &key, old_sha).await?;
            super::revisions::release(self.db, ws, old_sha).await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(workspace_id = %ws, branch = %self.branch, %old_sha, %error,
                "previews: could not release the superseded revision; retention will");
        }
    }
}

/// Queue the Airway change check of a preview's revision once it is ready,
/// after the preview row is written. A reused revision is ready now and no
/// compile will finish to ask. A compile that lands between the compile call and
/// the upsert found no preview at this commit and asked nothing, so a revision
/// that was not ready is looked at once more here: of the two, at least one
/// sees both the ready revision and the preview row. Idempotent per revision;
/// best-effort, as the compile worker's own request is.
pub(crate) async fn queue_check(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
    staged: &CompileState,
) {
    let ready = match (staged.revision_id, staged.status.as_str()) {
        (Some(rev), "ready") => Ok(Some(rev)),
        _ => compile_request::status(db, workspace_id, &staged.git_sha, RevisionKind::Staging)
            .await
            .map(|s| s.revision_id.filter(|_| s.status == "ready"))
            .map_err(|e| e.to_string()),
    };
    let result = match ready {
        Ok(Some(revision_id)) => {
            super::analyze::ensure_enqueued(db, workspace_id, branch, revision_id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        Ok(None) => Ok(()),
        Err(message) => Err(message),
    };
    if let Err(error) = result {
        tracing::warn!(%workspace_id, %branch, %error,
            "previews: could not queue the Airway change check; a refresh will ask again");
    }
}

/// Start previewing a branch at its head. Idempotent: a branch already
/// previewed at that commit keeps its revision (the staging compile reuses a
/// ready revision of the same SHA, and never doubles one in flight).
pub async fn create(req: &PreviewRequest<'_>) -> Result<Model, PreviewRequestError> {
    req.stage().await
}

/// Move a preview to its branch's current head: a new staging revision when the
/// head moved (or the last compile failed or went stale), the same one when not.
pub async fn refresh(req: &PreviewRequest<'_>) -> Result<Model, PreviewRequestError> {
    validate_branch_name(req.branch)?;
    if super::store::find(req.db, req.workspace.id, req.branch)
        .await?
        .is_none()
    {
        return Err(PreviewRequestError::NotFound(req.branch.to_string()));
    }
    req.stage().await
}

/// Delete a preview: cancel its queued runs, make its Airhouse schemas due now
/// (so the next TTL sweep, `previews::maintenance`, queues their drop rather
/// than waiting out the TTL), and — with the row, in one transaction — release
/// its staging revision (`previews::revisions`), so a request still naming it
/// is no longer served from it. A run already running finishes (and holds the
/// key, and its revision, until it does). Idempotent.
pub async fn delete(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
) -> Result<(), PreviewRequestError> {
    validate_branch_name(branch)?;
    let key = super::namespace::preview_key(workspace_id, branch);
    super::runs::cancel_queued(db, workspace_id, &key).await?;
    super::registry::expire_key(db, workspace_id, &key).await?;
    let txn = db.begin().await?;
    if let Some(git_sha) = super::store::delete(&txn, workspace_id, branch).await? {
        super::revisions::release(&txn, workspace_id, &git_sha).await?;
    }
    txn.commit().await?;
    Ok(())
}

/// One preview, as the API serves it.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct PreviewItem {
    pub branch: String,
    /// Set exactly when `status` is `ready`: the id a preview request sends as
    /// `x-oxy-preview-revision`.
    pub revision_id: Option<String>,
    pub sha: Option<String>,
    /// `compiling` | `ready` | `failed` | `stale`.
    pub status: String,
    pub error: Option<String>,
    pub created_by: Option<PreviewCreator>,
    /// ISO-8601 UTC.
    pub updated_at: String,
    /// ISO-8601 UTC: when the ready revision finished compiling.
    pub compiled_at: Option<String>,
    /// The Airway change check of the ready revision; `None` until one is
    /// queued for it (e.g. still compiling).
    pub checks: Option<CheckSummary>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct PreviewCreator {
    pub id: String,
    pub name: String,
}

fn iso_utc(t: &chrono::DateTime<chrono::FixedOffset>) -> String {
    t.with_timezone(&chrono::Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Where a preview's compile stands, from `revisions` and the task queue.
///
/// The staging status is `pending` both for "queued, no revision row yet" and
/// for "no trace at all" (the compile was cancelled, or retention took the
/// revision). The queue tells them apart: queued or running is still
/// `compiling`; nothing is `stale`, and a refresh compiles it again. A compile
/// that failed before it wrote a revision is neither: the status read already
/// answered `failed`, with the task's reason.
async fn status_of(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<(CompileState, Option<String>), PreviewRequestError> {
    let mut staged =
        compile_request::status(db, workspace_id, git_sha, RevisionKind::Staging).await?;
    if staged.status == "pending" {
        staged.status =
            if compile_request::compile_task_in_flight(db, workspace_id, git_sha).await? {
                "compiling"
            } else {
                "stale"
            }
            .into();
    }
    let compiled_at = match staged.revision_id {
        Some(id) => entity::revisions::Entity::find_by_id(id)
            .one(db)
            .await?
            .and_then(|r| r.finished_at)
            .map(|t| iso_utc(&t)),
        None => None,
    };
    Ok((staged, compiled_at))
}

/// Rows to API items, with each creator's name looked up in one query.
pub async fn items(
    db: &DatabaseConnection,
    rows: Vec<Model>,
) -> Result<Vec<PreviewItem>, PreviewRequestError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.created_by).collect();
    let names: std::collections::HashMap<Uuid, String> = if ids.is_empty() {
        Default::default()
    } else {
        entity::users::Entity::find()
            .filter(entity::users::Column::Id.is_in(ids))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u.name))
            .collect()
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let (staged, compiled_at) = status_of(db, r.workspace_id, &r.git_sha).await?;
        let checks = match staged.revision_id {
            Some(rev) => super::checks::outcome(db, r.workspace_id, rev)
                .await?
                .map(|o| o.summary()),
            None => None,
        };
        out.push(PreviewItem {
            created_by: r.created_by.map(|id| PreviewCreator {
                id: id.to_string(),
                name: names.get(&id).cloned().unwrap_or_default(),
            }),
            branch: r.branch,
            revision_id: staged.revision_id.map(|id| id.to_string()),
            sha: Some(r.git_sha),
            status: staged.status,
            error: staged.error,
            updated_at: iso_utc(&r.updated_at),
            compiled_at,
            checks,
        });
    }
    Ok(out)
}

/// The Airway change check of `branch`'s preview, at the revision it is
/// previewing now. A branch whose staging compile failed has no revision to
/// check and never will at that commit: `failed`, with the compile's error.
pub async fn checks(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
) -> Result<ChecksResponse, PreviewRequestError> {
    validate_branch_name(branch)?;
    let preview = super::store::find(db, workspace_id, branch)
        .await?
        .ok_or_else(|| PreviewRequestError::NotFound(branch.to_string()))?;
    let (staged, _) = status_of(db, workspace_id, &preview.git_sha).await?;
    if staged.status == "failed" {
        return Ok(ChecksResponse::compile_failed(branch, staged.error));
    }
    Ok(super::checks::for_preview(db, workspace_id, branch, staged.revision_id).await?)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
