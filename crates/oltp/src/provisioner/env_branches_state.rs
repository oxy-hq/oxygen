//! An environment branch row's state: find it, record an intent, record the
//! provider's answer, and move its status compare-and-set.
//!
//! The twin of `branch_state.rs` for `oltp_env_branches`. The lock that keeps
//! two operations off one row is `lock_env_branch` there; everything here runs
//! while it is held.

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::entity::env_branch_roles::{self as env_branch_roles, Entity as EnvBranchRoles};
use crate::entity::env_branches::{
    self as rows, Entity as EnvBranches, EnvBranchMode, EnvBranchStatus,
};
use crate::entity::tenants as oltp_tenants;
use crate::env_branches::EnvBranchKey;
use crate::provider::ProjectBranch;

use super::{OltpProvisioner, ProvisionerError, seal};

/// The instants a cut records: where the branch forked, and when it lapses.
#[derive(Debug, Clone, Copy)]
pub(super) struct CutTimes {
    pub fork_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl OltpProvisioner {
    pub(super) async fn find_env_row(
        &self,
        tenant: &oltp_tenants::Model,
        app_id: Uuid,
        env_name: &str,
    ) -> Result<Option<rows::Model>, ProvisionerError> {
        Ok(EnvBranches::find()
            .filter(rows::Column::TenantRowId.eq(tenant.id))
            .filter(rows::Column::AppId.eq(app_id))
            .filter(rows::Column::EnvName.eq(env_name))
            .one(&self.db)
            .await?)
    }

    /// Which environment branches the tenant has — keys only, because an
    /// operation that walks them must re-read each row under its lock.
    pub(super) async fn env_branch_keys(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<Vec<(Uuid, String)>, ProvisionerError> {
        Ok(EnvBranches::find()
            .filter(rows::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|r| (r.app_id, r.env_name))
            .collect())
    }

    /// Record the intent to cut, before the provider is asked: a new row, or an
    /// existing one (a failed earlier attempt) moved back to `cutting`. A crash
    /// after the provider answers but before it is recorded leaves this row, and
    /// the next attempt adopts the branch by name.
    pub(super) async fn upsert_cutting(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        mode: EnvBranchMode,
        times: CutTimes,
        existing: Option<rows::Model>,
    ) -> Result<rows::Model, ProvisionerError> {
        let now = Utc::now().fixed_offset();
        let intent = rows::ActiveModel {
            mode: ActiveValue::Set(mode),
            status: ActiveValue::Set(EnvBranchStatus::Cutting),
            error: ActiveValue::Set(None),
            fork_at: ActiveValue::Set(times.fork_at.into()),
            expires_at: ActiveValue::Set(times.expires_at.into()),
            updated_at: ActiveValue::Set(now),
            ..Default::default()
        };
        if let Some(row) = existing {
            let expected = row.status;
            return self.update_env_if(intent, &row, expected).await;
        }
        let mut insert = intent;
        insert.id = ActiveValue::Set(Uuid::new_v4());
        insert.tenant_row_id = ActiveValue::Set(tenant.id);
        insert.app_id = ActiveValue::Set(key.app_id);
        insert.env_name = ActiveValue::Set(key.env_name.clone());
        insert.provider_branch_id = ActiveValue::Set(None);
        insert.parent_branch_id = ActiveValue::Set(tenant.branch_id.clone());
        insert.created_at = ActiveValue::Set(now);
        Ok(insert.insert(&self.db).await?)
    }

    /// Record the provider's answer onto `row`, which must still be in
    /// `expected` (`cutting`, or `resetting` for a re-cut). A re-cut also
    /// stamps `last_reset_at`, and `mode` is the mode the branch was cut in
    /// (a reset may have changed it).
    pub(super) async fn record_env_cut(
        &self,
        row: rows::Model,
        cut: &ProjectBranch,
        expected: EnvBranchStatus,
        times: CutTimes,
        mode: EnvBranchMode,
    ) -> Result<rows::Model, ProvisionerError> {
        let now = Utc::now().fixed_offset();
        let sealed = match &cut.owner_role.password {
            Some(pw) => Some(seal(pw)?),
            None => None,
        };
        let changes = rows::ActiveModel {
            mode: ActiveValue::Set(mode),
            provider_branch_id: ActiveValue::Set(Some(cut.id.clone())),
            parent_branch_id: ActiveValue::Set(cut.parent_id.clone()),
            host: ActiveValue::Set(Some(cut.host.clone())),
            database_name: ActiveValue::Set(Some(cut.database.name.clone())),
            owner_role: ActiveValue::Set(Some(cut.owner_role.name.clone())),
            owner_password_ciphertext: ActiveValue::Set(sealed),
            fork_at: ActiveValue::Set(times.fork_at.into()),
            expires_at: ActiveValue::Set(times.expires_at.into()),
            last_reset_at: if expected == EnvBranchStatus::Resetting {
                ActiveValue::Set(Some(now))
            } else {
                ActiveValue::NotSet
            },
            updated_at: ActiveValue::Set(now),
            ..Default::default()
        };
        self.update_env_if(changes, &row, expected).await
    }

    /// Move `row` to `to` if it is still in the status `row` was read with.
    pub(super) async fn set_env_status(
        &self,
        row: rows::Model,
        to: EnvBranchStatus,
    ) -> Result<rows::Model, ProvisionerError> {
        let expected = row.status;
        let changes = rows::ActiveModel {
            status: ActiveValue::Set(to),
            updated_at: ActiveValue::Set(Utc::now().into()),
            ..Default::default()
        };
        self.update_env_if(changes, &row, expected).await
    }

    /// Forget the provider branch `row` recorded — deleted by the caller — so
    /// nothing reads its id, endpoint or owner again. Compare-and-set on the
    /// status `row` was read with.
    pub(super) async fn forget_env_cut(
        &self,
        row: rows::Model,
    ) -> Result<rows::Model, ProvisionerError> {
        let expected = row.status;
        let changes = rows::ActiveModel {
            provider_branch_id: ActiveValue::Set(None),
            host: ActiveValue::Set(None),
            database_name: ActiveValue::Set(None),
            owner_role: ActiveValue::Set(None),
            owner_password_ciphertext: ActiveValue::Set(None),
            updated_at: ActiveValue::Set(Utc::now().into()),
            ..Default::default()
        };
        self.update_env_if(changes, &row, expected).await
    }

    /// Forget every sealed role password of the row: its roles are re-minted.
    pub(super) async fn clear_env_roles(&self, row_id: Uuid) -> Result<(), ProvisionerError> {
        EnvBranchRoles::delete_many()
            .filter(env_branch_roles::Column::EnvBranchRowId.eq(row_id))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// `UPDATE … WHERE id = $id AND status = $expected`, or a typed refusal
    /// naming what the row turned out to be.
    async fn update_env_if(
        &self,
        changes: rows::ActiveModel,
        row: &rows::Model,
        expected: EnvBranchStatus,
    ) -> Result<rows::Model, ProvisionerError> {
        let result = EnvBranches::update_many()
            .set(changes)
            .filter(rows::Column::Id.eq(row.id))
            .filter(rows::Column::Status.eq(expected))
            .exec(&self.db)
            .await?;
        let current = EnvBranches::find_by_id(row.id).one(&self.db).await?;
        match (result.rows_affected, current) {
            (1, Some(updated)) => Ok(updated),
            (_, current) => Err(ProvisionerError::EnvBranchStateChanged {
                env_name: row.env_name.clone(),
                expected: expected.as_str(),
                found: current.map_or("deleted", |r| r.status.as_str()),
            }),
        }
    }
}
