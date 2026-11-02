use chrono::{DateTime, FixedOffset, Utc};
use oxy_platform::secrets::envelope;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter, Statement, TransactionTrait,
};
use thiserror::Error;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::admin::{AirhouseAdminClient, AirhouseError, TenantRecord, UserRole};
use crate::entity::Tenants as AirhouseTenants;
use crate::entity::tenants::{self as airhouse_tenants, TenantStatus};

/// Default `max_role` and `max_ttl_secs` for SAs minted on tenant
/// provisioning. The TTL matches the airhouse system cap
/// (`SYSTEM_MAX_TTL_SECS = 86400`); anything longer is rejected with 400 by
/// the Admin API. Role is `admin` so the broker can mint any role
/// (Owner→admin, Admin→writer, Member→reader) under it without re-rotating.
const SA_MAX_ROLE: UserRole = UserRole::Admin;
const SA_MAX_TTL_SECS: i32 = 24 * 60 * 60;

#[derive(Debug, Error)]
pub enum ProvisionerError {
    #[error("workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    #[error(
        "invalid tenant name {0:?}: must be 1-63 lowercase alphanumeric/hyphen/underscore chars, starting with a letter"
    )]
    InvalidTenantName(String),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("airhouse error: {0}")]
    Airhouse(#[from] AirhouseError),
    #[error("envelope crypto failed: {0}")]
    Crypto(String),
    #[error("airhouse tenant for workspace {0} has no service account; provision it first")]
    TenantHasNoServiceAccount(Uuid),
    #[error("airhouse tenant name {0:?} is already taken; pick a different name")]
    TenantNameTaken(String),
    #[error("Airhouse did not answer within {0}s")]
    AirhouseTimeout(u64),
}

/// How long removing a deleted workspace's tenant may spend on the revokes and
/// the tenant delete, once it holds the tenant's lock. The admin client sets no
/// request timeout; these take well under a second.
const DETACHED_DELETE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// How long the search for service accounts minted after the tenant was read
/// may take. It lists every account on the deployment and only adds to what is
/// revoked, so a slow list is given up rather than let it hold up the delete.
const DETACHED_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Result of a successful [`TenantProvisioner::rotate_service_account`]
/// call. The new SA is already in `airhouse_tenants`; the old one was
/// revoked airhouse-side as part of the rotation.
#[derive(Debug, Clone)]
pub struct RotatedServiceAccount {
    pub workspace_id: Uuid,
    pub old_sa_id: String,
    pub new_sa_id: String,
    pub rotated_at: DateTime<FixedOffset>,
}

/// Deterministic Airhouse SA name for a given tenant. Letting the name be
/// derivable means we can find and revoke an orphan SA from a previous
/// failed provision (its bearer is unrecoverable, so re-mint is the only
/// option).
pub fn service_account_name_for(tenant_id: &str) -> String {
    format!("oxy-tenant-{tenant_id}")
}

/// What a workspace's tenant is in Airhouse, kept past the workspace's delete.
///
/// Deleting a workspace cascades its `airhouse_tenants` row away, and with it
/// the only record of which Airhouse tenant and service account were its. So a
/// workspace delete reads this first
/// ([`detached_tenant`](TenantProvisioner::detached_tenant)), deletes the
/// workspace and records the tenant in the same transaction; a worker then
/// removes it ([`delete_detached`](TenantProvisioner::delete_detached)), see
/// `crate::workspace_delete`. Removing it first would leave a workspace whose
/// delete then failed with its data gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedTenant {
    pub workspace_id: Uuid,
    pub airhouse_tenant_id: String,
    pub service_account_id: Option<String>,
}

impl From<&airhouse_tenants::Model> for DetachedTenant {
    fn from(row: &airhouse_tenants::Model) -> Self {
        Self {
            workspace_id: row.workspace_id,
            airhouse_tenant_id: row.airhouse_tenant_id.clone(),
            service_account_id: row.service_account_id.clone(),
        }
    }
}

pub struct TenantProvisioner {
    db: DatabaseConnection,
    client: AirhouseAdminClient,
}

impl TenantProvisioner {
    pub fn new(db: DatabaseConnection, client: AirhouseAdminClient) -> Self {
        Self { db, client }
    }

    /// Idempotent: re-running with the same `workspace_id` returns the existing
    /// tenant without contacting Airhouse again. Also ensures a service
    /// account is provisioned for the tenant — used by the broker to mint
    /// per-user ephemeral credentials. SA provisioning is lazy so rows that
    /// predate the SA migration pick one up on the next call.
    #[instrument(skip(self), fields(workspace_id = %workspace_id, tenant_name = %tenant_name))]
    pub async fn provision(
        &self,
        workspace_id: Uuid,
        tenant_name: String,
    ) -> Result<TenantRecord, ProvisionerError> {
        validate_tenant_name(&tenant_name)?;
        let lock = TenantLock::acquire(&self.db, workspace_id).await?;
        let result = self
            .provision_holding_lock(workspace_id, &tenant_name)
            .await;
        lock.release().await;
        result
    }

    /// [`provision`](Self::provision), with this workspace's [`TenantLock`] held.
    async fn provision_holding_lock(
        &self,
        workspace_id: Uuid,
        tenant_name: &str,
    ) -> Result<TenantRecord, ProvisionerError> {
        info!(tenant_name = %tenant_name, "provisioning airhouse tenant for workspace");

        let existing_local = AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&self.db)
            .await?;

        let remote = match existing_local {
            Some(local) => self.reconcile_existing(&local).await?,
            None => self.create_or_adopt(workspace_id, tenant_name).await?,
        };

        self.ensure_service_account_for_workspace(workspace_id)
            .await?;

        // Fire any registered post-provision hooks (camera-fleet DDL,
        // future per-tenant setup). Hook failures are logged but don't
        // fail the provision — the affected domain has its own lazy
        // fallback (see `oxy_cameras::airhouse::connect_and_ensure`).
        // Hooks run AFTER the SA exists because most of them need to
        // mint credentials against the new tenant.
        crate::post_provision::invoke_all(workspace_id).await;

        Ok(remote)
    }

    /// Idempotent: succeeds whether the local row or the remote tenant exist.
    ///
    /// Best-effort SA revocation runs first so an orphaned SA doesn't survive
    /// a tenant deletion. Failures here are logged but do not block tenant
    /// deletion — the SA's tenant_id will still resolve to the deleted
    /// tenant on Airhouse's side, but that's recoverable on next provision.
    #[instrument(skip(self), fields(workspace_id = %workspace_id))]
    pub async fn deprovision(&self, workspace_id: Uuid) -> Result<(), ProvisionerError> {
        let lock = TenantLock::acquire(&self.db, workspace_id).await?;
        let result = self.deprovision_holding_lock(workspace_id).await;
        lock.release().await;
        result
    }

    /// [`deprovision`](Self::deprovision), with this workspace's [`TenantLock`] held.
    async fn deprovision_holding_lock(&self, workspace_id: Uuid) -> Result<(), ProvisionerError> {
        let local = AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&self.db)
            .await?;
        let Some(local) = local else {
            info!("no local tenant row; deprovision is a no-op");
            return Ok(());
        };

        self.delete_remote(&DetachedTenant::from(&local)).await?;
        AirhouseTenants::delete_by_id(local.id)
            .exec(&self.db)
            .await?;
        info!(tenant_id = %local.airhouse_tenant_id, "deprovisioned airhouse tenant");
        Ok(())
    }

    /// The tenant `workspace_id` has, read so it can still be removed after the
    /// workspace is deleted. `None` when the workspace has none.
    pub async fn detached_tenant(
        &self,
        workspace_id: Uuid,
    ) -> Result<Option<DetachedTenant>, ProvisionerError> {
        Ok(AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&self.db)
            .await?
            .as_ref()
            .map(DetachedTenant::from))
    }

    /// Remove a deleted workspace's tenant from Airhouse: revoke its service
    /// accounts, then delete the tenant. There is no local row left to delete —
    /// the workspace's delete cascaded it away.
    ///
    /// Waits for the tenant's lock first, as long as a provision may hold it: a
    /// workspace deleted just after it was created can have its provision still
    /// minting the service account or running hooks. The Airhouse calls made
    /// under the lock are bounded, since the admin client sets no timeout of
    /// its own: the search for late service accounts by
    /// [`DETACHED_LIST_TIMEOUT`], the revokes and the delete by
    /// [`DETACHED_DELETE_TIMEOUT`]. Runs on a worker, from the record the
    /// delete left, so a timeout is retried rather than lost.
    #[instrument(skip(self), fields(workspace_id = %tenant.workspace_id, tenant_id = %tenant.airhouse_tenant_id))]
    pub async fn delete_detached(&self, tenant: &DetachedTenant) -> Result<(), ProvisionerError> {
        let lock = TenantLock::acquire(&self.db, tenant.workspace_id).await?;
        let late_accounts = self.late_service_accounts(tenant).await;
        let result = match tokio::time::timeout(DETACHED_DELETE_TIMEOUT, async {
            for sa_id in &late_accounts {
                if let Err(e) = self.client.revoke_service_account(sa_id).await {
                    warn!(sa_id = %sa_id, "failed to revoke airhouse SA during deprovision: {e}");
                }
            }
            self.delete_remote(tenant).await
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ProvisionerError::AirhouseTimeout(
                DETACHED_DELETE_TIMEOUT.as_secs(),
            )),
        };
        lock.release().await;
        result?;
        info!("deleted the airhouse tenant of a deleted workspace");
        Ok(())
    }

    /// The tenant's live service accounts other than the one read with it: a
    /// provision that held the lock when the tenant was read may have minted
    /// one since, and with the row gone nothing else records it. Empty when the
    /// list fails or is slow — it only adds to what is revoked.
    async fn late_service_accounts(&self, tenant: &DetachedTenant) -> Vec<String> {
        match tokio::time::timeout(DETACHED_LIST_TIMEOUT, self.client.list_service_accounts()).await
        {
            Ok(Ok(accounts)) => accounts
                .into_iter()
                .filter(|account| {
                    account.tenant_id == tenant.airhouse_tenant_id
                        && account.revoked_at.is_none()
                        && tenant.service_account_id.as_deref() != Some(account.id.as_str())
                })
                .map(|account| account.id)
                .collect(),
            Ok(Err(e)) => {
                warn!(
                    "could not list airhouse service accounts; revoking only the one on record: {e}"
                );
                Vec::new()
            }
            Err(_) => {
                warn!(
                    "listing airhouse service accounts took over {}s; revoking only the one on record",
                    DETACHED_LIST_TIMEOUT.as_secs()
                );
                Vec::new()
            }
        }
    }

    /// Revoke the tenant's service account (logged, not fatal: the tenant's
    /// delete makes it useless), then delete the tenant. Airhouse answers 204
    /// whether or not the tenant still exists.
    async fn delete_remote(&self, tenant: &DetachedTenant) -> Result<(), ProvisionerError> {
        if let Some(sa_id) = &tenant.service_account_id
            && let Err(e) = self.client.revoke_service_account(sa_id).await
        {
            warn!(sa_id = %sa_id, "failed to revoke airhouse SA during deprovision: {e}");
        }
        self.client
            .delete_tenant(&tenant.airhouse_tenant_id)
            .await?;
        Ok(())
    }

    async fn reconcile_existing(
        &self,
        local: &airhouse_tenants::Model,
    ) -> Result<TenantRecord, ProvisionerError> {
        match self.client.get_tenant(&local.airhouse_tenant_id).await? {
            Some(remote) => {
                if local.status != TenantStatus::Active {
                    self.set_status(local.id, TenantStatus::Active).await?;
                }
                Ok(remote)
            }
            None => {
                warn!(
                    tenant_id = %local.airhouse_tenant_id,
                    "remote tenant missing; recreating to match local row"
                );
                let remote = self.client.create_tenant(&local.airhouse_tenant_id).await?;
                self.set_status(local.id, TenantStatus::Active).await?;
                Ok(remote)
            }
        }
    }

    async fn create_or_adopt(
        &self,
        workspace_id: Uuid,
        tenant_name: &str,
    ) -> Result<TenantRecord, ProvisionerError> {
        let create_result = self.client.create_tenant(tenant_name).await;

        match create_result {
            Ok(remote) => {
                self.insert_local_row(workspace_id, &remote, TenantStatus::Active)
                    .await?;
                Ok(remote)
            }
            Err(AirhouseError::AlreadyExists(msg)) => {
                // Refuse to silently adopt a tenant that already exists on
                // the airhouse side — that path could grant this workspace
                // access to another workspace's data if two users picked
                // the same name. Surface a typed error so the REST handler
                // can return 409 and the UI can ask the user for a fresh
                // name. Operators can recover the rare "this workspace's
                // own remote tenant exists but the local row was wiped"
                // case via the runbook (delete the remote tenant, then
                // re-provision under the same name).
                //
                // Critically we do NOT write a Failed local row for the
                // collided name. If we did, the next `provision()` call
                // would hit `reconcile_existing` first, see the Failed row
                // pointing at the foreign tenant id, and adopt it —
                // exactly the cross-workspace data leak this branch is
                // designed to prevent. `ensure_service_account_for_workspace`
                // would then revoke the legitimate SA on the other
                // workspace's tenant.
                warn!(
                    tenant_name,
                    msg, "airhouse rejected create with 409 — name already taken"
                );
                Err(ProvisionerError::TenantNameTaken(tenant_name.to_string()))
            }
            Err(e) => {
                let _ = self
                    .insert_failed_local_row(workspace_id, tenant_name)
                    .await;
                Err(e.into())
            }
        }
    }

    async fn insert_local_row(
        &self,
        workspace_id: Uuid,
        remote: &TenantRecord,
        status: TenantStatus,
    ) -> Result<(), ProvisionerError> {
        let row = airhouse_tenants::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            workspace_id: ActiveValue::Set(workspace_id),
            airhouse_tenant_id: ActiveValue::Set(remote.id.clone()),
            bucket: ActiveValue::Set(remote.bucket.clone()),
            prefix: ActiveValue::Set(remote.prefix.clone()),
            status: ActiveValue::Set(status),
            created_at: ActiveValue::Set(Utc::now().fixed_offset()),
            // SA fields are populated by ensure_service_account_for_workspace.
            ..Default::default()
        };
        row.insert(&self.db).await?;
        Ok(())
    }

    /// Write a placeholder row when remote provisioning fails before we ever
    /// got a `TenantRecord` back — bucket/prefix are unknown until the server
    /// resolves them, so we leave them empty/null until reconciliation runs.
    async fn insert_failed_local_row(
        &self,
        workspace_id: Uuid,
        tenant_name: &str,
    ) -> Result<(), sea_orm::DbErr> {
        let row = airhouse_tenants::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            workspace_id: ActiveValue::Set(workspace_id),
            airhouse_tenant_id: ActiveValue::Set(tenant_name.to_string()),
            bucket: ActiveValue::Set(String::new()),
            prefix: ActiveValue::Set(None),
            status: ActiveValue::Set(TenantStatus::Failed),
            created_at: ActiveValue::Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        row.insert(&self.db).await.map(|_| ())
    }

    async fn set_status(&self, id: Uuid, status: TenantStatus) -> Result<(), sea_orm::DbErr> {
        let row = airhouse_tenants::ActiveModel {
            id: ActiveValue::Set(id),
            status: ActiveValue::Set(status),
            ..Default::default()
        };
        row.update(&self.db).await.map(|_| ())
    }

    /// Ensure the tenant for `workspace_id` has a usable SA. Idempotent.
    ///
    /// "Usable" means the local row carries both `service_account_id` and
    /// `bearer_ciphertext`. If either is missing — either because this row
    /// predates the SA migration or because a previous provision crashed
    /// between SA mint and DB persist — we mint a fresh SA. Before minting
    /// we list remote SAs by deterministic name and revoke any orphan: its
    /// bearer is unrecoverable (Airhouse only stores the hash) so reuse is
    /// impossible.
    #[instrument(skip(self), fields(workspace_id = %workspace_id))]
    async fn ensure_service_account_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<(), ProvisionerError> {
        let local = AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&self.db)
            .await?
            .ok_or(ProvisionerError::WorkspaceNotFound(workspace_id))?;

        if local.service_account_id.is_some() && local.bearer_ciphertext.is_some() {
            return Ok(());
        }

        let sa_name = service_account_name_for(&local.airhouse_tenant_id);

        // Adoption-by-revocation: a previous failed provision may have left
        // an SA on the remote side. We can't recover its bearer, so revoke
        // before minting fresh.
        //
        // TODO: this scans every SA in the airhouse deployment. Should be
        // server-side filtered by tenant once airhouse exposes
        // `GET /tenants/{tid}/service-accounts` or
        // `GET /service-accounts?name=...`. The unfiltered list also lets
        // a separate deployment that picked the same SA name appear here
        // (we filter by exact name match before revoking, so we wouldn't
        // delete the wrong row, but the scan grows linearly with the
        // global SA count).
        let sas = self.client.list_service_accounts().await?;
        if let Some(orphan) = sas
            .into_iter()
            .find(|sa| sa.name == sa_name && sa.revoked_at.is_none())
        {
            warn!(
                sa_id = %orphan.id,
                sa_name = %sa_name,
                "found orphan airhouse SA from previous provision; revoking before re-minting"
            );
            self.client.revoke_service_account(&orphan.id).await?;
        }

        let created = self
            .client
            .create_service_account(
                &sa_name,
                &local.airhouse_tenant_id,
                SA_MAX_ROLE,
                SA_MAX_TTL_SECS,
            )
            .await?;
        let sa_id = created.record.id.clone();
        let max_role = created.record.max_role.clone();
        let max_ttl_secs = created.record.max_ttl_secs;

        let bearer_ciphertext = envelope::seal(created.bearer.as_bytes())
            .map_err(|e| ProvisionerError::Crypto(e.to_string()))?;

        let now = Utc::now().fixed_offset();
        let mut active: airhouse_tenants::ActiveModel = local.into();
        active.service_account_id = ActiveValue::Set(Some(sa_id.clone()));
        active.bearer_ciphertext = ActiveValue::Set(Some(bearer_ciphertext));
        active.bearer_max_role = ActiveValue::Set(Some(max_role));
        active.bearer_max_ttl_secs = ActiveValue::Set(Some(max_ttl_secs));
        active.sa_created_at = ActiveValue::Set(Some(now));
        active.update(&self.db).await?;

        info!(workspace_id = %workspace_id, sa_id = %sa_id, "provisioned airhouse SA");
        Ok(())
    }

    /// Rotate the service account for `workspace_id`. Used as the
    /// bearer-leak response — call this when an SA bearer is suspected
    /// compromised.
    ///
    /// Flow:
    /// 1. Revoke the old SA airhouse-side. New mints with the old bearer
    ///    immediately fail with 401; the broker then loads the new bearer
    ///    on its next cache-miss mint.
    /// 2. Create a fresh SA under the same deterministic name.
    /// 3. Atomic-ish DB swap: update `service_account_id`,
    ///    `bearer_ciphertext`, `sa_rotated_at` on the tenants row.
    ///
    /// Outstanding ephemerals already minted under the old SA continue to
    /// authenticate via SCRAM (their `cp_users` rows aren't touched);
    /// they expire on their own. Fresh mints route through the new SA
    /// once step 3 commits.
    ///
    /// Multi-replica caveat: in-memory broker caches in other replicas
    /// hold the old bearer until the cached credential expires (24h max).
    /// A mint attempted by another replica between steps 1 and 3 will
    /// fail with 401 — small operational hiccup, not a security gap.
    #[instrument(skip(self), fields(workspace_id = %workspace_id))]
    pub async fn rotate_service_account(
        &self,
        workspace_id: Uuid,
    ) -> Result<RotatedServiceAccount, ProvisionerError> {
        let local = AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&self.db)
            .await?
            .ok_or(ProvisionerError::WorkspaceNotFound(workspace_id))?;
        let old_sa_id = local
            .service_account_id
            .clone()
            .ok_or(ProvisionerError::TenantHasNoServiceAccount(workspace_id))?;

        info!(workspace_id = %workspace_id, old_sa_id = %old_sa_id, "rotating airhouse SA");

        // Step 1: revoke old. Idempotent on the airhouse side — a 204 is
        // returned even when the SA was already revoked.
        self.client.revoke_service_account(&old_sa_id).await?;

        // Step 2: create new SA under the same deterministic name. The
        // airhouse-side `service_accounts` table doesn't unique on name,
        // and the previous SA is now revoked (filtered out by
        // `find_active_service_account_by_bearer`), so reusing the name
        // is safe.
        let sa_name = service_account_name_for(&local.airhouse_tenant_id);
        let created = self
            .client
            .create_service_account(
                &sa_name,
                &local.airhouse_tenant_id,
                SA_MAX_ROLE,
                SA_MAX_TTL_SECS,
            )
            .await?;
        let new_sa_id = created.record.id.clone();

        let bearer_ciphertext = envelope::seal(created.bearer.as_bytes())
            .map_err(|e| ProvisionerError::Crypto(e.to_string()))?;
        let now = Utc::now().fixed_offset();

        // Step 3: persist.
        let mut active: airhouse_tenants::ActiveModel = local.into();
        active.service_account_id = ActiveValue::Set(Some(new_sa_id.clone()));
        active.bearer_ciphertext = ActiveValue::Set(Some(bearer_ciphertext));
        active.sa_rotated_at = ActiveValue::Set(Some(now));
        active.update(&self.db).await?;

        info!(
            workspace_id = %workspace_id,
            old_sa_id = %old_sa_id,
            new_sa_id = %new_sa_id,
            "rotated airhouse SA"
        );

        Ok(RotatedServiceAccount {
            workspace_id,
            old_sa_id,
            new_sa_id,
            rotated_at: now,
        })
    }
}

/// Tenant names must be valid PostgreSQL role identifiers:
/// - 1–63 chars
/// - first char is a lowercase ASCII letter
/// - remaining chars are lowercase ASCII alnum, `-`, or `_`
///
/// Equivalent regex (kept here for reference only; not evaluated):
/// `^[a-z][a-z0-9_-]{0,62}$`
/// The Airhouse-side tenant name for a workspace: derived from its id, which
/// is unique and already the key, so no person ever has to choose one.
pub fn tenant_name_for_workspace(workspace_id: Uuid) -> String {
    format!("oxy-ws-{workspace_id}")
}

fn validate_tenant_name(name: &str) -> Result<(), ProvisionerError> {
    if name.is_empty() || name.len() > 63 {
        return Err(ProvisionerError::InvalidTenantName(name.to_string()));
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err(ProvisionerError::InvalidTenantName(name.to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(ProvisionerError::InvalidTenantName(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_tenant_names() {
        for name in ["acme", "acme-corp", "my_tenant", "a123", "a1b2c3"] {
            assert!(validate_tenant_name(name).is_ok(), "expected ok for {name}");
        }
    }

    #[test]
    fn invalid_tenant_names() {
        for name in [
            "",
            "1starts-digit",
            "-starts-hyphen",
            "_starts_underscore",
            &"a".repeat(64),
        ] {
            assert!(
                validate_tenant_name(name).is_err(),
                "expected err for {name:?}"
            );
        }
    }

    #[test]
    fn single_char_tenant_name() {
        assert!(validate_tenant_name("a").is_ok());
    }

    #[test]
    fn max_length_tenant_name() {
        let name = format!("a{}", "x".repeat(62));
        assert_eq!(name.len(), 63);
        assert!(validate_tenant_name(&name).is_ok());
    }
}

/// One provision or deprovision of a workspace's tenant at a time, on any pod.
///
/// `provision` saves the tenant row and then mints the service account. Two
/// callers in that window (workspace creation's background provision and the
/// demo's own call, two uploads, a staff re-provision) each see a row with no
/// service account; the second would revoke the first's account as an orphan
/// and mint its own, and whichever saved its bearer last could leave a revoked
/// one behind. Under this lock the second caller starts after the first has
/// finished, finds the tenant and its service account, and changes nothing.
///
/// A Postgres transaction-scoped advisory lock on a pooled connection of its
/// own, keyed by the workspace; ending the transaction or dropping the
/// connection releases it. Another backend (a local SQLite test database) has
/// no other process to race with, so it takes none.
struct TenantLock(Option<DatabaseTransaction>);

/// How long a provision waits for another of the same workspace. A provision
/// is a few Airhouse calls plus the post-provision hooks.
const TENANT_LOCK_TIMEOUT: &str = "SET LOCAL lock_timeout = '120s'";

impl TenantLock {
    async fn acquire(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Self, sea_orm::DbErr> {
        if db.get_database_backend() != DatabaseBackend::Postgres {
            return Ok(Self(None));
        }
        let txn = db.begin().await?;
        txn.execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            TENANT_LOCK_TIMEOUT,
        ))
        .await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [tenant_lock_key(workspace_id).into()],
        ))
        .await?;
        Ok(Self(Some(txn)))
    }

    /// Let the lock go. A failed rollback is moot: the connection closing
    /// ends the transaction too.
    async fn release(self) {
        if let Some(txn) = self.0 {
            let _ = txn.rollback().await;
        }
    }
}

/// The advisory-lock key of `workspace_id`'s tenant: the workspace id's first
/// eight bytes, mixed with a constant so it does not collide with the plain
/// keys other locks in the database use. A collision would only serialize two
/// provisions, never let two run together.
fn tenant_lock_key(workspace_id: Uuid) -> i64 {
    const NAMESPACE: u64 = u64::from_be_bytes(*b"airhouse");
    let bytes = workspace_id.as_bytes();
    let head = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    (head ^ NAMESPACE) as i64
}

#[cfg(test)]
mod tenant_lock_tests {
    use super::*;

    #[test]
    fn each_workspace_has_its_own_stable_key() {
        let a = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let b = Uuid::from_u128(0x1112_1314_1516_1718_191a_1b1c_1d1e_1f20);
        assert_eq!(tenant_lock_key(a), tenant_lock_key(a));
        assert_ne!(tenant_lock_key(a), tenant_lock_key(b));
    }
}
