//! Asking GitHub where a branch is, and saying why it could not answer.
//!
//! The same workspace-level connection the compile worker fetches with
//! ([`github_token_for_workspace`], never the caller's own token): a head only
//! the caller's token can see would resolve here and then fail in the worker.

use entity::workspaces;
use oxy::github::{BranchHeadError, GitHubClient, github_token_for_workspace};

use crate::server::compile_git::{CompileGitError, github_repository};

/// Why GitHub is not where this branch can be compiled from. None of these
/// passes on its own: someone has to push, connect GitHub, or compile once on
/// the node that holds the files. Until then only a working copy has the
/// branch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotFromGit {
    #[error("the workspace has no git remote")]
    NoRemote,
    #[error("the workspace's remote {0:?} is not a github.com repository")]
    UnsupportedRemote(String),
    #[error("the workspace has no GitHub connection (an App installation or a token) to read with")]
    NoConnection,
    #[error("GitHub refused the workspace's token for {owner}/{repo} (HTTP {status})")]
    Denied {
        owner: String,
        repo: String,
        status: u16,
    },
    #[error(
        "GitHub has no branch {branch:?} in {owner}/{repo}, or the workspace's connection \
         cannot see that repository"
    )]
    BranchNotFound {
        owner: String,
        repo: String,
        branch: String,
    },
    #[error(
        "the workspace has no compiled revision yet, and its first compile runs on the node \
         that holds its working copy"
    )]
    NothingCompiled,
}

impl NotFromGit {
    /// A stable name for the reason, for a client that branches on it.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRemote => "no_git_remote",
            Self::UnsupportedRemote(_) => "unsupported_remote",
            Self::NoConnection => "no_github_connection",
            Self::Denied { .. } => "github_denied",
            Self::BranchNotFound { .. } => "branch_not_pushed",
            Self::NothingCompiled => "nothing_compiled",
        }
    }
}

/// Why a branch head is not known.
#[derive(Debug)]
pub(super) enum Unresolved {
    /// Git is not the source for this branch; a working copy may be.
    NotFromGit(NotFromGit),
    /// GitHub, or minting a token, did not answer. Asking again may work, and
    /// until it does nothing says which commit the branch is at.
    Unavailable(String),
}

/// The commit `branch` points at on GitHub, for a workspace a commit compile
/// can serve.
pub(super) async fn branch_head(
    workspace: &workspaces::Model,
    branch: &str,
) -> Result<String, Unresolved> {
    let (owner, repo) = github_repository(workspace).map_err(|e| {
        Unresolved::NotFromGit(match e {
            CompileGitError::UnsupportedRemote(remote) => NotFromGit::UnsupportedRemote(remote),
            _ => NotFromGit::NoRemote,
        })
    })?;
    // A worker builds a workspace's context from its promoted revision before
    // it drives any of that workspace's runs, so with nothing promoted a
    // commit compile is never claimed off the node that holds the files
    // (`internal-docs/factory-retirement.md`, phase 1).
    if workspace.current_revision_id.is_none() {
        return Err(Unresolved::NotFromGit(NotFromGit::NothingCompiled));
    }
    let token = github_token_for_workspace(workspace)
        .await
        .map_err(|e| Unresolved::Unavailable(format!("could not get a GitHub token: {e}")))?
        .ok_or(Unresolved::NotFromGit(NotFromGit::NoConnection))?;
    let client = GitHubClient::from_token(token)
        .map_err(|e| Unresolved::Unavailable(format!("could not build a GitHub client: {e}")))?;
    client
        .branch_head(&owner, &repo, branch)
        .await
        .map_err(unresolved)
}

fn unresolved(error: BranchHeadError) -> Unresolved {
    match error {
        BranchHeadError::NotFound {
            owner,
            repo,
            branch,
        } => Unresolved::NotFromGit(NotFromGit::BranchNotFound {
            owner,
            repo,
            branch,
        }),
        BranchHeadError::Denied {
            owner,
            repo,
            status,
        } => Unresolved::NotFromGit(NotFromGit::Denied {
            owner,
            repo,
            status,
        }),
        e @ (BranchHeadError::RateLimited { .. } | BranchHeadError::Unavailable(_)) => {
            Unresolved::Unavailable(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_found() -> BranchHeadError {
        BranchHeadError::NotFound {
            owner: "acme".into(),
            repo: "analytics".into(),
            branch: "feat/x".into(),
        }
    }

    /// What decides whether a working copy is asked instead: a branch GitHub
    /// does not have, or a token it refuses, will not change by waiting.
    #[test]
    fn what_a_person_must_fix_is_not_from_git_and_what_may_pass_is_unavailable() {
        assert!(matches!(
            unresolved(not_found()),
            Unresolved::NotFromGit(NotFromGit::BranchNotFound { .. })
        ));
        let denied = BranchHeadError::Denied {
            owner: "acme".into(),
            repo: "analytics".into(),
            status: 401,
        };
        assert!(matches!(
            unresolved(denied),
            Unresolved::NotFromGit(NotFromGit::Denied { status: 401, .. })
        ));
        for passing in [
            BranchHeadError::RateLimited { status: 429 },
            BranchHeadError::Unavailable("HTTP 502".into()),
        ] {
            assert!(matches!(unresolved(passing), Unresolved::Unavailable(_)));
        }
    }

    #[test]
    fn every_reason_has_its_own_code() {
        let reasons = [
            NotFromGit::NoRemote,
            NotFromGit::UnsupportedRemote("git@gitlab.com:a/b.git".into()),
            NotFromGit::NoConnection,
            NotFromGit::Denied {
                owner: "a".into(),
                repo: "b".into(),
                status: 403,
            },
            NotFromGit::BranchNotFound {
                owner: "a".into(),
                repo: "b".into(),
                branch: "c".into(),
            },
            NotFromGit::NothingCompiled,
        ];
        let codes: std::collections::BTreeSet<_> = reasons.iter().map(NotFromGit::code).collect();
        assert_eq!(codes.len(), reasons.len());
    }
}
