//! A branch row's state, and the lock that keeps two operations off it at once.
//!
//! Provision, reset, delete and an app's release on one `(tenant, kind)` each
//! take a transaction-scoped advisory lock for their whole run — provider call
//! included — so a double-clicked provision cannot buy two branches and a reset
//! cannot interleave with a provision's credential step. Every status change is
//! also compare-and-set against the status the caller read, so a write that
//! lost track of the row (a process that started before the lock existed, a
//! hand-edited row) fails loudly instead of flipping `resetting` back to
//! `active`.
//!
//! **The lock pins one control-plane connection** for the operation's length,
//! and that transaction sits idle while the provider works (seconds on Neon).
//! A managed Postgres with `idle_in_transaction_session_timeout` set would kill
//! it mid-operation and mutual exclusion would vanish without an error, so the
//! lock transaction turns that timeout off for itself (`SET LOCAL`, which ends
//! with the transaction). Acceptable for a manual staff operation; not a
//! pattern for a request path.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseTransaction, EntityTrait, QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::OltpBranch;
use crate::entity::branches::{self as oltp_branches, BranchStatus, Entity as OltpBranches};
use crate::entity::tenants as oltp_tenants;
use crate::provider::ProjectBranch;

use super::{OltpProvisioner, ProvisionerError, seal};

/// An advisory lock held by an otherwise idle transaction; released when the
/// transaction ends — by [`Self::release`], or by the rollback a drop sends
/// when an error unwinds past it.
pub(super) struct BranchLock(DatabaseTransaction);

impl BranchLock {
    pub(super) async fn release(self) {
        if let Err(e) = self.0.commit().await {
            tracing::warn!("releasing the OLTP branch lock: {e}");
        }
    }
}

/// Scoped to the lock's transaction; the pooled session gets its own setting
/// back when the transaction ends.
const IDLE_TIMEOUT_OFF: &str = "SET LOCAL idle_in_transaction_session_timeout = 0";

/// One lock per `(tenant, kind)`, spread over Postgres' bigint key space. A
/// collision only serializes two unrelated branch operations.
fn lock_key(tenant_row_id: Uuid, kind: OltpBranch) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("oxy-oltp-branch/{tenant_row_id}/{kind}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h as i64
}

impl OltpProvisioner {
    /// Wait for, and take, the lock on this tenant's `kind` branch.
    pub(super) async fn lock_branch(
        &self,
        tenant_row_id: Uuid,
        kind: OltpBranch,
    ) -> Result<BranchLock, ProvisionerError> {
        let txn = self.db.begin().await?;
        // Idle by design while the provider works — see the module header.
        txn.execute_unprepared(IDLE_TIMEOUT_OFF).await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [lock_key(tenant_row_id, kind).into()],
        ))
        .await?;
        Ok(BranchLock(txn))
    }

    pub(super) async fn find_branch(
        &self,
        tenant: &oltp_tenants::Model,
        kind: OltpBranch,
    ) -> Result<Option<oltp_branches::Model>, ProvisionerError> {
        Ok(OltpBranches::find()
            .filter(oltp_branches::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_branches::Column::Kind.eq(kind))
            .one(&self.db)
            .await?)
    }

    /// Which branches the tenant has — kinds only, because an operation that
    /// walks them must re-read each row under its lock, not trust this read.
    pub(super) async fn branch_kinds(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<Vec<OltpBranch>, ProvisionerError> {
        Ok(OltpBranches::find()
            .filter(oltp_branches::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|row| row.kind)
            .collect())
    }

    /// Recorded as `provisioning` straight after the provider call, so a crash
    /// in the credential step leaves a row the next run finishes — and a crash
    /// BEFORE this insert leaves a branch `create_branch` adopts by name.
    pub(super) async fn insert_branch(
        &self,
        tenant: &oltp_tenants::Model,
        kind: OltpBranch,
        cut: &ProjectBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let now = Utc::now();
        let mut active = oltp_branches::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            tenant_row_id: ActiveValue::Set(tenant.id),
            kind: ActiveValue::Set(kind),
            status: ActiveValue::Set(BranchStatus::Provisioning),
            created_at: ActiveValue::Set(now.into()),
            last_reset_at: ActiveValue::Set(None),
            updated_at: ActiveValue::Set(now.into()),
            ..Default::default()
        };
        apply_cut(&mut active, cut)?;
        Ok(active.insert(&self.db).await?)
    }

    /// Record a re-cut — only while the row is still `resetting`.
    pub(super) async fn record_cut(
        &self,
        row: oltp_branches::Model,
        cut: &ProjectBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        // Only the columns that change are Set; `update_many().set()` writes
        // exactly those.
        let mut changes = <oltp_branches::ActiveModel as Default>::default();
        apply_cut(&mut changes, cut)?;
        changes.last_reset_at = ActiveValue::Set(Some(Utc::now().into()));
        changes.updated_at = ActiveValue::Set(Utc::now().into());
        self.update_if(changes, &row, BranchStatus::Resetting).await
    }

    /// Move `row` to `to` if it is still in the status `row` was read with.
    pub(super) async fn set_status(
        &self,
        row: oltp_branches::Model,
        to: BranchStatus,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let expected = row.status;
        let changes = oltp_branches::ActiveModel {
            status: ActiveValue::Set(to),
            updated_at: ActiveValue::Set(Utc::now().into()),
            ..Default::default()
        };
        self.update_if(changes, &row, expected).await
    }

    /// Test seam for the compare-and-set: the only way to hand `set_status` a
    /// stale row is to hold one, which no public path does under the lock.
    #[doc(hidden)]
    pub async fn set_branch_status_for_test(
        &self,
        row: oltp_branches::Model,
        to: BranchStatus,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        self.set_status(row, to).await
    }

    /// `UPDATE … WHERE id = $id AND status = $expected`, or a typed refusal
    /// naming what the row turned out to be.
    async fn update_if(
        &self,
        changes: oltp_branches::ActiveModel,
        row: &oltp_branches::Model,
        expected: BranchStatus,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let result = OltpBranches::update_many()
            .set(changes)
            .filter(oltp_branches::Column::Id.eq(row.id))
            .filter(oltp_branches::Column::Status.eq(expected))
            .exec(&self.db)
            .await?;
        let current = OltpBranches::find_by_id(row.id).one(&self.db).await?;
        match (result.rows_affected, current) {
            (1, Some(updated)) => Ok(updated),
            (_, current) => Err(ProvisionerError::BranchStateChanged {
                branch: row.kind,
                expected: expected.as_str(),
                found: current.map_or("deleted", |r| r.status.as_str()),
            }),
        }
    }

    /// Forget what staging applied to this branch, so it re-migrates.
    pub(super) async fn clear_branch_ledger(
        &self,
        provider_branch_id: &str,
    ) -> Result<(), ProvisionerError> {
        use entity::custom_app_migrations as ledger;
        ledger::Entity::delete_many()
            .filter(ledger::Column::Store.eq(crate::branches::LEDGER_STORE_OLTP))
            .filter(ledger::Column::Target.eq(crate::branches::ledger_target(provider_branch_id)))
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

/// Copy the provider's answer onto a branch row, sealing a disclosed owner
/// password. `None` stays `None`: that provider's branch shares the tenant's
/// owner, and there is no branch-only secret to keep.
fn apply_cut(
    active: &mut oltp_branches::ActiveModel,
    cut: &ProjectBranch,
) -> Result<(), ProvisionerError> {
    active.provider_branch_id = ActiveValue::Set(cut.id.clone());
    active.parent_branch_id = ActiveValue::Set(cut.parent_id.clone());
    active.host = ActiveValue::Set(cut.host.clone());
    active.database_name = ActiveValue::Set(cut.database.name.clone());
    active.owner_role = ActiveValue::Set(cut.owner_role.name.clone());
    active.owner_password_ciphertext = ActiveValue::Set(match &cut.owner_role.password {
        Some(pw) => Some(seal(pw)?),
        None => None,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_per_tenant_and_stable() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            lock_key(a, OltpBranch::Staging),
            lock_key(a, OltpBranch::Staging)
        );
        assert_ne!(
            lock_key(a, OltpBranch::Staging),
            lock_key(b, OltpBranch::Staging)
        );
    }
}
