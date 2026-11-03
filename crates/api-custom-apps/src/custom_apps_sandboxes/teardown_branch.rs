//! The teardown step that removes a sandbox's own OLTP branch
//! (`custom_apps_env_branch`), run under the sandbox's lock between dropping
//! its schema on the shared staging branch and removing its row.
//!
//! The provisioner deletes the provider's branch, then the ledger rows, then
//! the `oltp_env_branches` row (and with it the sealed roles) — and a branch
//! the provider made under the sandbox's name that no row recorded. Any
//! failure — including a row that names production's own database, which the
//! provisioner refuses — keeps the sandbox row `deleting`, so a retry finishes
//! the job.

use oxy_oltp::env_branches::{EnvBranchKey, find_for_app};
use sea_orm::DatabaseConnection;

use super::teardown::SandboxTeardownTask;
use crate::custom_apps_env_branch::provisioner;

/// Delete the sandbox's OLTP branch, if the sandbox has one. `Ok(what was
/// done)`, or `Err(why not)`.
///
/// A sandbox with no `oltp_env_branches` row has nothing to delete, and no
/// provisioner is built for it: a deployment with the OLTP integration off
/// tears sandboxes down as before.
pub(super) async fn delete_env_branch(
    db: &DatabaseConnection,
    task: &SandboxTeardownTask,
) -> Result<&'static str, String> {
    let recorded = find_for_app(db, task.app_id, &task.environment)
        .await
        .map_err(|e| e.to_string())?;
    if recorded.is_none() {
        return Ok("no OLTP branch");
    }
    let key = EnvBranchKey {
        org_id: task.org_id,
        app_id: task.app_id,
        env_name: task.environment.clone(),
    };
    let gone = provisioner(db)
        .await?
        .delete_env_branch(&key)
        .await
        .map_err(|e| e.to_string())?;
    Ok(if gone {
        "its OLTP branch deleted"
    } else {
        "its OLTP branch was already gone"
    })
}
