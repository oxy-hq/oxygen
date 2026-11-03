//! The `custom_app_env_oltp_branch` task: cut, reset or migrate one sandbox's
//! own OLTP branch, run on the worker fleet.
//!
//! Shaped as the sandbox OLTP schema task is
//! (`custom_apps_sandboxes::oltp_task`): its own kind, so a worker that does
//! not know it fails the task instead of running it as another; the sandbox's
//! lock ([`SandboxLock`], the one a teardown holds) held for the whole run; the
//! row read under it, so a sandbox that is gone, or being torn down, gets
//! nothing done.
//!
//! - **`Cut` / `Reset`** call the provisioner, which leaves the branch
//!   `migrating` (and `failed`, with the reason, when it cannot). The task then
//!   applies the build the sandbox serves — its OLTP files, under the branch's
//!   own ledger target — and says `ready`. A failed apply says `failed` too:
//!   a branch missing its build's tables must not serve. **The build is read
//!   again after the provider call**, under the same lock: a cut takes
//!   minutes, a publish during it moves the sandbox's build, and its own
//!   `Migrate` cannot get the lock until this run is over. The row is read
//!   once more after each apply, so a build that landed during it is applied
//!   before `ready` too.
//! - **`Reset` re-cuts in the mode the sandbox's planes ask for now**, read
//!   from the row under the lock — not a mode the queue entry carried — so a
//!   switch between `branch` and `empty` ends in a branch `home_for` serves,
//!   and a reset queued before a switch back to `schema` does nothing. A reset
//!   of a sandbox with no branch (a drop ran first) cuts one. Both write the
//!   branch's fork point to the sandbox's `fork_at` (`planes::record_fork_at`):
//!   the expiry pass reads it there.
//! - **`Drop`** deletes the branch and its row, unless the planes ask for a
//!   branch again.
//! - **`Migrate { build_pk }`** applies one build's files to a branch that is
//!   already cut, when the sandbox still serves that build. Its failure is the
//!   run's, and the branch's status stays as it was: the previous build's
//!   tables are still there, as for staging's apply.
//!
//! The queue side — [`EnvBranchTask`], [`BranchOp`], [`enqueue`] and the
//! executor — is `queue`; its items are re-exported here.

use std::future::Future;

use chrono::{DateTime, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::WriterRef;
use oxy_oltp::entity::env_branches::EnvBranchStatus;
use oxy_oltp::env_branches::{find_for_app, mark_env_branch};
use oxy_oltp::resolver::{EnvBranchAccept, resolve_env_branch_writer_for_org};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::planes::record_fork_at;
pub use super::queue::{BranchOp, ENV_OLTP_BRANCH_KIND, EnvBranchExecutor, EnvBranchTask, enqueue};
use super::{provisioner, wants_branch};
use crate::custom_apps_env_resolve::sandbox_row;
use crate::custom_apps_migrations::{
    BRANCH_APPLY_TIMEOUT, MigrationError, apply_to_resolved_branch, declared_in_build,
};
use crate::custom_apps_nonproduction::staging_task_executor::{
    BUSY_RETRY_DELAYS, bounded, retry_busy,
};
use crate::custom_apps_sandboxes::lock::SandboxLock;

/// The most builds one cut or reset applies before it says `ready`: the one
/// the sandbox served after the provider call, plus those published while
/// applying. A sandbox published to faster than that is migrated by the
/// publish's own task once this run's lock is free.
const SETTLE_ROUNDS: usize = 3;

/// Do `task`'s op under the sandbox's lock: `Ok(what was done)`, or `Err(why
/// not)`. Waits [`BUSY_RETRY_DELAYS`] for the lock.
pub async fn run(db: &DatabaseConnection, task: &EnvBranchTask) -> Result<String, String> {
    run_with(db, task, async {}).await
}

/// [`run`], awaiting `between` after a cut's or reset's provider call and
/// before its build is read and applied — the window a publish lands in. A
/// seam for the test that lands one there; [`run`] passes nothing.
pub async fn run_with<F: Future<Output = ()>>(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    between: F,
) -> Result<String, String> {
    let environment = task.sandbox()?;
    let name = &task.environment;
    let lock = SandboxLock::acquire(db, task.app_id, name, &BUSY_RETRY_DELAYS)
        .await
        .map_err(|e| format!("sandbox {name}'s lock could not be taken: {e}"))?;
    let Some(lock) = lock else {
        return Err(format!(
            "sandbox {name} is locked by a teardown or another task of it; nothing was done to \
             its OLTP branch"
        ));
    };
    let outcome = under_lock(db, task, &environment, between).await;
    lock.release().await;
    outcome
}

async fn under_lock<F: Future<Output = ()>>(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    environment: &AppEnvironment,
    between: F,
) -> Result<String, String> {
    let name = &task.environment;
    let row = sandbox_row(db, task.app_id, environment)
        .await
        .map_err(|e| format!("sandbox {name}'s row could not be read: {e}"))?;
    let Some(row) = row else {
        return Ok(format!(
            "sandbox {name} is gone or being deleted; nothing was done to its OLTP branch"
        ));
    };
    match &task.op {
        BranchOp::Cut { at } => cut(db, task, &row, *at, between).await,
        BranchOp::Reset { at } => reset(db, task, &row, *at, between).await,
        BranchOp::Migrate { build_pk } => migrate(db, task, &row, *build_pk).await,
        BranchOp::Drop => drop_branch(db, task, &row).await,
    }
}

async fn cut<F: Future<Output = ()>>(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    row: &app_environments::Model,
    at: Option<DateTime<Utc>>,
    between: F,
) -> Result<String, String> {
    let Some(mode) = wants_branch(row) else {
        return Ok(format!(
            "sandbox {}'s planes ask for no OLTP branch; nothing was cut",
            task.environment
        ));
    };
    let provisioner = provisioner(db).await?;
    let branch = provisioner
        .cut_env_branch(&task.key(), mode, at)
        .await
        .map_err(|e| {
            format!(
                "sandbox {}'s OLTP branch was not cut: {e}",
                task.environment
            )
        })?;
    between.await;
    finish(db, task, &branch, "cut").await
}

async fn reset<F: Future<Output = ()>>(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    row: &app_environments::Model,
    at: Option<DateTime<Utc>>,
    between: F,
) -> Result<String, String> {
    let name = &task.environment;
    let Some(mode) = wants_branch(row) else {
        return Ok(format!(
            "sandbox {name}'s planes ask for no OLTP branch; nothing was reset"
        ));
    };
    let has_branch = find_for_app(db, task.app_id, name)
        .await
        .map_err(|e| format!("sandbox {name}'s OLTP branch row could not be read: {e}"))?
        .is_some();
    if !has_branch {
        return cut(db, task, row, at, between).await;
    }
    let provisioner = provisioner(db).await?;
    let branch = provisioner
        .reset_env_branch_as(&task.key(), at, Some(mode))
        .await
        .map_err(|e| format!("sandbox {name}'s OLTP branch was not reset: {e}"))?;
    between.await;
    finish(db, task, &branch, "reset").await
}

async fn drop_branch(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    row: &app_environments::Model,
) -> Result<String, String> {
    let name = &task.environment;
    if wants_branch(row).is_some() {
        return Ok(format!(
            "sandbox {name}'s planes ask for an OLTP branch again; the branch was kept"
        ));
    }
    let provisioner = provisioner(db).await?;
    let deleted = provisioner
        .delete_env_branch(&task.key())
        .await
        .map_err(|e| format!("sandbox {name}'s OLTP branch was not deleted: {e}"))?;
    Ok(match deleted {
        true => format!("sandbox {name}'s OLTP branch was deleted"),
        false => format!("sandbox {name} had no OLTP branch to delete"),
    })
}

/// After a cut or reset: apply the build the sandbox serves **now** (not the
/// one the run started with) and record the branch's fork point on the
/// sandbox, then `ready` — or `failed`, with the reason, because a branch
/// without its build's tables, or whose fork point is not recorded, must not
/// serve.
async fn finish(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    branch: &oxy_oltp::entity::env_branches::Model,
    did: &str,
) -> Result<String, String> {
    let row_id = branch.id;
    let name = &task.environment;
    // The branch is `ready` only when its build is applied **and** the
    // sandbox's fork point says when it was cut: the expiry pass reads it
    // there, so a branch whose fork point could not be written must not
    // serve under the old one.
    let settled = match apply_served(db, task).await {
        Ok(applied) => record_fork_at(db, task.app_id, name, branch.fork_at.to_utc())
            .await
            .map(|()| applied)
            .map_err(|e| format!("its fork point was not recorded on the sandbox: {e}")),
        Err(why) => Err(format!("was not migrated: {why}")),
    };
    match settled {
        Ok(applied) => {
            mark_env_branch(db, row_id, EnvBranchStatus::Ready, None)
                .await
                .map_err(|e| {
                    format!("sandbox {name}'s OLTP branch could not be marked ready: {e}")
                })?;
            Ok(format!("sandbox {name}'s OLTP branch {did}; {applied}"))
        }
        Err(why) => {
            let message = format!("sandbox {name}'s OLTP branch {did} but {why}");
            let failed = Some(message.clone());
            if let Err(e) = mark_env_branch(db, row_id, EnvBranchStatus::Failed, failed).await {
                tracing::warn!(app_id = %task.app_id, environment = %name,
                    "could not mark the environment branch failed: {e}");
            }
            Err(message)
        }
    }
}

/// Apply the build the sandbox's row names, read now, under the lock; then
/// read it again, and apply that too if a publish moved it meanwhile — up to
/// [`SETTLE_ROUNDS`] builds. `Ok(what was applied)`.
async fn apply_served(db: &DatabaseConnection, task: &EnvBranchTask) -> Result<String, String> {
    let environment = task.sandbox()?;
    let mut applied: Vec<(Uuid, String)> = Vec::new();
    for _ in 0..SETTLE_ROUNDS {
        let row = sandbox_row(db, task.app_id, &environment)
            .await
            .map_err(|e| format!("the sandbox's row could not be read: {e}"))?;
        let serving = row.and_then(|row| row.build_id);
        let Some(build_pk) = serving.filter(|pk| applied.last().is_none_or(|(last, _)| last != pk))
        else {
            break;
        };
        applied.push((build_pk, apply_build(db, task, build_pk).await?));
    }
    if applied.is_empty() {
        return Ok("no build to migrate".to_string());
    }
    let said: Vec<String> = applied.into_iter().map(|(_, said)| said).collect();
    Ok(said.join("; "))
}

async fn migrate(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    row: &app_environments::Model,
    build_pk: Uuid,
) -> Result<String, String> {
    let name = &task.environment;
    if row.build_id != Some(build_pk) {
        return Ok(format!(
            "sandbox {name} no longer serves this build; nothing was applied (the build it \
             serves queued its own task)"
        ));
    }
    apply_build(db, task, build_pk)
        .await
        .map_err(|why| format!("sandbox {name}'s OLTP branch was not migrated: {why}"))
}

/// Apply build `build_pk`'s OLTP files to the environment's branch, which may
/// be `migrating`: `Ok(what was applied)`.
async fn apply_build(
    db: &DatabaseConnection,
    task: &EnvBranchTask,
    build_pk: Uuid,
) -> Result<String, String> {
    let build = entity::app_builds::Entity::find_by_id(build_pk)
        .one(db)
        .await
        .map_err(|e| format!("the build could not be read: {e}"))?
        .ok_or_else(|| "the build is gone".to_string())?;
    let declared = declared_in_build(&build, &task.app_slug)
        .await
        .map_err(|e| e.to_string())?
        .oltp;
    if declared.is_empty() {
        return Ok("the build declares no OLTP migrations".to_string());
    }
    let writer = oxy_oltp::schema::app_writer_name(&task.app_slug)
        .and_then(|name| WriterRef::app(name).ok())
        .ok_or_else(|| format!("the app's slug '{}' backs no OLTP schema", task.app_slug))?;
    let branch = resolve_env_branch_writer_for_org(
        db,
        task.org_id,
        task.app_id,
        &task.environment,
        &writer,
        EnvBranchAccept::ReadyOrMigrating,
    )
    .await
    .map_err(|e| format!("the branch is not usable: {e}"))?
    .ok_or_else(|| "the environment has no OLTP branch".to_string())?;
    let attempt = || async {
        let applying = apply_to_resolved_branch(db, task.app_id, build_pk, &declared, &branch);
        bounded(BRANCH_APPLY_TIMEOUT, applying)
            .await
            .unwrap_or_else(|| {
                Err(MigrationError::Infra {
                    filename: String::new(),
                    message: format!(
                        "the step did not finish within {}s and was stopped",
                        BRANCH_APPLY_TIMEOUT.as_secs()
                    ),
                })
            })
    };
    let applied = retry_busy(&BUSY_RETRY_DELAYS, attempt)
        .await
        .map_err(|e| e.to_string())?;
    if !applied.deferred.is_empty() {
        return Err(format!(
            "{} file(s) were left unapplied",
            applied.deferred.len()
        ));
    }
    Ok(applied.summary())
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
