//! The org's cap on environment branches, enforced where a branch is made.
//!
//! The request-time check (`PlaneSpec::validate`) counts rows, and a cut is
//! queued work: several creates, planes writes and resets can each see room
//! for the last branch before any of their cuts has run. So the cut asks
//! again, here, under a lock that covers the tenant's whole set of
//! environment branches: it counts the **other** rows, refuses at the limit,
//! and records its own intent row before the lock is released — the next cut's
//! count sees it. The provider is not called under this lock.

use std::future::Future;

use sea_orm::{ColumnTrait, Condition, EntityTrait, PaginatorTrait, QueryFilter};

use crate::entity::env_branches::{self as rows, Entity as EnvBranches};
use crate::entity::tenants as oltp_tenants;
use crate::env_branches::{EnvBranchKey, cap_refusal, max_per_org};

use super::{OltpProvisioner, ProvisionerError};

/// Whether `others` branches leave no room for one more under `max`.
pub(super) fn at_cap(others: u64, max: u64) -> bool {
    others >= max
}

impl OltpProvisioner {
    /// Run `record` — the write of the cut's intent row — under the tenant's
    /// cap lock, after checking that the org's other environment branches leave
    /// room for it. The environment's own row, if a failed attempt left one, is
    /// not counted against it.
    pub(super) async fn record_within_cap<T>(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        record: impl Future<Output = Result<T, ProvisionerError>>,
    ) -> Result<T, ProvisionerError> {
        let lock = self.lock_env_cap(tenant.id).await?;
        let out = self.check_then_record(tenant, key, record).await;
        lock.release().await;
        out
    }

    async fn check_then_record<T>(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        record: impl Future<Output = Result<T, ProvisionerError>>,
    ) -> Result<T, ProvisionerError> {
        let others = EnvBranches::find()
            .filter(rows::Column::TenantRowId.eq(tenant.id))
            .filter(
                Condition::any()
                    .add(rows::Column::AppId.ne(key.app_id))
                    .add(rows::Column::EnvName.ne(key.env_name.as_str())),
            )
            .count(&self.db)
            .await?;
        let max = max_per_org();
        if at_cap(others, max) {
            return Err(ProvisionerError::EnvBranchCap(cap_refusal(others, max)));
        }
        record.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_is_refused_when_the_others_fill_the_cap() {
        assert!(!at_cap(0, 1));
        assert!(
            !at_cap(1, 2),
            "one below the limit leaves room for this one"
        );
        assert!(at_cap(2, 2));
        assert!(
            at_cap(9, 8),
            "a lowered limit refuses what it can no longer hold"
        );
        assert!(at_cap(0, 0), "a limit of zero holds none");
    }
}
