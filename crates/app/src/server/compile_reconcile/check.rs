//! One workspace's check: where is its default branch on GitHub, and is that
//! what it serves?

use std::time::Duration;

use entity::{revisions, workspaces};
use oxy::github::{BranchHeadError, GitHubClient, github_token_for_workspace};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::{
    CompileEnqueue, enqueue_compile_deduped_as,
};
use crate::server::compile_git::github_repository;
use crate::server::workspace_repo_facts::stored_default_branch;

/// How long a workspace GitHub will not answer for is left alone: its
/// repository or branch is gone, or the token cannot see it. An hour is short
/// enough that reconnecting GitHub takes effect the same morning and long
/// enough that the warning is not a drumbeat.
const UNREACHABLE_BACKOFF: Duration = Duration::from_secs(60 * 60);

/// How long a workspace with no GitHub connection, or none of the facts a
/// check needs, is left alone. Nothing this loop does can fix either; a
/// person has to, and six hours bounds the warnings to four a day.
const UNCHECKABLE_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);

/// What a check found, and so what the loop does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The promoted revision was compiled from the branch head.
    UpToDate { head: String },
    /// The head is compiled and ready, just not the revision being served: a
    /// newer compile from the working copy is. Compiling it again would
    /// change nothing.
    AlreadyCompiled { head: String },
    /// A commit compile of the head was handed to the deduped enqueue.
    Enqueued { head: String },
    /// The row lacks something a check needs. Carries which.
    Uncheckable(&'static str),
    /// No GitHub connection is linked to the workspace.
    NoToken,
    /// GitHub has no such repository or branch for this token.
    NotFound,
    /// GitHub refused the token.
    Denied,
    /// GitHub is rate-limiting. Stops the whole loop, not just this workspace.
    RateLimited,
    /// GitHub, or minting a token, failed in a way that may pass.
    Unavailable(String),
}

impl Outcome {
    /// The value written to `workspace_compile_checks.last_outcome`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::UpToDate { .. } => "up_to_date",
            Self::AlreadyCompiled { .. } => "already_compiled",
            Self::Enqueued { .. } => "enqueued",
            Self::Uncheckable(_) => "uncheckable",
            Self::NoToken => "no_token",
            Self::NotFound => "not_found",
            Self::Denied => "denied",
            Self::RateLimited => "rate_limited",
            Self::Unavailable(_) => "unavailable",
        }
    }

    pub fn head(&self) -> Option<&str> {
        match self {
            Self::UpToDate { head } | Self::AlreadyCompiled { head } | Self::Enqueued { head } => {
                Some(head)
            }
            _ => None,
        }
    }

    /// How much longer than the normal cadence to leave this workspace alone.
    pub fn backoff(&self) -> Duration {
        match self {
            Self::NotFound | Self::Denied => UNREACHABLE_BACKOFF,
            Self::NoToken | Self::Uncheckable(_) => UNCHECKABLE_BACKOFF,
            _ => Duration::ZERO,
        }
    }
}

/// What to do about a branch head, given what the workspace already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    Nothing,
    AlreadyCompiled,
    Compile,
}

/// `promoted` is the SHA of the revision being served; `ready_for_head` is
/// whether a ready main revision of `head` exists at all.
pub(super) fn decide(head: &str, promoted: Option<&str>, ready_for_head: bool) -> Decision {
    if promoted == Some(head) {
        Decision::Nothing
    } else if ready_for_head {
        Decision::AlreadyCompiled
    } else {
        Decision::Compile
    }
}

/// Check one workspace. `last_head` is the head its previous check saw.
pub(super) async fn check(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    last_head: Option<&str>,
) -> Outcome {
    let row = match workspaces::Entity::find_by_id(workspace_id).one(db).await {
        Ok(Some(row)) => row,
        Ok(None) => return Outcome::Uncheckable("the workspace is gone"),
        Err(e) => return Outcome::Unavailable(e.to_string()),
    };
    let Ok((owner, repo)) = github_repository(&row) else {
        return Outcome::Uncheckable("it has no github.com remote");
    };
    let Some(branch) = stored_default_branch(&row) else {
        return Outcome::Uncheckable("its default branch is not recorded");
    };
    let token = match github_token_for_workspace(&row).await {
        Ok(Some(token)) => token,
        Ok(None) => return Outcome::NoToken,
        Err(e) => return Outcome::Unavailable(e.to_string()),
    };
    let head = match branch_head(token, &owner, &repo, &branch).await {
        Ok(head) => head,
        Err(outcome) => return outcome,
    };

    let (promoted, ready_for_head) = match compiled_state(db, &row, &head).await {
        Ok(state) => state,
        Err(e) => return Outcome::Unavailable(e.to_string()),
    };
    match decide(&head, promoted.as_deref(), ready_for_head) {
        Decision::Nothing => Outcome::UpToDate { head },
        Decision::AlreadyCompiled => Outcome::AlreadyCompiled { head },
        Decision::Compile => {
            // A head this loop has not seen before is new content, and gets
            // the short retry window; the same head again is a commit that
            // has already been tried, and backs off the way a self-heal does.
            let how = CompileEnqueue {
                from_git: true,
                new_content: last_head != Some(head.as_str()),
            };
            enqueue_compile_deduped_as(
                db,
                workspace_id,
                Some(head.clone()),
                Some(branch),
                "branch head moved",
                how,
            )
            .await;
            Outcome::Enqueued { head }
        }
    }
}

async fn branch_head(
    token: String,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<String, Outcome> {
    let client =
        GitHubClient::from_token(token).map_err(|e| Outcome::Unavailable(e.to_string()))?;
    client
        .branch_head(owner, repo, branch)
        .await
        .map_err(|e| match e {
            BranchHeadError::NotFound { .. } => Outcome::NotFound,
            BranchHeadError::Denied { .. } => Outcome::Denied,
            BranchHeadError::RateLimited { .. } => Outcome::RateLimited,
            BranchHeadError::Unavailable(message) => Outcome::Unavailable(message),
        })
}

/// `(SHA being served, whether a ready main revision of head exists)`.
async fn compiled_state(
    db: &DatabaseConnection,
    row: &workspaces::Model,
    head: &str,
) -> Result<(Option<String>, bool), sea_orm::DbErr> {
    let promoted = match row.current_revision_id {
        Some(id) => revisions::Entity::find_by_id(id)
            .one(db)
            .await?
            .map(|r| r.git_sha),
        None => None,
    };
    let ready_for_head = revisions::Entity::find()
        .filter(revisions::Column::WorkspaceId.eq(row.id))
        .filter(revisions::Column::GitSha.eq(head))
        .filter(revisions::Column::Kind.eq("main"))
        .filter(revisions::Column::Status.eq("ready"))
        .one(db)
        .await?
        .is_some();
    Ok((promoted, ready_for_head))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_head_that_is_being_served_needs_nothing() {
        assert_eq!(decide(HEAD, Some(HEAD), true), Decision::Nothing);
    }

    /// The disk-snapshot case: a `local-…` revision is being served and the
    /// head has never been compiled.
    #[test]
    fn a_head_that_was_never_compiled_is_compiled() {
        assert_eq!(decide(HEAD, Some("local-6f1c"), false), Decision::Compile);
        assert_eq!(decide(HEAD, None, false), Decision::Compile);
    }

    /// The node with the working copy compiled the head and then something
    /// newer; compiling the head again would only fetch it to change nothing.
    #[test]
    fn a_head_that_is_compiled_but_not_served_is_left_alone() {
        assert_eq!(
            decide(HEAD, Some("89abcdef0123456789abcdef0123456789abcdef"), true),
            Decision::AlreadyCompiled
        );
    }

    #[test]
    fn only_what_a_person_must_fix_is_left_alone_for_longer() {
        let head = || HEAD.to_string();
        for prompt in [
            Outcome::UpToDate { head: head() },
            Outcome::Enqueued { head: head() },
            Outcome::AlreadyCompiled { head: head() },
            Outcome::RateLimited,
            Outcome::Unavailable("HTTP 502".into()),
        ] {
            assert_eq!(prompt.backoff(), Duration::ZERO, "{prompt:?}");
        }
        assert_eq!(Outcome::NotFound.backoff(), UNREACHABLE_BACKOFF);
        assert_eq!(Outcome::Denied.backoff(), UNREACHABLE_BACKOFF);
        assert_eq!(Outcome::NoToken.backoff(), UNCHECKABLE_BACKOFF);
    }
}
