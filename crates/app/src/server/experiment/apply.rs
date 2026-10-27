//! The proposed coefficient as a branch (`POST /experiments/{name}/write-back/apply`).
//!
//! The platform never writes `main` (spec § "Write-back"): the coefficient carries
//! the same git provenance as the design that earned it and becomes a branch only
//! when a person applies it. This is the IDE's protected-`main` flow
//! (`useSaveToNewBranch`) done server-side, with the same primitives. The HTTP
//! handler is `api::experiment::apply_write_back`.
//!
//! **Order is the safety property.** Everything that can refuse — the run is
//! reported, a patch exists, the file is on `HEAD`, the edit applies to that text —
//! happens before the branch exists, so a refusal never leaves one behind.

use std::path::Path;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use entity::experiment_results;
use oxy::github::default_git_client;
use oxy_experiment::patch::{CoefficientEdit, PatchRefusal, commit_message, render_patch};
use oxy_experiment::run::RunState;
use oxy_git::GitClient;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use super::store::run_by_name;
use crate::server::api::workspaces::{git_delete_branch, git_switch_branch};

pub struct ApplyCtx<'a> {
    pub db: &'a DatabaseConnection,
    pub workspace: Option<&'a entity::workspaces::Model>,
    pub workspace_id: Uuid,
    pub root: &'a Path,
    pub name: &'a str,
    pub user_id: Option<Uuid>,
    pub user_slug: &'a str,
    pub now: DateTime<Utc>,
}

/// C5: `{ branch, file_path }` and nothing else.
#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub branch: String,
    pub file_path: String,
}

#[derive(Debug)]
pub enum ApplyError {
    NoWorkingCopy,
    NoSuchWorkspace,
    NoSuchRun,
    HeadMoved,
    BranchExists { branch: String },
    AlreadyApplied,
    NotReported { state: String },
    NoPatch { reason: String },
    FileMissing { file_path: String },
    Refused(PatchRefusal),
    Corrupt(String),
    Git(String),
    Db(String),
}

impl IntoResponse for ApplyError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApplyError::NoWorkingCopy => (
                StatusCode::SERVICE_UNAVAILABLE,
                "this node holds no working copy".to_string(),
            ),
            ApplyError::NoSuchWorkspace => (StatusCode::NOT_FOUND, "no such workspace".to_string()),
            ApplyError::NoSuchRun => (StatusCode::NOT_FOUND, "no such experiment run".to_string()),
            ApplyError::HeadMoved => (
                StatusCode::CONFLICT,
                "the default branch moved while the write-back was being applied; retry"
                    .to_string(),
            ),
            ApplyError::BranchExists { branch } => (
                StatusCode::CONFLICT,
                format!("the branch `{branch}` already exists; retry in a moment"),
            ),
            ApplyError::AlreadyApplied => (
                StatusCode::CONFLICT,
                "the default branch already carries this coefficient; nothing to apply".to_string(),
            ),
            ApplyError::NotReported { state } => (
                StatusCode::CONFLICT,
                format!("the run is `{state}`; write-back is available once it is reported"),
            ),
            ApplyError::NoPatch { reason } => (StatusCode::CONFLICT, reason),
            ApplyError::FileMissing { file_path } => (
                StatusCode::CONFLICT,
                format!("`{file_path}` is not on the default branch any more"),
            ),
            ApplyError::Refused(r) => (StatusCode::CONFLICT, r.to_string()),
            ApplyError::Corrupt(e) | ApplyError::Git(e) | ApplyError::Db(e) => {
                tracing::error!(error = %e, "experiments: write-back apply failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "write-back apply failed".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// `<slug>/experiment-<name>-YYYY-MM-DD-HHMMSS`. Every character outside
/// `[A-Za-z0-9-]` (dots included, so `..` cannot appear) becomes `-`, repeats are
/// squeezed and `-` is trimmed from both ends, so no name can yield `--` (reserved
/// for the worktree directory encoding; `validate_branch_name` rejects it) or a
/// ref git refuses. A name with
/// nothing left falls back to a fixed segment.
pub fn branch_name(user_slug: &str, experiment: &str, now: DateTime<Utc>) -> String {
    let mut segment = String::new();
    for c in experiment.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '-' {
            c
        } else {
            '-'
        };
        if !(c == '-' && segment.ends_with('-')) {
            segment.push(c);
        }
    }
    let segment = segment.trim_matches('-');
    let segment = if segment.is_empty() {
        "experiment"
    } else {
        segment
    };
    format!(
        "{user_slug}/experiment-{segment}-{}",
        now.format("%Y-%m-%d-%H%M%S")
    )
}

/// The IDE hook's `toUserSlug`: the mailbox local part (or the name), lower-cased,
/// runs of non-alphanumerics collapsed to `-`.
pub fn user_slug(email: Option<&str>, name: &str) -> String {
    let raw = email
        .and_then(|e| e.split('@').next())
        .unwrap_or(name)
        .to_lowercase();
    let mut slug = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "user".to_string()
    } else {
        slug
    }
}

pub async fn apply_write_back(c: ApplyCtx<'_>) -> Result<ApplyResponse, ApplyError> {
    let edit = load_edit(c.db, c.workspace_id, c.name).await?;
    let git = default_git_client();
    // Everything that can refuse, before a branch exists. HEAD is read once and
    // the file is read at that sha, so the diff is against exactly what the
    // branch must start as.
    let head = rev_sha(c.root, "HEAD").await?;
    let before = git
        .file_at_rev(c.root, &edit.file_path, Some(&head))
        .await
        .map_err(|e| ApplyError::Git(e.to_string()))?;
    if before.is_empty() {
        return Err(ApplyError::FileMissing {
            file_path: edit.file_path,
        });
    }
    let rendered = render_patch(&before, &edit).map_err(ApplyError::Refused)?;

    let branch = branch_name(c.user_slug, c.name, c.now);
    // Names have second resolution: a same-second double submit, or a branch a
    // person made by that name, would be reused by the switch and then deleted by
    // the cleanup below. Refuse instead; only a branch this call creates is ever
    // deleted.
    if branch_taken(c.root, &branch).await {
        return Err(ApplyError::BranchExists { branch });
    }
    // The switch creates the ref before it creates the worktree, so it can fail
    // with the branch created: clean up on every failure from here on.
    match switch_and_commit(&git, &c, &branch, &head, &edit, &rendered.patched).await {
        Ok(()) => Ok(ApplyResponse {
            branch,
            file_path: edit.file_path,
        }),
        Err(e) => {
            // As the IDE hook does: never leave an orphaned branch behind.
            if let Err(d) = git_delete_branch(c.root, &branch).await {
                tracing::warn!(branch, error = %d, "experiments: could not delete the branch after a failed write-back");
            }
            Err(e)
        }
    }
}

/// `git rev-parse --verify <rev>` in `root`.
async fn rev_sha(root: &Path, rev: &str) -> Result<String, ApplyError> {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--verify", rev])
        .current_dir(root)
        .output()
        .await
        .map_err(|e| ApplyError::Git(e.to_string()))?;
    if !out.status.success() {
        return Err(ApplyError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether `branch` exists locally or as `origin/<branch>`. `ensure_local_ref`
/// reuses either, so a branch that exists is someone else's: never ours to
/// build on, and never ours to delete.
async fn branch_taken(root: &Path, branch: &str) -> bool {
    rev_sha(root, &format!("refs/heads/{branch}")).await.is_ok()
        || rev_sha(root, &format!("refs/remotes/origin/{branch}"))
            .await
            .is_ok()
}

async fn switch_and_commit(
    git: &impl GitClient,
    c: &ApplyCtx<'_>,
    branch: &str,
    head: &str,
    edit: &CoefficientEdit,
    patched: &str,
) -> Result<(), ApplyError> {
    git_switch_branch(c.root, branch, c.workspace, c.workspace_id, c.user_id)
        .await
        .map_err(|e| ApplyError::Git(e.to_string()))?;
    // HEAD moved between reading the file and forking the branch (an IDE commit,
    // a fetch): the patch was rendered against text the branch does not start
    // from and would silently revert the concurrent change.
    let tip = rev_sha(c.root, &format!("refs/heads/{branch}")).await?;
    if tip != head {
        return Err(ApplyError::HeadMoved);
    }
    write_and_commit(git, c.root, branch, edit, patched).await
}

async fn write_and_commit(
    git: &impl GitClient,
    root: &Path,
    branch: &str,
    edit: &CoefficientEdit,
    patched: &str,
) -> Result<(), ApplyError> {
    let git_err = |e: oxy_shared::errors::OxyError| ApplyError::Git(e.to_string());
    let worktree = git
        .get_or_create_worktree(root, branch)
        .await
        .map_err(git_err)?;
    git.write_and_stage_file(&worktree, &edit.file_path, patched)
        .await
        .map_err(git_err)?;
    // Names the prior the edit would replace, when the run froze one.
    let sha = git
        .commit_changes(&worktree, &commit_message(edit))
        .await
        .map_err(git_err)?;
    // Nothing committed: HEAD already carries the edit (it was merged since the
    // run was reported). The caller deletes the empty branch.
    if sha.is_empty() {
        return Err(ApplyError::AlreadyApplied);
    }
    tracing::info!(branch, commit = %sha, file = %edit.file_path, "experiments: write-back applied to a branch");
    Ok(())
}

async fn load_edit(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<CoefficientEdit, ApplyError> {
    let run = run_by_name(db, workspace_id, name)
        .await
        .map_err(|e| ApplyError::Db(e.to_string()))?
        .ok_or(ApplyError::NoSuchRun)?;
    if !matches!(RunState::parse(&run.state), Some(RunState::Reported)) {
        return Err(ApplyError::NotReported { state: run.state });
    }
    let row = experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run.id))
        .one(db)
        .await
        .map_err(|e| ApplyError::Db(e.to_string()))?
        .ok_or_else(|| ApplyError::NotReported {
            state: run.state.clone(),
        })?;
    let Some(patch) = row.coefficient_patch.as_deref() else {
        return Err(ApplyError::NoPatch {
            reason: super::results_api::no_patch_reason(&row.report),
        });
    };
    serde_json::from_str(patch).map_err(|e| {
        ApplyError::Corrupt(format!("the stored coefficient patch does not parse: {e}"))
    })
}
