//! Opening an outbox's store as its owner: production's tenant database, or
//! the OLTP branch hosting a non-production environment ([`super::branch`]).
//! Each call opens one short connection.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{EventsError, OutboxTarget, store};
use crate::entity::tenants::{self as oltp_tenants, TenantStatus};
use crate::migrator::{MigrateError, owner_dsn, tenant_for_org};
use crate::provisioner::reconcile_platform_schema_as_owner;
use crate::sql::PgSqlExecutor;

/// The org's tenant row, refusing one that cannot be used.
async fn active_tenant(
    control: &DatabaseConnection,
    org_id: Uuid,
) -> Result<oltp_tenants::Model, EventsError> {
    let tenant = tenant_for_org(control, org_id).await.map_err(|e| match e {
        MigrateError::NotProvisioned(org_id) => EventsError::NotProvisioned { org_id },
        other => EventsError::Store(format!("find the org's OLTP store: {other}")),
    })?;
    // A failed or deleting provision: the operator's remedy is the same.
    if tenant.status != TenantStatus::Active {
        return Err(EventsError::NotProvisioned { org_id });
    }
    Ok(tenant)
}

/// [`active_tenant`], brought current so a tenant provisioned before the
/// outbox existed gets it on first use.
async fn current_tenant(
    control: &DatabaseConnection,
    org_id: Uuid,
) -> Result<oltp_tenants::Model, EventsError> {
    let tenant = active_tenant(control, org_id).await?;
    reconcile_platform_schema_as_owner(control, &PgSqlExecutor, &tenant)
        .await
        .map_err(|e| EventsError::Store(format!("bring the org's OLTP store up to date: {e}")))
}

/// An owner connection to `target`'s store. With `bring_current` — a
/// publish's grant and a claim, either of which may be the first to touch the
/// outbox — the store's Oxy-owned objects are brought current first; a read of
/// what is already there never does. `None`: `target` is a branch the org does
/// not have.
pub(super) async fn owner_client(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    bring_current: bool,
) -> Result<Option<tokio_postgres::Client>, EventsError> {
    let dsn = match target {
        OutboxTarget::Production => {
            let tenant = if bring_current {
                current_tenant(control, org_id).await?
            } else {
                active_tenant(control, org_id).await?
            };
            owner_dsn(&tenant)
                .map_err(|e| EventsError::Store(format!("open the org's OLTP store: {e}")))?
        }
        OutboxTarget::Branch(environment) => {
            match super::branch::owner_dsn(control, org_id, environment, bring_current).await? {
                Some(dsn) => dsn,
                None => return Ok(None),
            }
        }
    };
    crate::connect::connect(&dsn, "oxy_events")
        .await
        .map(Some)
        .map_err(|e| store("open the org's OLTP store", &e))
}

/// [`owner_client`] for a caller that writes, where a branch the org does not
/// have is [`EventsError::NoBranch`].
pub(super) async fn required_client(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    bring_current: bool,
) -> Result<tokio_postgres::Client, EventsError> {
    owner_client(control, org_id, target, bring_current)
        .await?
        .ok_or_else(|| match target {
            OutboxTarget::Branch(environment) => EventsError::NoBranch {
                org_id,
                environment: environment.clone(),
            },
            // Production's store is never `None`: a missing one is an error.
            OutboxTarget::Production => EventsError::NotProvisioned { org_id },
        })
}
