//! One call to compile a branch or a commit of a workspace, for a caller with
//! a person waiting on the answer: [`compile`], with [`status`] beside it.
//!
//! Any pod can answer both. `compile` finds the commit, answers with a ready
//! revision of it when there is one, and otherwise queues the compile and
//! returns; the queued task does the work and `status` reports it. Nothing
//! here compiles in the request.
//!
//! **Where the commit comes from.** A branch is looked up on GitHub
//! ([`from_git`]) and compiled from the commit GitHub has, by whichever pod
//! claims the `compile_git` task: no worktree, nothing pulled first, and
//! uncommitted edits on some node's disk are not part of it. When GitHub
//! cannot serve the branch — it was never pushed, or the workspace has no
//! remote or no connection ([`NotFromGit`]) — only a working copy has it. A
//! process that owns working copies then compiles its own, as every branch
//! compile did before ([`working_copy`]); a process that owns none answers
//! [`Refusal::NeedsWorkingCopy`], naming the reason, and its route sends the
//! request on to the node that has the files when there is one
//! (`server::factory_replay`).
//!
//! **Kinds.** `staging` is the only kind compiled here: a revision that is
//! never promoted, and that nothing reads unless it is pinned
//! (`internal-docs/compile-boundary.md` § "Staging revisions"). `main` is the
//! Compile button's and the periodic check's, and which branch it follows is
//! not this call's to decide; `draft` needs an owner and nothing queues one.
//!
//! This is not the path for an automatic trigger. Those go through
//! `enqueue_compile_deduped_as`, which backs off a commit that keeps failing;
//! a person asking again is the retry here.

mod from_git;
mod status;
mod working_copy;

use axum::http::StatusCode;
use oxy_compile::RevisionKind;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

pub use from_git::NotFromGit;
pub use status::{CompileState, compile_task_in_flight, status};

use from_git::Unresolved;

/// What to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// The commit this branch points at now.
    Branch(&'a str),
    /// One commit, by its full 40-character SHA. Always fetched: a working
    /// copy is read by branch, never by commit.
    Commit(&'a str),
}

/// Why nothing was compiled or queued.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("invalid branch: {0}")]
    InvalidBranch(String),
    #[error("{0}")]
    NotACommit(String),
    #[error(
        "a {0} revision cannot be compiled through this call; it compiles staging revisions only"
    )]
    UnsupportedKind(&'static str),
    /// Neither GitHub nor this node's working copy has the branch.
    #[error("{0}")]
    UnknownBranch(String),
    /// A state someone has to fix first: a detached workspace, uncommitted
    /// edits in the branch's worktree, a worktree that cannot be made.
    #[error("{0}")]
    Conflict(String),
    /// GitHub cannot serve the branch and this process has no working copy to
    /// read it from.
    #[error(
        "branch {branch:?} cannot be compiled on this server [{}]: {why}. This server holds \
         no working copy, so it compiles only what is on GitHub — push the branch, or ask \
         again when the node that holds the workspace's files is running",
        why.code()
    )]
    NeedsWorkingCopy { branch: String, why: NotFromGit },
    /// GitHub did not say where the branch is. Asking again may work.
    #[error("could not ask GitHub where branch {branch:?} is: {message}. Try again shortly")]
    GitHubUnavailable { branch: String, message: String },
    #[error("{0}")]
    Internal(String),
}

impl Refusal {
    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }

    /// Whether the node that holds the workspace's files could answer what
    /// this process refused.
    pub fn needs_working_copy(&self) -> bool {
        matches!(self, Self::NeedsWorkingCopy { .. })
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidBranch(_) | Self::NotACommit(_) | Self::UnsupportedKind(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::UnknownBranch(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) | Self::NeedsWorkingCopy { .. } => StatusCode::CONFLICT,
            Self::GitHubUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<sea_orm::DbErr> for Refusal {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Internal(format!("database error: {error}"))
    }
}

/// One commit to compile, and where its files are read from.
struct Commit {
    git_sha: String,
    branch: Option<String>,
    /// Fetched from GitHub by the pod that claims the task, rather than read
    /// from the branch's worktree on this node.
    from_git: bool,
}

/// Compile `target` of `workspace` as `kind`, or answer with what already
/// exists: a ready revision of the commit is reused, and a compile already
/// queued or running is reported rather than doubled. Returns as soon as the
/// task is queued; [`status`] says how it went.
#[tracing::instrument(skip_all, fields(workspace_id = %workspace.id, kind = kind.as_str()))]
pub async fn compile(
    db: &DatabaseConnection,
    workspace: &entity::workspaces::Model,
    target: Target<'_>,
    kind: RevisionKind,
) -> Result<CompileState, Refusal> {
    supported(kind)?;
    let commit = resolve(workspace, target).await?;
    let (ws, sha) = (workspace.id, commit.git_sha.clone());

    if let Some(revision_id) = oxy_compile::find_reusable_revision(db, ws, kind, &sha).await? {
        return Ok(CompileState::ready(ws, sha, revision_id));
    }
    // One in flight already: answer with it rather than compiling twice.
    if let Some(latest) = status::latest_for_sha(db, ws, &sha, kind).await?
        && latest.status == "compiling"
    {
        return Ok(status::from_row(ws, sha, latest));
    }
    // Queued but not yet claimed has no revision row, so the check above cannot
    // see it; the queue can. Without this a second request before a worker
    // picks the first up compiles the same commit twice.
    if compile_task_in_flight(db, ws, &sha).await? {
        return Ok(CompileState::pending(ws, sha, None));
    }

    let task_id = enqueue(db, ws, &commit, kind).await?;
    tracing::info!(
        %task_id,
        git_sha = %sha,
        branch = ?commit.branch,
        from_git = commit.from_git,
        "compile request: queued"
    );
    Ok(CompileState::pending(ws, sha, Some(task_id)))
}

/// The kinds this call compiles. See the module docs for why the other two
/// are refused rather than passed through.
fn supported(kind: RevisionKind) -> Result<(), Refusal> {
    match kind {
        RevisionKind::Staging => Ok(()),
        RevisionKind::Main | RevisionKind::Draft => Err(Refusal::UnsupportedKind(kind.as_str())),
    }
}

async fn resolve(
    workspace: &entity::workspaces::Model,
    target: Target<'_>,
) -> Result<Commit, Refusal> {
    let branch = match target {
        Target::Branch(branch) => branch.trim(),
        Target::Commit(sha) => {
            return Ok(Commit {
                git_sha: crate::server::compile_git::commit_sha(Some(sha))
                    .map_err(|e| Refusal::NotACommit(e.to_string()))?,
                branch: None,
                from_git: true,
            });
        }
    };
    // "Compile the current branch" from a detached workspace: there is no
    // branch whose head could be compiled, which is a state to fix, not a bad
    // name.
    if let Some(detached) = oxy_git::detached_label_refusal(branch) {
        return Err(Refusal::Conflict(detached.to_string()));
    }
    oxy_git::cli::branch::validate_branch_name(branch)
        .map_err(|e| Refusal::InvalidBranch(e.to_string()))?;

    let (git_sha, from_git) = match from_git::branch_head(workspace, branch).await {
        Ok(sha) => (sha, true),
        Err(Unresolved::Unavailable(message)) => {
            return Err(Refusal::GitHubUnavailable {
                branch: branch.to_string(),
                message,
            });
        }
        Err(Unresolved::NotFromGit(why)) => {
            if !oxy::workspace_fs_probe::process_owns_workspace_files() {
                return Err(Refusal::NeedsWorkingCopy {
                    branch: branch.to_string(),
                    why,
                });
            }
            (
                working_copy::branch_head(workspace, branch, &why).await?,
                false,
            )
        }
    };
    Ok(Commit {
        git_sha,
        branch: Some(branch.to_string()),
        from_git,
    })
}

/// Queue the compile. `promote` is `false` whatever is asked: the only kind
/// compiled here is one that must never become what the workspace serves.
async fn enqueue(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    commit: &Commit,
    kind: RevisionKind,
) -> Result<String, Refusal> {
    const PROMOTE: bool = false;
    let branch = commit.branch.as_deref();
    if commit.from_git {
        return Ok(crate::server::compile_git::enqueue(
            db,
            workspace_id,
            &commit.git_sha,
            branch,
            kind,
            PROMOTE,
        )
        .await?);
    }
    let branch = branch.ok_or_else(|| Refusal::internal("a working-copy compile has no branch"))?;
    crate::server::api::compile::enqueue_compile_task(
        db,
        workspace_id,
        &commit.git_sha,
        branch,
        kind,
        PROMOTE,
    )
    .await
    .map_err(|(_, message)| Refusal::Internal(message))
}
