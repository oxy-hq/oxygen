//! The maintenance pass that deletes OLTP environment branches no sandbox
//! holds any more ([`super::maintenance`]'s fifth step).
//!
//! `app_environments` cascades from its app and its owner, and
//! `oltp_env_branches` deliberately has no foreign key to it, so deleting an
//! app or a user removes its sandboxes without a teardown and leaves their
//! branches: billed at the provider (an `empty` branch carries no provider
//! expiry) and counted against `OXY_APP_ENV_OLTP_BRANCHES_MAX` forever, with
//! no sandbox left to delete. This pass finds them
//! ([`oxy_oltp::env_branches::orphaned`]) and deletes each — provider branch,
//! roles, ledger and row — through the provisioner's own delete, under the
//! branch's lock, only if still no sandbox holds it once the lock is taken.
//! The provisioner refuses a row that names production or the org's staging
//! branch, as it does for a teardown.
//!
//! Unlike the other passes this one does the deletes inline rather than
//! queueing a task: there is no sandbox row left for a teardown to run under.
//! A pass takes at most [`super::maintenance::SANDBOXES_PER_PASS`]; the next
//! pass continues.

use oxy_oltp::env_branches::orphaned;
use sea_orm::DatabaseConnection;

use super::maintenance::SANDBOXES_PER_PASS;
use crate::custom_apps_env_branch::provisioner;

/// One pass: how many orphaned branches were deleted. Never fails the
/// sweep: the passes before it have done their work, so a failed read or a
/// branch that cannot be deleted is logged and left for the next pass. With
/// no orphan no provisioner is built, so a deployment with the OLTP
/// integration off sweeps as before.
pub async fn sweep(db: &DatabaseConnection) -> usize {
    let keys = match orphaned(db, SANDBOXES_PER_PASS as u64).await {
        Ok(keys) if keys.is_empty() => return 0,
        Ok(keys) => keys,
        Err(e) => {
            tracing::warn!(error = %e, "sandbox_maintenance: could not read orphaned OLTP branches");
            return 0;
        }
    };
    let provisioner = match provisioner(db).await {
        Ok(p) => p,
        Err(why) => {
            tracing::warn!(orphans = keys.len(), %why,
                "sandbox_maintenance: orphaned OLTP branches left for a later pass");
            return 0;
        }
    };
    let mut deleted = 0;
    for key in keys {
        match provisioner.delete_orphaned_env_branch(&key).await {
            Ok(true) => {
                tracing::info!(org_id = %key.org_id, app_id = %key.app_id, environment = %key.env_name,
                    "sandbox_maintenance: deleted an OLTP branch no sandbox holds");
                deleted += 1;
            }
            // A sandbox holds it again, or it went, since the read.
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(org_id = %key.org_id, app_id = %key.app_id, environment = %key.env_name,
                    error = %e, "sandbox_maintenance: could not delete an orphaned OLTP branch");
            }
        }
    }
    deleted
}
