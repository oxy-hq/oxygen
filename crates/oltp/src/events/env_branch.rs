//! The outbox on a sandbox's own OLTP branch (app events design §6.7): a
//! sandbox whose planes put `ctx.oltp` on a branch of its own (`oltp` is
//! `branch` or `empty`, `oltp_env_branches`) publishes there, and its events
//! wait in that branch's outbox ([`OutboxTarget::EnvBranch`](super::OutboxTarget::EnvBranch)).
//!
//! **A branch is a copy of production at its cut** (or production's structure
//! alone, for `empty`), so it carries the platform steps production had then.
//! Its row records the version it is known to carry
//! (`oltp_env_branches.platform_schema_version`) and the cut it was recorded
//! on (`platform_schema_cut_at`); on any other cut — every cut and reset, an
//! older binary's reset among them, which leaves the version alone — the
//! branch reads as version 0. The first grant or claim applies the steps after
//! it as the branch's owner, through the path the staging branch and a tenant
//! are brought current by, and records the version on the cut. A read never
//! reconciles.
//!
//! **`None` is "nothing to reach"**: the environment has no branch row (never
//! cut, or torn down with its sandbox), or one not serving now — being cut,
//! mid-reset or failed. A sandbox's teardown drops its branch, so a body read
//! there finds nothing and a delete deletes nothing, and a relay pass drains
//! rather than retrying forever.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::EventsError;
use super::branch::resolve_error;
use crate::provisioner::reconcile_branch_platform_schema_as_owner;
use crate::resolver::resolve_env_branch_owner;
use crate::sql::PgSqlExecutor;

/// The owner DSN of `app_id`'s `environment` branch. With `bring_current` (a
/// publish's grant, a claim), its Oxy-owned objects are brought current
/// first. `None`: no such branch serving now.
pub(super) async fn owner_dsn(
    control: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
    environment: &str,
    bring_current: bool,
) -> Result<Option<String>, EventsError> {
    let Some(owner) = resolve_env_branch_owner(control, org_id, app_id, environment)
        .await
        .map_err(|e| resolve_error(org_id, e))?
    else {
        return Ok(None);
    };
    if bring_current {
        reconcile_branch_platform_schema_as_owner(control, &PgSqlExecutor, &owner)
            .await
            .map_err(|e| {
                EventsError::Store(format!("bring the sandbox's OLTP branch up to date: {e}"))
            })?;
    }
    Ok(Some(owner.dsn))
}
