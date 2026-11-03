//! Where a branch lives and who keeps its role passwords — the two facts the
//! role machinery (`branch_roles.rs`) and the resolver need from a branch row,
//! whichever table the row is in.
//!
//! Two tables hold branches of an org's OLTP database: `oltp_branches` (the
//! org's shared staging branch) and `oltp_env_branches` (one custom-app
//! environment's own branch). They are shaped alike on purpose, so minting
//! roles, converging writers and building the owner's DSN are the same work
//! against either. A [`BranchSite`] is that work's input, projected from a row,
//! so none of it names a table's `Model`.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::OltpBranch;
use crate::entity::branch_roles::{self as branch_roles, Entity as BranchRoles};
use crate::entity::branches as oltp_branches;
use crate::entity::env_branch_roles::{self as env_branch_roles, Entity as EnvBranchRoles};
use crate::entity::env_branches;

use super::ProvisionerError;

/// Which table keeps the sealed per-role passwords of a branch, and the row
/// they hang off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleStore {
    /// `oltp_branch_roles`, keyed by an `oltp_branches` row.
    OrgBranch(Uuid),
    /// `oltp_env_branch_roles`, keyed by an `oltp_env_branches` row.
    Environment(Uuid),
}

impl RoleStore {
    /// The roles that already have a sealed password here.
    pub(crate) async fn minted(&self, db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
        Ok(match *self {
            RoleStore::OrgBranch(row_id) => BranchRoles::find()
                .filter(branch_roles::Column::BranchRowId.eq(row_id))
                .all(db)
                .await?
                .into_iter()
                .map(|r| r.role_name)
                .collect(),
            RoleStore::Environment(row_id) => EnvBranchRoles::find()
                .filter(env_branch_roles::Column::EnvBranchRowId.eq(row_id))
                .all(db)
                .await?
                .into_iter()
                .map(|r| r.role_name)
                .collect(),
        })
    }

    /// Record `role`'s freshly sealed password.
    pub(crate) async fn insert(
        &self,
        db: &DatabaseConnection,
        role: &str,
        sealed: Vec<u8>,
    ) -> Result<(), DbErr> {
        let now = chrono::Utc::now().into();
        match *self {
            RoleStore::OrgBranch(row_id) => {
                branch_roles::ActiveModel {
                    id: ActiveValue::Set(Uuid::new_v4()),
                    branch_row_id: ActiveValue::Set(row_id),
                    role_name: ActiveValue::Set(role.to_string()),
                    password_ciphertext: ActiveValue::Set(sealed),
                    created_at: ActiveValue::Set(now),
                }
                .insert(db)
                .await?;
            }
            RoleStore::Environment(row_id) => {
                env_branch_roles::ActiveModel {
                    id: ActiveValue::Set(Uuid::new_v4()),
                    env_branch_row_id: ActiveValue::Set(row_id),
                    role_name: ActiveValue::Set(role.to_string()),
                    password_ciphertext: ActiveValue::Set(sealed),
                    created_at: ActiveValue::Set(now),
                }
                .insert(db)
                .await?;
            }
        }
        Ok(())
    }

    /// Drop `role`'s sealed password. Absent is fine.
    pub(crate) async fn forget(&self, db: &DatabaseConnection, role: &str) -> Result<(), DbErr> {
        match *self {
            RoleStore::OrgBranch(row_id) => {
                BranchRoles::delete_many()
                    .filter(branch_roles::Column::BranchRowId.eq(row_id))
                    .filter(branch_roles::Column::RoleName.eq(role))
                    .exec(db)
                    .await?;
            }
            RoleStore::Environment(row_id) => {
                EnvBranchRoles::delete_many()
                    .filter(env_branch_roles::Column::EnvBranchRowId.eq(row_id))
                    .filter(env_branch_roles::Column::RoleName.eq(role))
                    .exec(db)
                    .await?;
            }
        }
        Ok(())
    }

    /// `role`'s sealed password, if one was minted.
    pub(crate) async fn password_of(
        &self,
        db: &DatabaseConnection,
        role: &str,
    ) -> Result<Option<Vec<u8>>, DbErr> {
        Ok(match *self {
            RoleStore::OrgBranch(row_id) => BranchRoles::find()
                .filter(branch_roles::Column::BranchRowId.eq(row_id))
                .filter(branch_roles::Column::RoleName.eq(role))
                .one(db)
                .await?
                .map(|r| r.password_ciphertext),
            RoleStore::Environment(row_id) => EnvBranchRoles::find()
                .filter(env_branch_roles::Column::EnvBranchRowId.eq(row_id))
                .filter(env_branch_roles::Column::RoleName.eq(role))
                .one(db)
                .await?
                .map(|r| r.password_ciphertext),
        })
    }
}

/// Where one branch is, who owns it, and where its role passwords are kept.
#[derive(Debug, Clone)]
pub(crate) struct BranchSite {
    /// What logs and errors call the branch: `"staging"` (the
    /// [`OltpBranch`]'s name) for the org's shared branch, the environment's
    /// name for an environment's.
    pub label: String,
    /// The branch row's id, in whichever table `roles` names.
    #[allow(dead_code)]
    pub row_id: Uuid,
    pub host: String,
    pub database_name: String,
    pub owner_role: String,
    /// The branch's own sealed owner password; `None` where it shares the
    /// tenant's owner (a local cluster) or none was minted yet.
    pub owner_password_ciphertext: Option<Vec<u8>>,
    pub roles: RoleStore,
}

impl BranchSite {
    /// The org's shared branch (`oltp_branches`).
    pub(crate) fn of_org_branch(row: &oltp_branches::Model) -> Self {
        Self {
            label: row.kind.to_string(),
            row_id: row.id,
            host: row.host.clone(),
            database_name: row.database_name.clone(),
            owner_role: row.owner_role.clone(),
            owner_password_ciphertext: row.owner_password_ciphertext.clone(),
            roles: RoleStore::OrgBranch(row.id),
        }
    }

    /// An environment's branch (`oltp_env_branches`). `None` while the row
    /// records an intent the provider has not yet answered: the host, database
    /// and owner arrive with the provider's branch.
    pub(crate) fn of_env(row: &env_branches::Model) -> Option<Self> {
        row.provider_branch_id.as_ref()?;
        Some(Self {
            label: row.env_name.clone(),
            row_id: row.id,
            host: row.host.clone()?,
            database_name: row.database_name.clone()?,
            owner_role: row.owner_role.clone()?,
            owner_password_ciphertext: row.owner_password_ciphertext.clone(),
            roles: RoleStore::Environment(row.id),
        })
    }

    /// The error for a branch that needs its own owner password and has none.
    /// The org's shared branch keeps its long-standing typed error; any other
    /// site is named by its label.
    pub(crate) fn owner_password_missing(&self, org_id: Uuid) -> ProvisionerError {
        match OltpBranch::parse(&self.label) {
            Some(kind) if matches!(self.roles, RoleStore::OrgBranch(_)) => {
                ProvisionerError::BranchOwnerPasswordMissing(org_id, kind)
            }
            _ => ProvisionerError::BranchSiteOwnerPasswordMissing(org_id, self.label.clone()),
        }
    }
}
