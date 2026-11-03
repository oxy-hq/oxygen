//! The OLTP arm of the reset route: put a sandbox's own OLTP branch back to a
//! fresh copy of production, now or as of `at`
//! (`POST …/environments/{name}/reset {"planes": ["oltp"], "at": …}`).
//!
//! **Queued, never done in the request**: a cut copies a database. Under a lock
//! on the sandbox's row the route queues an `EnvBranchTask` — a `Reset` when
//! the sandbox has a branch row, a `Cut` when it has none (a cut that failed
//! before a row, or a drop that ran) — and writes `app_environments.fork_at`
//! to the new fork point in the same transaction, because the expiry pass reads
//! the sandbox's age from there: a reset restarts the clock, and a sandbox
//! must not be torn down for the age of the cut it just replaced.
//!
//! A sandbox on `schema` mode, and staging, have nothing to reset here
//! (`409 plane_not_resettable`, with a message that says what to do instead):
//! a schema sandbox is re-seeded by publishing to it again.

use chrono::Utc;
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::env_branches::{check_fork_at, find_for_app};
use sea_orm::{EntityTrait, QuerySelect, TransactionTrait};

use super::SandboxError;
use super::delete::refuse_unless_own;
use super::reset::{PlaneReset, ResetStatus, ResetTarget};
use crate::custom_apps_env_branch::planes::record_fork_at;
use crate::custom_apps_env_branch::queue::{BranchOp, EnvBranchTask, enqueue_in};
use crate::custom_apps_env_branch::wants_branch;

/// Why a sandbox on `schema` has no OLTP reset, and what to do instead.
pub(super) const SCHEMA_MODE_REFUSAL: &str = "the oltp plane of this sandbox runs on a schema of \
    the org's staging branch, which has no reset: publish to the sandbox again to re-seed its \
    schema, or use --plane oltp=branch for a copy of production's data that can be reset";

/// Why staging has no OLTP reset.
pub(super) const STAGING_REFUSAL: &str = "staging's oltp plane is the org's OLTP staging branch, \
    which is reset for the whole org, not per environment: an operator runs \
    `oxyc oltp reset --org <org> --branch staging` (POST \
    /api/admin/orgs/{org_id}/oltp/branches/staging/reset); only a sandbox on \
    --plane oltp=branch or oltp=empty has an OLTP reset";

/// Queue the reset of `target.environment`'s OLTP branch.
pub(super) async fn reset_oltp(target: &ResetTarget<'_>) -> Result<PlaneReset, SandboxError> {
    let (db, app) = (target.db, target.app);
    let not_resettable = || SandboxError::PlaneNotResettableHere(SCHEMA_MODE_REFUSAL.to_string());
    if !matches!(target.environment, AppEnvironment::Dev { .. }) {
        return Err(SandboxError::PlaneNotResettableHere(
            STAGING_REFUSAL.to_string(),
        ));
    }
    let now = Utc::now();
    if let Some(at) = target.at {
        check_fork_at(at, now).map_err(SandboxError::InvalidForkAt)?;
    }
    let name = target.environment.name();
    let txn = db.begin().await.map_err(|e| SandboxError::db("begin", e))?;
    let row = app_environments::Entity::find_by_id((app.id, name.clone()))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| SandboxError::db("lock the environment", e))?
        .ok_or_else(|| SandboxError::NotFound(name.clone()))?;
    refuse_unless_own(&row, Some(target.actor))?;
    if row.deleting_at.is_some() {
        return Err(SandboxError::Deleting(name));
    }
    wants_branch(&row).ok_or_else(not_resettable)?;
    let has_branch = find_for_app(&txn, app.id, &row.name)
        .await
        .map_err(|e| SandboxError::db("read the OLTP branch", e))?
        .is_some();
    let op = match has_branch {
        true => BranchOp::Reset { at: target.at },
        false => BranchOp::Cut { at: target.at },
    };
    let task = EnvBranchTask {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        workspace_id: app.project_id,
        environment: row.name.clone(),
        op,
    };
    let run_id = enqueue_in(&txn, &task)
        .await
        .map_err(|e| SandboxError::db("queue the OLTP branch reset", e))?;
    record_fork_at(&txn, app.id, &row.name, target.at.unwrap_or(now))
        .await
        .map_err(|e| SandboxError::db("record the fork point", e))?;
    txn.commit()
        .await
        .map_err(|e| SandboxError::db("commit", e))?;
    Ok(PlaneReset {
        plane: "oltp",
        status: ResetStatus::Queued,
        run_id: Some(run_id),
    })
}
