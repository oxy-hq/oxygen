//! Recording a remote-backed workspace's default branch.
//!
//! The periodic check (`check.rs`) and the push webhook act only on a workspace
//! whose `default_branch` is recorded: without it there is no branch to ask
//! GitHub about, and `state::seed` gives the workspace no check row. The Factory's
//! fetch sweep used to record it from the checkout; with the sweep gone, this
//! asks GitHub instead, one small request per workspace, at the start of a tick.
//!
//! `repo_subdir` is not derivable from GitHub (it is where in the repository the
//! workspace lives, which only the workspace knows), so it is left as it is: a
//! NULL subdirectory is read as the repository root.

use async_trait::async_trait;
use entity::workspaces;
use std::time::{Duration, Instant};

use oxy::github::{BranchHeadError, DefaultBranchError, GitHubClient, github_token_for_workspace};
use sea_orm::sea_query::{NullOrdering, Order};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};

use crate::server::compile_git::github_repository;

/// At most this many workspaces are asked about per tick, before the checks.
pub const MAX_PER_TICK: u64 = 5;

/// A workspace asked about and left unrecorded is not asked again for this long.
pub const RETRY_AFTER: chrono::Duration = chrono::Duration::hours(1);

/// Where a repository's default branch comes from: GitHub, or a fake in tests.
#[async_trait]
pub trait DefaultBranchSource: Send + Sync {
    /// Asked with at most `timeout`: what is left of the pass's budget.
    async fn default_branch(
        &self,
        owner: &str,
        repo: &str,
        timeout: Duration,
    ) -> Result<String, DefaultBranchError>;
}

#[async_trait]
impl DefaultBranchSource for GitHubClient {
    async fn default_branch(
        &self,
        owner: &str,
        repo: &str,
        timeout: Duration,
    ) -> Result<String, DefaultBranchError> {
        self.repository_default_branch(owner, repo, timeout).await
    }
}

/// What one workspace's lookup came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Learned {
    /// The branch to record.
    Branch(String),
    /// Nothing to record: no github.com remote, no connection, or GitHub
    /// answered for this repository without naming a branch (not found,
    /// denied, an unmapped status, a body without one). The workspace is asked
    /// again after [`RETRY_AFTER`]; the pass goes on to the next one.
    Nothing,
    /// The request never completed (or ran out of the pass's budget): GitHub
    /// is unwell, so the pass stops asking anyone this tick.
    Stop,
    /// GitHub rate-limited the token: the pass stops, and the caller pauses
    /// this process as the check loop does.
    RateLimited,
}

/// Ask `source` for `row`'s default branch, with at most `timeout`.
pub async fn learn(
    source: &dyn DefaultBranchSource,
    row: &workspaces::Model,
    timeout: Duration,
) -> Learned {
    let Ok((owner, repo)) = github_repository(row) else {
        return Learned::Nothing;
    };
    let asked = tokio::time::timeout(timeout, source.default_branch(&owner, &repo, timeout)).await;
    match asked {
        Ok(Ok(branch)) => Learned::Branch(branch),
        Ok(Err(DefaultBranchError::Request(BranchHeadError::RateLimited { .. }))) => {
            Learned::RateLimited
        }
        Ok(Err(DefaultBranchError::Request(BranchHeadError::Unavailable(error)))) => {
            tracing::warn!(
                target: "compile_reconcile", workspace_id = %row.id, %error,
                "GitHub did not answer a default-branch lookup; the pass stops for this tick"
            );
            Learned::Stop
        }
        Err(_) => Learned::Stop,
        Ok(Err(e)) => {
            tracing::info!(
                target: "compile_reconcile",
                workspace_id = %row.id,
                error = %e,
                "could not learn the default branch from GitHub"
            );
            Learned::Nothing
        }
    }
}

/// How one pass went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    pub recorded: usize,
    /// GitHub did not answer, or the budget ran out; the pass stopped there.
    pub stopped: bool,
    /// GitHub rate-limited a lookup; the caller pauses this process.
    pub rate_limited: bool,
}

/// Record the default branch of up to [`MAX_PER_TICK`] ready, github.com-backed
/// workspaces that have none, asking GitHub with each workspace's own token,
/// within `budget`.
pub(super) async fn record_missing(db: &DatabaseConnection, budget: Duration) -> Pass {
    record_missing_with(db, budget, |row| async move {
        let token = github_token_for_workspace(&row).await.ok().flatten()?;
        let client = GitHubClient::from_token(token).ok()?;
        Some(Box::new(client) as Box<dyn DefaultBranchSource>)
    })
    .await
}

/// [`record_missing`] with the source chosen per workspace by `source_for`;
/// `None` (no connection) counts as an unanswered attempt.
///
/// Every workspace asked about is stamped (`default_branch_attempted_at`),
/// the one that stopped the pass included: an unstamped row sorts first, so a
/// repository GitHub keeps failing on would otherwise be the first one asked
/// on every tick, by every replica, and nothing behind it would ever be.
pub async fn record_missing_with<F, Fut>(
    db: &DatabaseConnection,
    budget: Duration,
    source_for: F,
) -> Pass
where
    F: Fn(workspaces::Model) -> Fut,
    Fut: std::future::Future<Output = Option<Box<dyn DefaultBranchSource>>>,
{
    let started = Instant::now();
    let now = chrono::Utc::now().fixed_offset();
    let rows = match missing(db, now).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(target: "compile_reconcile", error = %e, "reading workspaces with no default branch failed");
            return Pass::default();
        }
    };
    let mut pass = Pass::default();
    for row in rows {
        // What is left of the budget, not a fresh one: one slow answer must not
        // stretch the pass past what the tick gave it.
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            // Not asked: nothing to hold against the rest.
            pass.stopped = true;
            break;
        }
        let asking = async {
            match source_for(row.clone()).await {
                Some(source) => learn(source.as_ref(), &row, remaining).await,
                None => Learned::Nothing,
            }
        };
        // The token lookup counts against the budget too.
        let learned = tokio::time::timeout(remaining, asking)
            .await
            .unwrap_or(Learned::Stop);
        match learned {
            Learned::Branch(branch) => {
                if store(db, row, Some(&branch), now).await {
                    pass.recorded += 1;
                }
            }
            Learned::Nothing => {
                store(db, row, None, now).await;
            }
            stop @ (Learned::Stop | Learned::RateLimited) => {
                pass.rate_limited = stop == Learned::RateLimited;
                pass.stopped = true;
                store(db, row, None, now).await;
                break;
            }
        }
    }
    pass
}

/// Ready workspaces with a github.com remote and no recorded default branch,
/// not asked about within [`RETRY_AFTER`], least recently asked first.
async fn missing(
    db: &DatabaseConnection,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Vec<workspaces::Model>, DbErr> {
    workspaces::Entity::find()
        .filter(workspaces::Column::GitRemoteUrl.contains("github.com"))
        .filter(workspaces::Column::Status.eq(workspaces::WorkspaceStatus::Ready))
        .filter(
            Condition::any()
                .add(workspaces::Column::DefaultBranch.is_null())
                .add(workspaces::Column::DefaultBranch.eq("")),
        )
        .filter(
            Condition::any()
                .add(workspaces::Column::DefaultBranchAttemptedAt.is_null())
                .add(workspaces::Column::DefaultBranchAttemptedAt.lt(now - RETRY_AFTER)),
        )
        .order_by_with_nulls(
            workspaces::Column::DefaultBranchAttemptedAt,
            Order::Asc,
            NullOrdering::First,
        )
        .order_by_asc(workspaces::Column::Id)
        .limit(MAX_PER_TICK)
        .all(db)
        .await
}

/// Write the branch (or, with none, the time of the unanswered attempt).
async fn store(
    db: &DatabaseConnection,
    row: workspaces::Model,
    branch: Option<&str>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> bool {
    let id = row.id;
    let mut active: workspaces::ActiveModel = row.into();
    active.default_branch_attempted_at = Set(Some(now));
    if let Some(branch) = branch {
        active.default_branch = Set(Some(branch.to_string()));
    }
    match active.update(db).await {
        Ok(_) => {
            if let Some(branch) = branch {
                tracing::info!(target: "compile_reconcile", workspace_id = %id, branch, "recorded the default branch from GitHub");
            }
            true
        }
        Err(e) => {
            tracing::warn!(target: "compile_reconcile", workspace_id = %id, error = %e, "recording the default branch failed");
            false
        }
    }
}

#[cfg(test)]
#[path = "default_branch_tests.rs"]
mod tests;
