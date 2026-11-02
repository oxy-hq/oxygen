//! Give a workspace its Airhouse tenant when it has none.
//!
//! In a cloud deployment every workspace's data lives in its Airhouse, so a
//! workspace is given a tenant when it is created, and anything that needs one
//! (a file import, the demo) asks again first. Both go through
//! [`ensure_tenant`], which returns at once for a workspace whose tenant is
//! usable and otherwise calls [`crate::TenantProvisioner::provision`], which
//! takes a per-workspace lock, so callers racing converge on one tenant and one
//! service account.
//!
//! The caller decides whether this process may provision at all: a local
//! `OXY_ROLE=all` process never does (it is not a cloud deployment).

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use thiserror::Error;
use uuid::Uuid;

use crate::entity::tenants::{self, TenantStatus};
use crate::provisioner::{ProvisionerError, tenant_name_for_workspace};

/// What [`ensure_tenant`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsuredTenant {
    /// The workspace already had an active tenant; Airhouse was not called.
    AlreadyActive,
    /// The workspace had none (or a failed one), and now has one.
    Provisioned,
}

#[derive(Debug, Error)]
pub enum EnsureTenantError {
    /// This deployment has no Airhouse (`AIRHOUSE_*` unset).
    #[error("Airhouse is not configured on this deployment")]
    NotConfigured,
    /// The tenant is being deleted. Provisioning now would fight the delete.
    #[error("this workspace's Airhouse tenant is being deleted")]
    PendingDelete,
    #[error("could not look up this workspace's Airhouse tenant: {0}")]
    Lookup(#[from] sea_orm::DbErr),
    #[error("this workspace has no Airhouse tenant, and provisioning one failed: {0}")]
    Provision(#[from] ProvisionerError),
}

/// Make sure `workspace_id` has a usable Airhouse tenant (see [`is_usable`]),
/// provisioning one under [`tenant_name_for_workspace`] when it has none.
///
/// Concurrent callers, on one pod or several, are serialized inside
/// [`crate::TenantProvisioner::provision`] by a per-workspace lock: the second
/// waits for the first, then finds the tenant and its service account saved and
/// changes nothing. So nothing here waits or retries.
pub async fn ensure_tenant(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<EnsuredTenant, EnsureTenantError> {
    let existing = tenants::Entity::find()
        .filter(tenants::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await?;
    match existing {
        Some(row) if is_usable(&row) => return Ok(EnsuredTenant::AlreadyActive),
        Some(row) if row.status == TenantStatus::PendingDelete => {
            return Err(EnsureTenantError::PendingDelete);
        }
        // No row, a failed one, or an active one whose service account is not
        // saved (being minted by another caller, from before service accounts,
        // or left by a provision that failed): `provision` waits for any
        // caller holding the lock, then finishes whatever is missing.
        Some(_) | None => {}
    }
    let provisioner =
        crate::config::provisioner_for(db.clone()).ok_or(EnsureTenantError::NotConfigured)?;
    provisioner
        .provision(workspace_id, tenant_name_for_workspace(workspace_id))
        .await?;
    Ok(EnsuredTenant::Provisioned)
}

/// A tenant an import can use: active, with its service account and the
/// sealed bearer saved. `provision` saves the row as active first and the
/// service account after, so active alone is not enough: the broker refuses a
/// row with no bearer (`TenantHasNoServiceAccount`).
fn is_usable(row: &tenants::Model) -> bool {
    row.status == TenantStatus::Active
        && row.service_account_id.is_some()
        && row.bearer_ciphertext.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: TenantStatus, service_account: bool) -> tenants::Model {
        tenants::Model {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            airhouse_tenant_id: "t".into(),
            bucket: "b".into(),
            prefix: None,
            status,
            created_at: chrono::Utc::now().fixed_offset(),
            service_account_id: service_account.then(|| "sa".to_string()),
            bearer_ciphertext: service_account.then(|| vec![1]),
            bearer_max_role: None,
            bearer_max_ttl_secs: None,
            sa_created_at: None,
            sa_rotated_at: None,
        }
    }

    #[test]
    fn active_is_usable_only_with_its_service_account_saved() {
        assert!(is_usable(&row(TenantStatus::Active, true)));
        assert!(!is_usable(&row(TenantStatus::Active, false)));
        assert!(!is_usable(&row(TenantStatus::Failed, true)));
    }
}
