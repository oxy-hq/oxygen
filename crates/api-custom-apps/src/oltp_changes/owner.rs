//! The org's OLTP store as the change stream reaches it: brought current to
//! the latest platform step, then opened as its owner, the only role that
//! reads and writes `oxy_changes`. Shared by the install at a promote
//! (`install`) and the relay (`relay`), so both find and open a store the
//! same way.

use oxy_oltp::entity::tenants::{self as oltp_tenants, TenantStatus};
use oxy_oltp::migrator::{MigrateError, owner_dsn, tenant_for_org};
use oxy_oltp::provisioner::reconcile_platform_schema_as_owner;
use oxy_oltp::sql::PgSqlExecutor;
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

/// The platform step that creates `oxy_changes`. A store below it has
/// captured nothing, so a caller that requires nothing need not bring it
/// current.
pub(crate) const CHANGES_STEP: i32 = 8;

/// The org's store, brought current so `oxy_changes` exists. `None` when
/// nothing is `required` and the store cannot hold any capture: none, not
/// active, or below the step that creates `oxy_changes` ([`CHANGES_STEP`]).
/// `Err` says what was being done and why it failed.
pub(crate) async fn current_tenant(
    db: &DatabaseConnection,
    org_id: Uuid,
    required: bool,
) -> Result<Option<oltp_tenants::Model>, String> {
    let tenant = match tenant_for_org(db, org_id).await {
        Ok(tenant) => tenant,
        Err(MigrateError::NotProvisioned(_)) if !required => return Ok(None),
        Err(e) => return Err(format!("find the org's OLTP store: {e}")),
    };
    if tenant.status != TenantStatus::Active {
        if !required {
            return Ok(None);
        }
        return Err(format!(
            "the org's OLTP store is {}, not active",
            tenant.status.as_str()
        ));
    }
    if !required && tenant.platform_schema_version < CHANGES_STEP {
        return Ok(None);
    }
    reconcile_platform_schema_as_owner(db, &PgSqlExecutor, &tenant)
        .await
        .map(Some)
        .map_err(|e| format!("bring the org's OLTP store up to date: {e}"))
}

/// A dedicated connection to `tenant` as its owner. `label` names it in the
/// log line its connection task writes when it closes.
pub(crate) async fn owner_client(
    tenant: &oltp_tenants::Model,
    label: &'static str,
) -> Result<Client, String> {
    let doing = "open the org's OLTP store as its owner";
    let dsn = owner_dsn(tenant).map_err(|e| format!("{doing}: {e}"))?;
    oxy_oltp::connect::connect(&dsn, label)
        .await
        .map_err(|e| format!("{doing}: {}", oxy_oltp::connect::pg_detail(&e)))
}
