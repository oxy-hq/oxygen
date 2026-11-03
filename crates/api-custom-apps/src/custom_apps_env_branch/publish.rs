//! What a publish to a branch-mode sandbox queues: the migration of its own
//! OLTP branch, never a schema on the org's staging branch.
//!
//! The sandbox's row says which. A sandbox whose planes ask for a branch
//! ([`wants_branch`]) gets an [`EnvBranchTask`]: `Migrate` for the build just
//! published when the environment already has a branch row, and a `Cut` (at
//! the sandbox's `fork_at` while the history reaches it, else now) when it has
//! none — a cut applies the build the sandbox serves anyway. **A branch-mode sandbox never gets a
//! `SandboxOltpTask`:** that task seeds a schema in staging's branch, which
//! this sandbox does not use.
//!
//! A failure to queue is a warning on the publish response, as for every
//! other migration a publish queues; it never fails the publish.

use oxy_oltp::env_branches::find_for_app;
use sea_orm::DatabaseConnection;

use super::planes::fork_point;
use super::task::{BranchOp, EnvBranchTask, enqueue};
use super::wants_branch;
use crate::custom_apps_env_resolve::sandbox_row;
use crate::custom_apps_sandboxes::publish::SandboxBuild;

/// The warnings of queueing the branch task for `build`'s sandbox, or `None`
/// when the sandbox does not ask for a branch (the caller queues its schema
/// task as before).
pub(crate) async fn queue_branch_migrations(
    db: &DatabaseConnection,
    build: SandboxBuild<'_>,
) -> Option<Vec<String>> {
    let environment = build.environment;
    let not_queued = |why: String| {
        tracing::warn!(app_id = %build.app_id, %environment, "publish: {why}");
        Some(vec![format!(
            "{environment}'s own OLTP branch was not migrated ({why}); the publish went on, and \
             the sandbox's ctx.oltp may be refused or lack this build's tables until a later \
             publish to it queues the migration"
        )])
    };
    let row = match sandbox_row(db, build.app_id, environment).await {
        Ok(Some(row)) => row,
        Ok(None) => return None,
        // The mode is unknown: leave it to `queue_oltp_schema`, which reads its
        // own state and warns if that fails too.
        Err(e) => {
            tracing::warn!(app_id = %build.app_id, %environment, error = %e,
                "publish: the sandbox's row could not be read to choose its OLTP task");
            return None;
        }
    };
    wants_branch(&row)?;
    let cut = match find_for_app(db, build.app_id, &environment.name()).await {
        Ok(branch) => branch.is_none(),
        Err(e) => return not_queued(format!("the branch row could not be read: {e}")),
    };
    let op = match cut {
        // The recorded fork point while the history still reaches it, else
        // now (recorded) — a sandbox whose first cut was refused days ago
        // would otherwise fail its cut as outside the history.
        true => match fork_point(db, &row).await {
            Ok(at) => BranchOp::Cut { at },
            Err(e) => return not_queued(format!("the fork point could not be recorded: {e}")),
        },
        false => BranchOp::Migrate {
            build_pk: build.build_pk,
        },
    };
    let task = EnvBranchTask {
        app_id: build.app_id,
        app_slug: build.app_slug.to_string(),
        org_id: build.org_id,
        workspace_id: build.workspace_id,
        environment: environment.name(),
        op,
    };
    match enqueue(db, &task).await {
        Ok(_) => Some(Vec::new()),
        Err(e) => not_queued(e.to_string()),
    }
}
