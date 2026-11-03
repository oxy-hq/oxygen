//! Deleting environment branches: one by key, and all of a tenant's with the
//! org (`deprovision`, `delete_branches`). The twin of the staging branch's
//! delete in `branches.rs`; the cut and reset are `env_branches.rs`.

use sea_orm::EntityTrait;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::OltpBranch;
use crate::entity::env_branches::{self as rows, Entity};
use crate::entity::tenants as oltp_tenants;
use crate::env_branches::{EnvBranchKey, is_orphaned};
use crate::provider::{BranchInit, BranchRequest};

use super::branches::Strict;
use super::env_branches::{delete_request, refuse_env_production, refuse_unrecorded_production};
use super::{OltpProvisioner, ProvisionerError};

impl OltpProvisioner {
    /// Delete the environment's branch: the provider's, its ledger, its row
    /// (and with it the sealed roles). `Ok(false)` when there was none.
    #[instrument(skip(self), fields(org_id = %key.org_id, app_id = %key.app_id, env = %key.env_name))]
    pub async fn delete_env_branch(&self, key: &EnvBranchKey) -> Result<bool, ProvisionerError> {
        self.delete_env_branch_if(key, Holders::Any).await
    }

    /// [`Self::delete_env_branch`] for a branch no sandbox holds any more
    /// ([`crate::env_branches::orphaned`]) — only if, read again under the
    /// branch's lock, still no `app_environments` row holds `(app, env)`. A
    /// sandbox created again under the name since the sweep read keeps it.
    /// `Ok(false)` when nothing was deleted.
    #[instrument(skip(self), fields(org_id = %key.org_id, app_id = %key.app_id, env = %key.env_name))]
    pub async fn delete_orphaned_env_branch(
        &self,
        key: &EnvBranchKey,
    ) -> Result<bool, ProvisionerError> {
        self.delete_env_branch_if(key, Holders::NoneLeft).await
    }

    async fn delete_env_branch_if(
        &self,
        key: &EnvBranchKey,
        holders: Holders,
    ) -> Result<bool, ProvisionerError> {
        let Some(tenant) = self.find_tenant(key.org_id).await? else {
            return Ok(false);
        };
        self.assert_provider_matches(&tenant)?;
        let lock = self
            .lock_env_branch(tenant.id, key.app_id, &key.env_name)
            .await?;
        let out = self.delete_env_held(&tenant, key, holders).await;
        lock.release().await;
        out
    }

    /// With the branch's lock held: check who still holds it, then delete.
    async fn delete_env_held(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        holders: Holders,
    ) -> Result<bool, ProvisionerError> {
        if matches!(holders, Holders::NoneLeft)
            && !is_orphaned(&self.db, key.app_id, &key.env_name).await?
        {
            return Ok(false);
        }
        self.delete_env_locked(tenant, key.app_id, &key.env_name)
            .await
    }

    /// With the branch's lock held, on the row as it is now.
    pub(super) async fn delete_env_locked(
        &self,
        tenant: &oltp_tenants::Model,
        app_id: Uuid,
        env_name: &str,
    ) -> Result<bool, ProvisionerError> {
        let Some(row) = self.find_env_row(tenant, app_id, env_name).await? else {
            return Ok(false);
        };
        let out = self.delete_env_row(tenant, &row).await;
        self.failed_on_error(row.id, out).await?;
        Ok(true)
    }

    async fn delete_env_row(
        &self,
        tenant: &oltp_tenants::Model,
        row: &rows::Model,
    ) -> Result<(), ProvisionerError> {
        self.delete_env_provider_branches(tenant, row).await?;
        Entity::delete_by_id(row.id).exec(&self.db).await?;
        info!(env = %row.env_name, "deleted OLTP environment branch");
        Ok(())
    }

    /// Delete whatever the provider holds for `row` — the recorded branch,
    /// then anything left under the derived name — and its ledgers. The row
    /// itself is the caller's. Production is refused before any provider call.
    async fn delete_env_provider_branches(
        &self,
        tenant: &oltp_tenants::Model,
        row: &rows::Model,
    ) -> Result<(), ProvisionerError> {
        refuse_env_production(tenant, row)?;
        self.refuse_env_staging(tenant, row).await?;
        // Asked as the branch was cut: the row's mode, whatever comes next.
        let req = delete_request(tenant, row.app_id, &row.env_name, row.mode);
        if let Some(id) = &row.provider_branch_id {
            self.provider.delete_branch(&req, id).await?;
            self.clear_branch_ledger(id).await?;
        }
        self.delete_unrecorded_branch(tenant, row, &req).await
    }

    /// [`refuse_env_production`]'s twin for the org's shared staging branch.
    /// Provider names are derived and distinct, so only a corrupted recorded
    /// id can name it — and deleting "this sandbox's" branch would then delete
    /// every sandbox schema on staging, and staging's data with them.
    async fn refuse_env_staging(
        &self,
        tenant: &oltp_tenants::Model,
        row: &rows::Model,
    ) -> Result<(), ProvisionerError> {
        let Some(id) = row.provider_branch_id.as_deref() else {
            return Ok(());
        };
        let staging = self.find_branch(tenant, OltpBranch::Staging).await?;
        if staging.is_some_and(|s| s.provider_branch_id == id) {
            return Err(ProvisionerError::EnvBranchIsStaging {
                org_id: tenant.org_id,
                env_name: row.env_name.clone(),
                branch_id: id.to_string(),
            });
        }
        Ok(())
    }

    /// A cut resumed in another mode than the row records — a failed or
    /// interrupted `branch` cut picked up as `empty`, or the reverse. What the
    /// earlier attempt left under the derived name holds the other mode's data,
    /// and adopting it would serve production's rows as an empty branch (or an
    /// empty branch as a copy), so it is deleted and the row forgets it. The
    /// cut that follows starts from nothing: a fresh fork point, new roles.
    pub(super) async fn discard_other_mode(
        &self,
        tenant: &oltp_tenants::Model,
        row: rows::Model,
    ) -> Result<rows::Model, ProvisionerError> {
        warn!(
            env = %row.env_name,
            recorded = ?row.mode,
            "an environment branch cut changed mode; deleting the earlier attempt's branch"
        );
        self.delete_env_provider_branches(tenant, &row).await?;
        self.clear_env_roles(row.id).await?;
        self.forget_env_cut(row).await
    }

    /// Delete a branch the provider made under this environment's name that no
    /// row records: a cut whose answer was lost or never recorded, or a re-cut
    /// that created the new branch and failed before recording it. The name is
    /// derived, so a match is ours. Without this the row is deleted and the
    /// branch never goes — for an empty one, which carries no provider expiry,
    /// never at all. Looked up after the recorded id is deleted, so it finds
    /// only what is left over.
    pub(super) async fn delete_unrecorded_branch(
        &self,
        tenant: &oltp_tenants::Model,
        row: &rows::Model,
        req: &BranchRequest,
    ) -> Result<(), ProvisionerError> {
        let Some(found) = self.provider.find_branch_id(req).await? else {
            return Ok(());
        };
        refuse_unrecorded_production(tenant, row, &found)?;
        warn!(branch_id = %found, env = %row.env_name, "deleting an environment branch no row recorded");
        // Asked as schema-only, the one shape whose delete accepts both a
        // branch cut from production and a parentless one: what is left under
        // the name may be either — a failed re-cut in a new mode leaves its
        // branch in that mode while the row still records the old one.
        let mut stale = req.clone();
        stale.init = BranchInit::SchemaOnly;
        self.provider.delete_branch(&stale, &found).await?;
        self.clear_branch_ledger(&found).await
    }

    /// Delete every environment branch of `tenant`. `delete_tenant_branches`'
    /// second half, with the same strictness as the staging branch's.
    pub(super) async fn delete_tenant_env_branches(
        &self,
        tenant: &oltp_tenants::Model,
        strict: Strict,
    ) -> Result<(), ProvisionerError> {
        for (app_id, env_name) in self.env_branch_keys(tenant).await? {
            let lock = self.lock_env_branch(tenant.id, app_id, &env_name).await?;
            // Re-read under the lock: a reset that ran while this waited may
            // have re-cut the branch under a new id.
            let out = self.delete_env_locked(tenant, app_id, &env_name).await;
            lock.release().await;
            match (out, strict) {
                (Ok(_), _) => {}
                (Err(e), Strict::Always) => return Err(e),
                // The project delete that follows takes the branch on this
                // provider; one that would not go first must not keep the
                // project, and the org's data in it, alive.
                // Its ledger still goes: the branch is gone once the project is.
                (Err(e), Strict::UnlessProjectDeleteTakesBranches) => {
                    log_env_delete_failure(tenant, app_id, &env_name, &e);
                    self.clear_env_ledger_of(tenant, app_id, &env_name).await?;
                }
            }
        }
        Ok(())
    }

    async fn clear_env_ledger_of(
        &self,
        tenant: &oltp_tenants::Model,
        app_id: Uuid,
        env_name: &str,
    ) -> Result<(), ProvisionerError> {
        let row = self.find_env_row(tenant, app_id, env_name).await?;
        if let Some(id) = row.and_then(|r| r.provider_branch_id) {
            self.clear_branch_ledger(&id).await?;
        }
        Ok(())
    }
}

/// Which environment branches a delete may take: any, or only one no
/// sandbox holds any more.
#[derive(Debug, Clone, Copy)]
enum Holders {
    Any,
    NoneLeft,
}

fn log_env_delete_failure(
    tenant: &oltp_tenants::Model,
    app_id: Uuid,
    env_name: &str,
    e: &ProvisionerError,
) {
    tracing::error!(
        org_id = %tenant.org_id,
        app_id = %app_id,
        env = env_name,
        "could not delete the OLTP environment branch on its own; the project delete takes it: {e}"
    );
}
