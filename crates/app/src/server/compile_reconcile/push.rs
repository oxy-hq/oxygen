//! A GitHub `push` delivery: which workspaces it concerns, and a commit
//! compile for each.
//!
//! The receiver (`api::webhooks::github`) verifies the signature and hands the
//! parsed payload here. Everything after that is the same path the periodic
//! check takes: the head is compared with what the workspace serves
//! ([`super::check::decide`]) and a compile is handed to the deduped enqueue,
//! so a delivery is never queued behind another compile, a duplicate delivery
//! queues nothing while the first is in flight or after it is ready, and a
//! commit that keeps failing backs off.
//!
//! **Same gate as the periodic check** ([`super::enabled`]). With it off a
//! delivery is read and dropped.
//!
//! What a push is matched on, in order:
//!
//! * a **branch**: `refs/heads/<name>`. A tag, a deletion, or a push with no
//!   usable `after` commit is ignored;
//! * the **repository**: the payload's `owner/name` against the workspace's
//!   github.com remote, ignoring case, as GitHub does;
//! * the **recorded default branch** (`workspaces.default_branch`). A
//!   workspace with none recorded cannot be matched and is left to the
//!   periodic check, which skips it too;
//! * the **subdirectory** (`workspaces.repo_subdir`), only to *narrow*: a
//!   workspace that lives below `analytics/` is skipped when the push lists
//!   its changed files and none is under `analytics/`. NULL means "the
//!   repository root" and also "nobody has recorded it", so it is never
//!   skipped on a path, and neither is a push whose file lists may be
//!   incomplete (see [`PushEvent::changed_paths`]).

use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::check::{Decision, compiled_state, decide};
use crate::server::api::middlewares::workspace_context::{
    CompileEnqueue, enqueue_compile_deduped_as,
};
use crate::server::compile_git::{commit_sha, github_slug_of};

const BRANCH_REF_PREFIX: &str = "refs/heads/";

/// A payload's `commits` is capped (GitHub's documentation has said both 20 and
/// 2,048), and a push at the cap has been cut off. Below the smaller figure the
/// list is taken as complete; at or above it, it is not.
const COMPLETE_COMMIT_LIST_BELOW: usize = 20;

/// Changed paths past which a payload is assumed to be truncated rather than
/// complete. A push that touches this much is not a case for narrowing.
const COMPLETE_PATH_LIST_BELOW: usize = 3_000;

/// The part of GitHub's `push` payload this reads.
#[derive(Debug, Clone, Deserialize)]
pub struct PushEvent {
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub deleted: bool,
    /// The commit the ref points at after the push. All zeros on a deletion.
    #[serde(default)]
    pub after: String,
    pub repository: PushRepository,
    #[serde(default)]
    pub commits: Vec<PushCommit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PushRepository {
    /// `owner/name`.
    pub full_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PushCommit {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub modified: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
}

impl PushEvent {
    /// The branch pushed, or `None` for a tag or any other ref.
    pub fn branch(&self) -> Option<&str> {
        self.git_ref
            .strip_prefix(BRANCH_REF_PREFIX)
            .filter(|b| !b.is_empty())
    }

    /// The commit the branch now points at, or `None` for a deletion and for
    /// anything that is not a full commit id.
    pub fn head(&self) -> Option<String> {
        if self.deleted || self.after.bytes().all(|b| b == b'0') {
            return None;
        }
        commit_sha(Some(&self.after)).ok()
    }

    /// Whether the payload names `owner/repo`. GitHub treats both as case
    /// insensitive.
    pub fn is_for(&self, owner: &str, repo: &str) -> bool {
        self.repository
            .full_name
            .split_once('/')
            .is_some_and(|(o, r)| o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo))
    }

    /// Every path the push changed, or `None` when the payload cannot be
    /// trusted to list them all: no commits (a branch created at an existing
    /// commit, or a force push to one), or a list long enough that GitHub
    /// would have cut it off.
    pub fn changed_paths(&self) -> Option<impl Iterator<Item = &str>> {
        if self.commits.is_empty() || self.commits.len() >= COMPLETE_COMMIT_LIST_BELOW {
            return None;
        }
        let total: usize = self
            .commits
            .iter()
            .map(|c| c.added.len() + c.modified.len() + c.removed.len())
            .sum();
        if total >= COMPLETE_PATH_LIST_BELOW {
            return None;
        }
        Some(self.commits.iter().flat_map(|c| {
            c.added
                .iter()
                .chain(&c.modified)
                .chain(&c.removed)
                .map(String::as_str)
        }))
    }

    /// Whether the push may have changed files under `subdir`. `None` is the
    /// repository root, or a workspace nobody has recorded a subdirectory
    /// for; either way every path is its path.
    pub fn touches(&self, subdir: Option<&str>) -> bool {
        let Some(subdir) = subdir
            .map(|s| s.trim_matches('/'))
            .filter(|s| !s.is_empty())
        else {
            return true;
        };
        match self.changed_paths() {
            None => true,
            Some(mut paths) => paths.any(|p| {
                p.strip_prefix(subdir)
                    .is_some_and(|rest| rest.starts_with('/'))
            }),
        }
    }
}

/// Whether `push` concerns a workspace with these recorded facts. The row's
/// status is checked by the query, not here.
pub(super) fn concerns(
    push: &PushEvent,
    remote: &str,
    default_branch: Option<&str>,
    repo_subdir: Option<&str>,
) -> bool {
    let Some(branch) = push.branch() else {
        return false;
    };
    let Some((owner, repo)) = github_slug_of(remote) else {
        return false;
    };
    default_branch == Some(branch) && push.is_for(&owner, &repo) && push.touches(repo_subdir)
}

/// Why a delivery did nothing, or what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handled {
    /// Nothing was looked at.
    Ignored(&'static str),
    /// `matched` workspaces were on this repository and branch; for
    /// `queued` of them the head was neither served nor already compiled, so
    /// a compile went to the deduped enqueue (which may still decline it).
    Matched { matched: usize, queued: usize },
}

impl Handled {
    /// One line for the delivery's response body, which GitHub shows under
    /// the app's "Recent deliveries".
    pub fn describe(&self) -> String {
        match self {
            Self::Ignored(why) => format!("ignored: {why}"),
            Self::Matched { matched, queued } => {
                format!("{matched} workspace(s) match; {queued} handed to the compile queue")
            }
        }
    }
}

/// Act on one verified `push` delivery.
pub async fn handle(db: &DatabaseConnection, push: &PushEvent) -> Result<Handled, DbErr> {
    if !super::enabled() {
        return Ok(Handled::Ignored(
            "push-triggered compiles are switched off on this deployment",
        ));
    }
    let Some(branch) = push.branch() else {
        return Ok(Handled::Ignored("not a branch"));
    };
    let Some(head) = push.head() else {
        return Ok(Handled::Ignored(
            "the branch was deleted, or names no commit",
        ));
    };

    let rows = candidates(db, push, branch).await?;
    let mut matched = 0;
    let mut queued = 0;
    let mut failed: Option<DbErr> = None;
    for row in rows {
        let remote = row.git_remote_url.as_deref().unwrap_or_default();
        let subdir = row.repo_subdir.as_deref();
        if !concerns(push, remote, row.default_branch.as_deref(), subdir) {
            continue;
        }
        matched += 1;
        match compiled_state(db, &row, &head).await {
            Ok((promoted, ready)) => {
                if decide(&head, promoted.as_deref(), ready) == Decision::Compile {
                    queue(db, row.id, &head, branch).await;
                    queued += 1;
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "compile_reconcile", workspace_id = %row.id, error = %e,
                    "a push could not be compared with what the workspace serves"
                );
                failed.get_or_insert(e);
            }
        }
    }
    // One workspace's failure must not hide the others' compiles, but the
    // delivery still reports it, so it shows as failed in GitHub.
    match failed {
        Some(e) => Err(e),
        None => Ok(Handled::Matched { matched, queued }),
    }
}

/// Ready workspaces on `branch` whose remote mentions the pushed repository.
/// A coarse pre-filter in SQL; [`concerns`] decides.
async fn candidates(
    db: &DatabaseConnection,
    push: &PushEvent,
    branch: &str,
) -> Result<Vec<workspaces::Model>, DbErr> {
    // `strpos`, not `LIKE`: a repository name may contain `_`.
    let mentions = Expr::cust_with_values(
        "strpos(lower(git_remote_url), $1) > 0",
        [push.repository.full_name.to_ascii_lowercase()],
    );
    workspaces::Entity::find()
        .filter(workspaces::Column::Status.eq(WorkspaceStatus::Ready))
        .filter(workspaces::Column::GitRemoteUrl.is_not_null())
        .filter(workspaces::Column::DefaultBranch.eq(branch))
        .filter(mentions)
        .all(db)
        .await
}

async fn queue(db: &DatabaseConnection, workspace_id: Uuid, head: &str, branch: &str) {
    enqueue_compile_deduped_as(
        db,
        workspace_id,
        Some(head.to_string()),
        Some(branch.to_string()),
        "github push",
        // A pushed commit is new source, so a failure keeps the short retry
        // window rather than the exponential one a repeated head gets.
        CompileEnqueue {
            from_git: true,
            new_content: true,
        },
    )
    .await;
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
