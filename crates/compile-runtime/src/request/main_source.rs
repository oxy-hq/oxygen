//! Where an operator's "compile `main` now" reads its files from, decided the
//! way [`super::compile`] decides it for a branch: GitHub first, and a working
//! copy only on a process that has one.
//!
//! A working-copy compile (`from_git: false`, run `source_type` `compile`) is
//! claimable only by a process whose role owns workspace files
//! (`worker`'s `!from_git && !process_can_compile()` refusal). A
//! deployment with no such node (prod, INC-164) leaves one queued forever, its
//! run showing `running`. So a process that owns none must not queue one.

use entity::workspaces;

use super::Refusal;
use super::from_git::{Unresolved, branch_head};
use oxy_app_core::workspace_repo_facts::stored_default_branch;

/// What an operator's compile of `main` will read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainSource {
    /// The commit GitHub has at the branch head; any pod can compile it.
    Git { git_sha: String, branch: String },
    /// The working copy, on a process that holds one.
    WorkingCopy,
}

/// Decide the source for compiling `workspace`'s `branch` (the workspace's
/// default branch when `None`).
///
/// `owns_working_copy` is whether *this process* can claim a working-copy
/// compile, passed in so the no-Factory branch is reachable from a test: only
/// the handler reads the real process role.
pub async fn main_source(
    workspace: &workspaces::Model,
    branch: Option<&str>,
    owns_working_copy: bool,
) -> Result<MainSource, Refusal> {
    let branch = branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .or_else(|| stored_default_branch(workspace));
    let Some(branch) = branch else {
        return no_branch(owns_working_copy);
    };
    oxy_git::cli::branch::validate_branch_name(&branch)
        .map_err(|e| Refusal::InvalidBranch(e.to_string()))?;
    match branch_head(workspace, &branch).await {
        Ok(git_sha) => Ok(MainSource::Git { git_sha, branch }),
        Err(Unresolved::Unavailable(message)) => {
            Err(Refusal::GitHubUnavailable { branch, message })
        }
        Err(Unresolved::NotFromGit(_)) if owns_working_copy => Ok(MainSource::WorkingCopy),
        Err(Unresolved::NotFromGit(why)) => Err(Refusal::NeedsWorkingCopy { branch, why }),
    }
}

/// Nothing says which branch to ask GitHub about.
fn no_branch(owns_working_copy: bool) -> Result<MainSource, Refusal> {
    if owns_working_copy {
        return Ok(MainSource::WorkingCopy);
    }
    Err(Refusal::Conflict(
        "this workspace records no default branch and the request names none, so GitHub cannot \
         be asked for a commit. This server holds no working copy, so it compiles only what is \
         on GitHub — pass `branch`, or run it where the workspace's files live"
            .to_string(),
    ))
}
