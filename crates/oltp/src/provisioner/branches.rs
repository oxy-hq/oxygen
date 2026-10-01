//! The org's staging branch: cut it, reset it, delete it.
//!
//! **Manual by ruling** (env design §11 #16, 2026-09-29): nothing here runs on
//! a timer or as a side effect of a publish. `provision_branch` is
//! `oxyc oltp provision --branch staging`; `reset_branch` is `oxyc oltp reset`,
//! which the API only reaches with an explicit confirmation; `deprovision`
//! deletes branches with the database, and `delete_branches` deletes only them.
//!
//! Every state that is not `active` is one a resolver refuses, so a failure
//! anywhere below leaves the branch unreachable rather than half-built:
//! `provisioning` until every credential exists, `resetting` from before the
//! provider call until the new credentials are sealed. Each operation holds the
//! `(tenant, kind)` lock from `branch_state.rs` for its whole run.

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::OltpBranch;
use crate::entity::branch_roles::{self as branch_roles, Entity as BranchRoles};
use crate::entity::branches::{self as oltp_branches, BranchStatus, Entity as OltpBranches};
use crate::entity::tenants as oltp_tenants;
use crate::provider::{BranchRequest, ProviderError};

use super::{OltpProvisioner, ProvisionerError};

impl OltpProvisioner {
    /// Create the org's `kind` branch, or finish one a failed attempt left.
    ///
    /// Idempotent. A re-run mints only the credentials that are missing and
    /// never rotates one that exists — a staging function mid-request must not
    /// lose its connection because an operator ran the command twice — and it
    /// brings the branch level with any writer provisioned since it was cut.
    #[instrument(skip(self), fields(org_id = %org_id, branch = %kind))]
    pub async fn provision_branch(
        &self,
        org_id: Uuid,
        kind: OltpBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let tenant = self.branchable_tenant(org_id).await?;
        let lock = self.lock_branch(tenant.id, kind).await?;
        let out = self.provision_branch_locked(&tenant, kind).await;
        lock.release().await;
        let row = out?;
        info!(host = %row.host, database = %row.database_name, "provisioned OLTP branch");
        Ok(row)
    }

    async fn provision_branch_locked(
        &self,
        tenant: &oltp_tenants::Model,
        kind: OltpBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let row = match self.find_branch(tenant, kind).await? {
            Some(row) if row.status == BranchStatus::Resetting => {
                return Err(ProvisionerError::BranchNotReady {
                    org_id: tenant.org_id,
                    branch: kind,
                    status: row.status.as_str(),
                });
            }
            Some(row) => row,
            None => {
                // Read before the copy (`branch_ledger`): the branch starts
                // with every migration production had applied when it was cut.
                let snapshot = self.production_ledger(tenant.org_id).await?;
                let cut = self
                    .provider
                    .create_branch(&branch_request(tenant, kind))
                    .await?;
                self.seed_branch_ledger(&cut.id, &snapshot).await?;
                self.insert_branch(tenant, kind, &cut).await?
            }
        };
        self.finish_branch(tenant, row).await
    }

    /// Throw the branch's data away and re-cut it from production's head.
    ///
    /// The caller has already had a human confirm it — the HTTP route refuses
    /// without `confirm: true` — because it discards every app's staging state
    /// in the org at once. Sets the branch's migration ledger to a copy of
    /// production's (`branch_ledger`): the fresh copy holds exactly what
    /// production had applied, so staging's own files are forgotten and
    /// production's are not run a second time. A reset that failed part
    /// way leaves the row `resetting`; running it again finishes it.
    #[instrument(skip(self), fields(org_id = %org_id, branch = %kind))]
    pub async fn reset_branch(
        &self,
        org_id: Uuid,
        kind: OltpBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let tenant = self.branchable_tenant(org_id).await?;
        let lock = self.lock_branch(tenant.id, kind).await?;
        let out = self.reset_branch_locked(&tenant, kind).await;
        lock.release().await;
        let row = out?;
        info!(host = %row.host, "reset OLTP branch from production");
        Ok(row)
    }

    async fn reset_branch_locked(
        &self,
        tenant: &oltp_tenants::Model,
        kind: OltpBranch,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        let row = self
            .find_branch(tenant, kind)
            .await?
            .ok_or(ProvisionerError::BranchNotProvisioned(tenant.org_id, kind))?;
        refuse_production(tenant, &row)?;

        // Unreachable from here until `finish_branch` seals the new credentials.
        let row = if row.status == BranchStatus::Resetting {
            row
        } else {
            self.set_status(row, BranchStatus::Resetting).await?
        };
        let req = branch_request(tenant, kind);
        // Read before the copy (`branch_ledger`).
        let snapshot = self.production_ledger(tenant.org_id).await?;
        let cut = match self
            .provider
            .reset_branch(&req, &row.provider_branch_id)
            .await
        {
            Ok(cut) => cut,
            // Gone provider-side (deleted by hand, expired): cut a new one
            // rather than leave the org with a row pointing at nothing.
            Err(ProviderError::BranchNotFound(id)) => {
                warn!(branch_id = %id, "branch vanished provider-side; cutting a new one");
                self.provider.create_branch(&req).await?
            }
            Err(e) => return Err(e.into()),
        };
        // The OLD id's ledger: when the branch was re-cut under a new id, the
        // new one has none yet, and the old rows would otherwise outlive it.
        self.clear_branch_ledger(&row.provider_branch_id).await?;
        BranchRoles::delete_many()
            .filter(branch_roles::Column::BranchRowId.eq(row.id))
            .exec(&self.db)
            .await?;
        let row = self.record_cut(row, &cut).await?;
        self.seed_branch_ledger(&row.provider_branch_id, &snapshot)
            .await?;
        self.finish_branch(tenant, row).await
    }

    /// Delete the org's branches and nothing else — the production database
    /// and its rows stay. What the staff console's org delete runs: that path
    /// does not tear down production OLTP (see `per-org-oltp-postgres.md`), but
    /// a branch is a copy of the org's data and goes with the org.
    ///
    /// Strict on every provider: the project survives this, so nothing will
    /// take a branch with it later.
    #[instrument(skip(self), fields(org_id = %org_id))]
    pub async fn delete_branches(&self, org_id: Uuid) -> Result<(), ProvisionerError> {
        let Some(tenant) = self.find_tenant(org_id).await? else {
            return Ok(());
        };
        self.assert_provider_matches(&tenant)?;
        self.delete_tenant_branches(&tenant, Strict::Always).await
    }

    /// Delete every branch of `tenant` at the provider, then its row and
    /// ledger. `deprovision`'s first step, and `delete_branches`' only one.
    pub(super) async fn delete_tenant_branches(
        &self,
        tenant: &oltp_tenants::Model,
        strict: Strict,
    ) -> Result<(), ProvisionerError> {
        for kind in self.branch_kinds(tenant).await? {
            let lock = self.lock_branch(tenant.id, kind).await?;
            // The row as it is now that this holds the lock: a reset that ran
            // while this waited may have re-cut the branch under a new id, and
            // deleting the id read before the wait would leave that one alive.
            let Some(row) = self.find_branch(tenant, kind).await? else {
                lock.release().await;
                continue;
            };
            let out = self.delete_one_branch(tenant, &row).await;
            lock.release().await;
            match (out, strict) {
                (Ok(()), _) => {}
                (Err(e), Strict::Always) => return Err(e),
                // The project delete that follows takes its branches on this
                // provider. A branch that would not go first — a provider
                // blip, or a row naming production that the guard refused —
                // must not keep the project, and the org's data in it, alive.
                // Its ledger still goes: the branch is gone once the project is.
                (Err(e), Strict::UnlessProjectDeleteTakesBranches) => {
                    error!(
                        branch = %row.kind,
                        branch_id = %row.provider_branch_id,
                        "could not delete the OLTP branch on its own; the project delete takes it: {e}"
                    );
                    self.clear_branch_ledger(&row.provider_branch_id).await?;
                }
            }
        }
        Ok(())
    }

    async fn delete_one_branch(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_branches::Model,
    ) -> Result<(), ProvisionerError> {
        refuse_production(tenant, row)?;
        self.provider
            .delete_branch(&branch_request(tenant, row.kind), &row.provider_branch_id)
            .await?;
        self.clear_branch_ledger(&row.provider_branch_id).await?;
        OltpBranches::delete_by_id(row.id).exec(&self.db).await?;
        info!(branch = %row.kind, branch_id = %row.provider_branch_id, "deleted OLTP branch");
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────────────

    /// Mint what is missing, converge writers, and only then call it active.
    async fn finish_branch(
        &self,
        tenant: &oltp_tenants::Model,
        row: oltp_branches::Model,
    ) -> Result<oltp_branches::Model, ProvisionerError> {
        // Before any credential is minted: on a row that names production,
        // minting is `ALTER ROLE … PASSWORD` on production's analyst and every
        // writer. Reset already checked; provision reaches here with a row it
        // only read, so this is the check both share.
        refuse_production(tenant, &row)?;
        // Roles before grants: a writer provisioned after the cut does not
        // exist on a Neon branch until its credential is minted there.
        self.mint_branch_credentials(tenant, &row).await?;
        self.converge_branch_writers(tenant, &row).await?;
        if row.status == BranchStatus::Active {
            return Ok(row);
        }
        self.set_status(row, BranchStatus::Active).await
    }

    /// The tenant a branch may be taken from: active, and on this provider.
    async fn branchable_tenant(
        &self,
        org_id: Uuid,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        let tenant = self.active_tenant(org_id).await?;
        self.assert_provider_matches(&tenant)?;
        Ok(tenant)
    }

    /// `deprovision`'s guard, for the same reason: ids are not portable across
    /// providers, and a branch op on the wrong one acts on nothing it can see.
    fn assert_provider_matches(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<(), ProvisionerError> {
        if tenant.provider != self.provider.name() {
            return Err(ProvisionerError::ProviderMismatch {
                org_id: tenant.org_id,
                recorded: tenant.provider.clone(),
                configured: self.provider.name(),
            });
        }
        Ok(())
    }
}

/// Whether a branch that will not delete stops the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Strict {
    Always,
    /// Log and carry on where the project delete that follows removes the
    /// branch anyway (Neon); strict where it would strand it (a local
    /// cluster's sibling database).
    UnlessProjectDeleteTakesBranches,
}

pub(super) fn branch_request(tenant: &oltp_tenants::Model, kind: OltpBranch) -> BranchRequest {
    BranchRequest {
        project_id: tenant.project_id.clone(),
        parent_branch_id: tenant.branch_id.clone(),
        name: kind.provider_name().to_string(),
        database_name: tenant.database_name.clone(),
        owner_role: tenant.owner_role.clone(),
    }
}

/// Refuse a branch row whose id names production — the tenant's own branch on
/// Neon, its own database on a local cluster. Nothing should ever record one,
/// which is exactly why a reset or delete must not trust that it didn't.
pub(super) fn refuse_production(
    tenant: &oltp_tenants::Model,
    row: &oltp_branches::Model,
) -> Result<(), ProvisionerError> {
    let id = row.provider_branch_id.as_str();
    if id == tenant.branch_id || id == tenant.project_id || id == tenant.database_name {
        return Err(ProvisionerError::BranchIsProduction {
            org_id: tenant.org_id,
            branch: row.kind,
            branch_id: id.to_string(),
        });
    }
    Ok(())
}
