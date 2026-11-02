//! Removing the Airhouse tenants of workspaces that are deleted.
//!
//! Every workspace a cloud deployment creates gets a tenant, so every delete
//! path has to remove one, or the tenant — its catalog, its files in S3 and a
//! live service account — outlives the only row that named it. Each path calls
//! [`tenants_of`] before it deletes, and [`record`] in the delete's own
//! transaction: the cascade takes the `airhouse_tenants` row, and
//! `airhouse_tenant_deletions` keeps what is needed to remove the tenant. The
//! two commit together or not at all.
//!
//! [`sweep`] runs on the worker fleet and does the removal
//! ([`TenantProvisioner::delete_detached`]). It is not done in the request: it
//! waits for the tenant's lock, up to two minutes behind a provision still
//! running on a just-created workspace, and calls Airhouse. It is not a queued
//! run either: the run driver works under a live workspace's context and
//! retires every run whose workspace is gone, which here is all of them.
//!
//! The sweep only ever removes tenants this deployment recorded. Dev and
//! staging share one Airhouse, so a sweep that removed every tenant with no
//! workspace in this database would delete the other deployment's.

use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use tracing::{error, info};
use uuid::Uuid;

use crate::provisioner::{DetachedTenant, TenantProvisioner};

/// How long a claimed removal is left to the sweeper that claimed it before
/// another may take it: longer than the lock wait (120s) and the Airhouse calls
/// (25s), so two sweepers never remove one tenant at once in the usual case —
/// and removing one twice is harmless.
const CLAIM_LEASE_SECS: i64 = 600;

/// How many removals one sweep takes.
const SWEEP_BATCH: i64 = 10;

/// How often a worker sweeps.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// The tenants of `workspace_ids`, read before the workspaces are deleted.
/// Empty when Airhouse is not configured on this deployment. A failed read is
/// logged, not returned, like the rest of a delete's cleanup.
pub async fn tenants_of(db: &DatabaseConnection, workspace_ids: &[Uuid]) -> Vec<DetachedTenant> {
    let Some(provisioner) = crate::provisioner_for(db.clone()) else {
        return Vec::new();
    };
    let mut tenants = Vec::new();
    for &workspace_id in workspace_ids {
        match provisioner.detached_tenant(workspace_id).await {
            Ok(Some(tenant)) => tenants.push(tenant),
            Ok(None) => {}
            Err(e) => error!(
                %workspace_id,
                "could not read the workspace's airhouse tenant before deleting it; \
                 if it has one, the tenant will be left behind in Airhouse: {e}"
            ),
        }
    }
    tenants
}

/// Record `tenants` for removal, inside the transaction that deletes their
/// workspaces.
pub async fn record(txn: &impl ConnectionTrait, tenants: &[DetachedTenant]) -> Result<(), DbErr> {
    for tenant in tenants {
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO airhouse_tenant_deletions \
                 (airhouse_tenant_id, workspace_id, service_account_id) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (airhouse_tenant_id) DO UPDATE SET \
                 service_account_id = COALESCE(EXCLUDED.service_account_id, \
                     airhouse_tenant_deletions.service_account_id), \
                 next_attempt_at = now()",
            [
                tenant.airhouse_tenant_id.clone().into(),
                tenant.workspace_id.into(),
                tenant.service_account_id.clone().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// Remove the recorded tenants that are due, and return how many were removed.
/// Several workers sweep at once: each claims its rows with a lease, so a
/// tenant is removed by one of them.
pub async fn sweep(db: &DatabaseConnection) -> usize {
    match crate::provisioner_for(db.clone()) {
        Some(provisioner) => sweep_with(db, &provisioner).await,
        None => 0,
    }
}

/// [`sweep`], removing through `provisioner`.
pub async fn sweep_with(db: &DatabaseConnection, provisioner: &TenantProvisioner) -> usize {
    let claimed = match claim(db).await {
        Ok(claimed) => claimed,
        Err(e) => {
            error!("could not claim airhouse tenant removals: {e}");
            return 0;
        }
    };
    let mut removed = 0;
    for (tenant, attempts) in claimed {
        if remove(db, provisioner, &tenant, attempts).await {
            removed += 1;
        }
    }
    removed
}

/// Claim up to [`SWEEP_BATCH`] due removals by moving them [`CLAIM_LEASE_SECS`]
/// into the future, and return each with its attempt count.
async fn claim(db: &DatabaseConnection) -> Result<Vec<(DetachedTenant, i32)>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE airhouse_tenant_deletions SET \
                 attempts = attempts + 1, \
                 next_attempt_at = now() + make_interval(secs => $1) \
             WHERE airhouse_tenant_id IN ( \
                 SELECT airhouse_tenant_id FROM airhouse_tenant_deletions \
                 WHERE next_attempt_at <= now() \
                 ORDER BY next_attempt_at \
                 LIMIT $2 \
                 FOR UPDATE SKIP LOCKED) \
             RETURNING airhouse_tenant_id, workspace_id, service_account_id, attempts",
            [(CLAIM_LEASE_SECS as f64).into(), SWEEP_BATCH.into()],
        ))
        .await?;
    rows.iter()
        .map(|row| {
            Ok((
                DetachedTenant {
                    airhouse_tenant_id: row.try_get("", "airhouse_tenant_id")?,
                    workspace_id: row.try_get("", "workspace_id")?,
                    service_account_id: row.try_get("", "service_account_id")?,
                },
                row.try_get("", "attempts")?,
            ))
        })
        .collect()
}

/// Remove one claimed tenant. On success its row goes; on failure the row is
/// pushed back by [`retry_delay`] with the error, which the log line repeats so
/// a removal that keeps failing is visible.
async fn remove(
    db: &DatabaseConnection,
    provisioner: &TenantProvisioner,
    tenant: &DetachedTenant,
    attempts: i32,
) -> bool {
    let result = provisioner.delete_detached(tenant).await;
    let settled = match &result {
        Ok(()) => {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM airhouse_tenant_deletions WHERE airhouse_tenant_id = $1",
                [tenant.airhouse_tenant_id.clone().into()],
            ))
            .await
        }
        Err(e) => {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE airhouse_tenant_deletions SET \
                     next_attempt_at = now() + make_interval(secs => $2), \
                     last_error = $3 \
                 WHERE airhouse_tenant_id = $1",
                [
                    tenant.airhouse_tenant_id.clone().into(),
                    (retry_delay(attempts).as_secs() as f64).into(),
                    e.to_string().into(),
                ],
            ))
            .await
        }
    };
    if let Err(e) = settled {
        error!(
            tenant_id = %tenant.airhouse_tenant_id,
            "could not update the airhouse tenant removal record: {e}"
        );
    }
    match result {
        Ok(()) => {
            info!(
                workspace_id = %tenant.workspace_id,
                tenant_id = %tenant.airhouse_tenant_id,
                "removed the airhouse tenant of a deleted workspace"
            );
            true
        }
        Err(e) => {
            error!(
                workspace_id = %tenant.workspace_id,
                tenant_id = %tenant.airhouse_tenant_id,
                attempts,
                "removing the airhouse tenant of a deleted workspace failed; it stays recorded \
                 in airhouse_tenant_deletions and is retried: {e}"
            );
            false
        }
    }
}

/// 30s after the first failure, doubling, at most an hour. A removal is never
/// given up: the row is the record that the tenant is still there.
pub fn retry_delay(attempts: i32) -> Duration {
    let doublings = attempts.saturating_sub(1).clamp(0, 7) as u32;
    Duration::from_secs((30u64 << doublings).min(3600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_to_an_hour() {
        assert_eq!(retry_delay(1), Duration::from_secs(30));
        assert_eq!(retry_delay(2), Duration::from_secs(60));
        assert_eq!(retry_delay(5), Duration::from_secs(480));
        assert_eq!(retry_delay(8), Duration::from_secs(3600));
        assert_eq!(retry_delay(1000), Duration::from_secs(3600));
    }
}
