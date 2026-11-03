//! Resetting a custom-app environment's OLTP branch: restore it from its
//! parent, or delete it and cut it again — in the mode the caller now asks for.
//! The cut, the landing of a provider's answer and the status moves are
//! `env_branches.rs`; this is the other way into them.

use chrono::{DateTime, Utc};
use tracing::{info, instrument, warn};

use crate::entity::env_branches::{self as rows, EnvBranchMode, EnvBranchStatus};
use crate::entity::tenants as oltp_tenants;
use crate::env_branches::EnvBranchKey;
use crate::provider::{BranchInit, BranchRequest, ProjectBranch, ProviderError};

use super::env_branches::{
    Landing, check_fork, cut_times, db_precision, env_request, refuse_env_production,
};
use super::{OltpProvisioner, ProvisionerError};

impl OltpProvisioner {
    /// Throw the environment's data away and cut it again.
    ///
    /// A `branch` row with `at == None` is restored in place from its parent
    /// (`reset_branch`). `at.is_some()` or an `empty` branch is deleted at the
    /// provider and cut again — an empty one has no parent data to restore from.
    /// Either way the old ledger and roles are forgotten, the fork point moves,
    /// and the branch comes back `migrating`. A reset that failed part way is
    /// `failed` (or `resetting`); running it again finishes it.
    pub async fn reset_env_branch(
        &self,
        key: &EnvBranchKey,
        at: Option<DateTime<Utc>>,
    ) -> Result<rows::Model, ProvisionerError> {
        self.reset_env_branch_as(key, at, None).await
    }

    /// [`Self::reset_env_branch`], cutting again in `mode` when that is not the
    /// mode the row records (`None`: the row's). A switch between `branch` and
    /// `empty` can only be a delete and a fresh cut, whatever `at` says: one
    /// has a parent to restore from and the other has none. The row records
    /// the new mode once the provider's answer is landed.
    #[instrument(skip(self), fields(org_id = %key.org_id, app_id = %key.app_id, env = %key.env_name))]
    pub async fn reset_env_branch_as(
        &self,
        key: &EnvBranchKey,
        at: Option<DateTime<Utc>>,
        mode: Option<EnvBranchMode>,
    ) -> Result<rows::Model, ProvisionerError> {
        check_fork(at)?;
        let at = at.map(db_precision);
        let tenant = self.branchable_tenant(key.org_id).await?;
        let lock = self
            .lock_env_branch(tenant.id, key.app_id, &key.env_name)
            .await?;
        let out = self.reset_env_locked(&tenant, key, at, mode).await;
        lock.release().await;
        let row = out?;
        info!(host = ?row.host, "reset OLTP environment branch");
        Ok(row)
    }

    async fn reset_env_locked(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        at: Option<DateTime<Utc>>,
        mode: Option<EnvBranchMode>,
    ) -> Result<rows::Model, ProvisionerError> {
        let row = self
            .find_env_row(tenant, key.app_id, &key.env_name)
            .await?
            .ok_or_else(|| ProvisionerError::EnvBranchNotCut {
                org_id: key.org_id,
                env_name: key.env_name.clone(),
            })?;
        refuse_env_production(tenant, &row)?;
        let id = row.id;
        let out = self.run_reset(tenant, key, row, at, mode).await;
        self.failed_on_error(id, out).await
    }

    async fn run_reset(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        row: rows::Model,
        at: Option<DateTime<Utc>>,
        mode: Option<EnvBranchMode>,
    ) -> Result<rows::Model, ProvisionerError> {
        let mode = mode.unwrap_or(row.mode);
        // Unreachable from here until the new credentials are sealed.
        let row = if row.status == EnvBranchStatus::Resetting {
            row
        } else {
            self.set_env_status(row, EnvBranchStatus::Resetting).await?
        };
        let times = cut_times(at);
        let snapshot = self
            .production_ledger_at(tenant.org_id, times.fork_at)
            .await?;
        let req = env_request(tenant, key, mode, at, times);
        let cut = self.recut(tenant, &req, &row, at).await?;
        // The OLD id's ledger: a re-cut under a new id leaves it behind.
        if let Some(old) = &row.provider_branch_id {
            self.clear_branch_ledger(old).await?;
        }
        self.clear_env_roles(row.id).await?;
        let landing = Landing {
            tenant,
            key,
            cut,
            expected: EnvBranchStatus::Resetting,
            snapshot: &snapshot,
            times,
            mode,
        };
        self.land_cut(row, landing).await
    }

    /// The provider half of a reset: restore in place, or delete and cut again.
    async fn recut(
        &self,
        tenant: &oltp_tenants::Model,
        req: &BranchRequest,
        row: &rows::Model,
        at: Option<DateTime<Utc>>,
    ) -> Result<ProjectBranch, ProvisionerError> {
        let Some(id) = row.provider_branch_id.as_deref() else {
            return self.cut_fresh(tenant, req, row).await;
        };
        let restorable = row.mode == EnvBranchMode::Branch && req.init == BranchInit::ParentData;
        if restorable && at.is_none() {
            return match self.provider.reset_branch(req, id).await {
                Ok(cut) => {
                    // A restore keeps the branch, and the provider keeps its
                    // expiry: move it to the new fork's, or the provider reaps
                    // the branch at the old date while the row says ready.
                    if let Some(at) = req.expires_at {
                        self.provider.set_branch_expiry(req, id, at).await?;
                    }
                    Ok(cut)
                }
                // Gone provider-side (deleted by hand, expired): cut a new one.
                Err(ProviderError::BranchNotFound(gone)) => {
                    warn!(branch_id = %gone, "environment branch vanished provider-side; cutting anew");
                    self.cut_fresh(tenant, req, row).await
                }
                Err(e) => Err(e.into()),
            };
        }
        // The branch is deleted as it was cut: a switch of mode changes what
        // `req` asks to create, not what is there to delete.
        let mut gone = req.clone();
        gone.init = match row.mode {
            EnvBranchMode::Empty => BranchInit::SchemaOnly,
            EnvBranchMode::Branch => BranchInit::ParentData,
        };
        self.provider.delete_branch(&gone, id).await?;
        // An earlier re-cut may have created its branch and failed before
        // recording it: that one is under the name too, and is not adopted.
        self.cut_fresh(tenant, req, row).await
    }

    /// A reset's create when the row's branch is not there to restore or
    /// delete: the row recorded none, or the provider lost it. A branch still
    /// under the derived name is a stale one — an answer never recorded, or an
    /// earlier re-cut that created its branch and failed before recording it
    /// — holding data from an earlier instant. Adopting it would seed a ledger
    /// as of now over that data and credit migrations it lacks, and a reset
    /// means fresh, so it is deleted before the create.
    async fn cut_fresh(
        &self,
        tenant: &oltp_tenants::Model,
        req: &BranchRequest,
        row: &rows::Model,
    ) -> Result<ProjectBranch, ProvisionerError> {
        self.delete_unrecorded_branch(tenant, row, req).await?;
        Ok(self.provider.create_branch(req).await?)
    }
}
