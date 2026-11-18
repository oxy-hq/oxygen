//! An org branch's Oxy-owned objects brought current, lazily: the branch half
//! of [`super::reconcile_platform_schema_as_owner`].
//!
//! A branch is a copy of production taken when it was cut, so it carries the
//! platform steps production had then and none added since (a branch cut
//! before the events outbox has no `oxy_events`). Its row records the version
//! it is known to carry (`oltp_branches.platform_schema_version`), and the
//! first touch that needs a newer object — the outbox's grant or claim —
//! applies the steps after it, through [`super::apply_platform_steps`], the
//! one path a tenant is brought current by, as the branch's owner.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tracing::info;

use super::ProvisionerError;
use crate::branches::{BranchCut, CutScope};
use crate::platform::PLATFORM_SCHEMA_VERSION;
use crate::resolver::BranchOwner;
use crate::sql::TenantSqlExecutor;

/// Apply the platform steps `owner`'s branch lacks, as its owner, then record
/// it current. Nothing to do when it already is. Recorded only after the
/// steps succeed, and only on the cut they ran on: a branch left behind is
/// brought current on its next touch, and one reset since holds a fresh copy
/// of production whose row its reset recorded.
///
/// The steps are the tenant's, named by the tenant's provider and database:
/// a branch keeps the tenant's roles (an analyst named after the tenant's
/// database), and a Neon branch's database has the tenant's name. On a local
/// cluster, step 1's `REVOKE … ON DATABASE` therefore lands on production's
/// database, where it already holds; every step is idempotent.
pub(crate) async fn reconcile_branch_platform_schema_as_owner(
    db: &DatabaseConnection,
    sql: &dyn TenantSqlExecutor,
    owner: &BranchOwner,
) -> Result<(), ProvisionerError> {
    let from = owner.platform_schema_version;
    if from >= PLATFORM_SCHEMA_VERSION {
        return Ok(());
    }
    super::apply_platform_steps(
        sql,
        &owner.dsn,
        from,
        &owner.tenant.provider,
        &owner.tenant.database_name,
    )
    .await?;
    record_current(db, &owner.cut).await?;
    info!(
        from,
        to = PLATFORM_SCHEMA_VERSION,
        branch = %owner.cut.provider_branch_id,
        "reconciled OLTP branch platform schema"
    );
    Ok(())
}

/// The org's staging branch's record: `$1` the version, `$2` the row,
/// `$3` the provider's branch and `$4` the cut, as [`record_current`] binds
/// them.
const RECORD_ORG_BRANCH_SQL: &str = "UPDATE oltp_branches \
       SET platform_schema_version = $1, updated_at = now() \
     WHERE id = $2 AND status = 'active' AND provider_branch_id = $3 \
       AND COALESCE(last_reset_at, created_at) = $4 AND platform_schema_version < $1";

/// A sandbox's own branch's record, the version with the cut it holds on
/// (`oltp_env_branches.platform_schema_cut_at`): a row recorded on another
/// cut is recorded again, whatever version it names.
const RECORD_ENV_BRANCH_SQL: &str = "UPDATE oltp_env_branches \
       SET platform_schema_version = $1, platform_schema_cut_at = $4, updated_at = now() \
     WHERE id = $2 AND status IN ('ready', 'migrating') AND provider_branch_id = $3 \
       AND COALESCE(last_reset_at, created_at) = $4 \
       AND (platform_schema_version < $1 OR platform_schema_cut_at IS DISTINCT FROM $4)";

/// Record `cut`'s branch at [`PLATFORM_SCHEMA_VERSION`], in one statement
/// that matches only the row still describing that cut, serving: a reset or
/// a re-cut since copied production again, and recorded that copy itself.
/// The org's staging branch is an `oltp_branches` row, a sandbox's own branch
/// an `oltp_env_branches` one (`ready`, or `migrating` while a publish applies
/// its migrations), which also records the cut the version holds on.
async fn record_current(db: &DatabaseConnection, cut: &BranchCut) -> Result<(), ProvisionerError> {
    let sql = match cut.scope {
        CutScope::OrgBranch => RECORD_ORG_BRANCH_SQL,
        CutScope::Environment => RECORD_ENV_BRANCH_SQL,
    };
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [
            PLATFORM_SCHEMA_VERSION.into(),
            cut.row_id.into(),
            cut.provider_branch_id.clone().into(),
            cut.cut_at.into(),
        ],
    ))
    .await?;
    Ok(())
}
