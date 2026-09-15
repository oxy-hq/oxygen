//! Service-account-backed credential broker.
//!
//! Mints short-lived per-(workspace, subject, role) ephemeral wire-protocol
//! credentials by calling the Airhouse Admin API's
//! `POST /admin/v1/tenants/{tenant}/tokens` endpoint. The bearer used to
//! authenticate the mint is the per-tenant SA bearer that
//! [`crate::TenantProvisioner`] sealed onto `airhouse_tenants` at provision
//! time.
//!
//! # Caching
//!
//! Mints are cached in process memory keyed by
//! `(workspace_id, subject, role)`. A cached entry is returned as long as
//! its `expires_at` is more than [`CACHE_REFRESH_BUFFER`] in the future; once
//! within the buffer we mint fresh so the caller doesn't get an
//! about-to-expire credential. Multi-replica oxy multiplies the mint load
//! per SA — the airhouse-side default 60/min cap is the relevant ceiling
//! and is monitored by `airhouse_mints_rate_limited_total`.
//!
//! Two known cache rough edges, tracked as TODOs in the call sites:
//! - Thundering herd: two concurrent calls for the same key both miss and
//!   both mint. Per-key single-flight is the right fix; not yet
//!   implemented.
//! - Unbounded growth: entries are removed only on explicit eviction or
//!   cache-key churn. There is no proactive sweep for expired or unused
//!   keys.
//!
//! # Auth-failure handling
//!
//! [`AirhouseTokenBroker::evict_and_remint`] exists for connectors that
//! observe a SCRAM 28P01 (e.g. because the broker cache lagged a remote
//! revoke) and want to drop the stale cache entry and mint fresh. The
//! current connectors call mint and surface auth failures verbatim — they
//! do not retry. Wiring an evict-and-remint pass into the connectors is a
//! tracked follow-up; meanwhile a freshly rotated SA briefly forces every
//! caller with a stale cache entry to fail until they reload.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::workspace_members::WorkspaceRole;
use oxy_platform::db::establish_connection;
use oxy_platform::secrets::envelope;
use oxy_shared::errors::OxyError;
use rand::RngExt;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::admin::{AirhouseAdminClient, AirhouseError, EphemeralCredential, TokenAuth, UserRole};
use crate::entity::Tenants as AirhouseTenants;
use crate::entity::tenants::{self as airhouse_tenants};

/// Refresh a cached credential when it's within this many seconds of its
/// `expires_at`. Picked so a slow caller (60s+ between checking cache and
/// opening a SCRAM session) never gets a credential that expires mid-handshake.
const CACHE_REFRESH_BUFFER: Duration = Duration::from_secs(60);

/// Default TTL for credentials minted for in-product use (SQL IDE, agentic
/// queries, `airhouse_managed`). Long enough to amortise the mint cost
/// across an interactive session, short enough that a leaked credential
/// expires before any human can reuse it.
pub const DEFAULT_INTERNAL_TTL: Duration = Duration::from_secs(900);

/// Default TTL for tokens shown to a user for paste-into-psql use. Capped
/// by the airhouse system-wide 24h ceiling.
pub const DEFAULT_EXTERNAL_TTL: Duration = Duration::from_secs(86_400);

const RATE_LIMIT_BACKOFFS: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(500),
    Duration::from_secs(2),
];

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("airhouse tenant for workspace {0} not found")]
    TenantNotFound(Uuid),
    #[error(
        "airhouse tenant for workspace {0} has no service account; \
         re-run TenantProvisioner::provision to back-fill"
    )]
    TenantHasNoServiceAccount(Uuid),
    #[error("could not decrypt SA bearer for workspace {0}: {1}")]
    Crypto(Uuid, String),
    #[error("airhouse mint rate limit exceeded after {0} retries")]
    RateLimitExceeded(usize),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("airhouse error: {0}")]
    Airhouse(#[from] AirhouseError),
}

impl From<BrokerError> for OxyError {
    fn from(value: BrokerError) -> Self {
        OxyError::DBError(value.to_string())
    }
}

/// Identity that the minted credential will be audited under.
///
/// Subjects are stored verbatim in `cp_users.subject` on the airhouse side,
/// so they need to be opaque-ish (we hand the raw oxy `users.id` UUID for
/// real users, never email). System-side operations carry a
/// `system:workspace:<uuid>:<purpose>` prefix so audit queries can segment.
#[derive(Debug, Clone)]
pub enum BrokerSubject {
    User(Uuid),
    System {
        workspace_id: Uuid,
        purpose: SystemPurpose,
    },
    /// A custom app writing its own facts, whoever invoked it. Audited as
    /// `system:workspace:<uuid>:app:<slug>`. A Writer mint for this subject
    /// asks Airhouse to confine the credential to `schema`; see
    /// [`AirhouseTokenBroker::mint_for_app`].
    App {
        workspace_id: Uuid,
        app_slug: String,
        schema: String,
    },
}

/// Reason a system-issued credential is needed. Embedded in the audit
/// subject so `purpose=scheduler` queries can be filtered apart from
/// `purpose=crawler`. New variants land here as they appear in callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPurpose {
    Scheduler,
    SchemaCrawler,
    AgenticBackground,
    /// Edge box ingest path — high-volume camera_events / *_health writes
    /// from `oxy-cameras`. Sits in the audit log as `purpose=edge-ingest`.
    EdgeIngest,
    /// Operator UI reading compliance reports for the cameras Compliance
    /// tab. Distinct from `EdgeIngest` so the audit log can filter
    /// human-driven SELECTs out of the bulk write traffic, and so the
    /// broker mints them as Reader (least privilege).
    ComplianceReportsRead,
}

impl SystemPurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            SystemPurpose::Scheduler => "scheduler",
            SystemPurpose::SchemaCrawler => "crawler",
            SystemPurpose::AgenticBackground => "agentic-bg",
            SystemPurpose::EdgeIngest => "edge-ingest",
            SystemPurpose::ComplianceReportsRead => "compliance-reports-read",
        }
    }
}

impl BrokerSubject {
    /// Render the subject string airhouse will store on the audit row.
    pub fn audit_subject(&self) -> String {
        match self {
            BrokerSubject::User(uid) => uid.to_string(),
            BrokerSubject::System {
                workspace_id,
                purpose,
            } => {
                format!("system:workspace:{workspace_id}:{}", purpose.as_str())
            }
            BrokerSubject::App {
                workspace_id,
                app_slug,
                ..
            } => format!("system:workspace:{workspace_id}:app:{app_slug}"),
        }
    }

    /// The schemas a mint for this subject asks Airhouse to confine writes to.
    /// Only an app's Writer credential is scoped: Airhouse refuses a scope on
    /// any other role, and a Reader has nothing to confine.
    fn write_schemas(&self, role: UserRole) -> Option<Vec<String>> {
        match (self, role) {
            (BrokerSubject::App { schema, .. }, UserRole::Writer) => Some(vec![schema.clone()]),
            _ => None,
        }
    }

    fn workspace_id(&self) -> Option<Uuid> {
        match self {
            BrokerSubject::User(_) => None,
            BrokerSubject::System { workspace_id, .. }
            | BrokerSubject::App { workspace_id, .. } => Some(*workspace_id),
        }
    }
}

/// Map an oxy workspace role to the airhouse role that should be used when
/// minting on behalf of that user. Mirrors the existing
/// `UserProvisioner::map_role` (Owner→admin, Admin→writer, Member→reader)
/// but operates on `WorkspaceRole` so callers downstream of
/// `EffectiveWorkspaceRole` can look it up directly.
pub fn airhouse_role_for(role: WorkspaceRole) -> UserRole {
    match role {
        WorkspaceRole::Owner => UserRole::Admin,
        WorkspaceRole::Admin => UserRole::Writer,
        WorkspaceRole::Member => UserRole::Reader,
        WorkspaceRole::Viewer => UserRole::Reader,
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    cred: EphemeralCredential,
}

impl CacheEntry {
    fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        let buffer = chrono::Duration::from_std(CACHE_REFRESH_BUFFER)
            .unwrap_or_else(|_| chrono::Duration::seconds(60));
        self.cred.expires_at > now + buffer
    }
}

type CacheKey = (Uuid, String, UserRole);
type SharedCache = Arc<RwLock<HashMap<CacheKey, CacheEntry>>>;

/// SA-backed credential broker. Single shared instance per process; obtain
/// via [`crate::token_broker`].
pub struct AirhouseTokenBroker {
    client: AirhouseAdminClient,
    cache: SharedCache,
}

impl AirhouseTokenBroker {
    pub fn new(client: AirhouseAdminClient) -> Self {
        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Mint a credential on behalf of a real oxy user. The audit subject is
    /// the user's UUID; the role is whatever the caller's role-mapping
    /// returns from [`airhouse_role_for`].
    #[instrument(skip(self), fields(workspace_id = %workspace_id, subject = %oxy_user_id))]
    pub async fn mint_for_user(
        &self,
        workspace_id: Uuid,
        oxy_user_id: Uuid,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        self.mint(workspace_id, BrokerSubject::User(oxy_user_id), role, ttl)
            .await
    }

    /// Mint a credential for a background path (scheduler, schema crawl,
    /// agentic background runs). The audit subject is
    /// `system:workspace:<uuid>:<purpose>`.
    #[instrument(skip(self), fields(workspace_id = %workspace_id, purpose = ?purpose))]
    pub async fn mint_for_system(
        &self,
        workspace_id: Uuid,
        purpose: SystemPurpose,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        self.mint(
            workspace_id,
            BrokerSubject::System {
                workspace_id,
                purpose,
            },
            role,
            ttl,
        )
        .await
    }

    /// Mint a credential for a custom app to write its own facts in `schema`.
    ///
    /// Minted for the app, not the caller: a scheduled run and a Member's click
    /// write the same way, the stance `ctx.oltp` takes. A `Writer` mint asks
    /// Airhouse for `write_schemas: [schema]`. An Airhouse that supports it
    /// echoes the scope back in [`EphemeralCredential::write_schemas`]; an older
    /// one ignores the field and returns a tenant-wide Writer, which the caller
    /// must treat as unscoped — the Oxy host's `sql_rules` check is then the
    /// only fence.
    #[instrument(skip(self), fields(workspace_id = %workspace_id, app = %app_slug))]
    pub async fn mint_for_app(
        &self,
        workspace_id: Uuid,
        app_slug: &str,
        schema: &str,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        self.mint(
            workspace_id,
            BrokerSubject::App {
                workspace_id,
                app_slug: app_slug.to_string(),
                schema: schema.to_string(),
            },
            role,
            ttl,
        )
        .await
    }

    /// Drop any cached credential for the `(workspace_id, subject, role)` key
    /// and mint fresh. Use this from a connector's auth-failure path: SCRAM
    /// returned `28P01`, evict the cache entry that produced it, mint once
    /// more, retry SCRAM exactly once.
    #[instrument(skip(self), fields(workspace_id = %workspace_id))]
    pub async fn evict_and_remint(
        &self,
        workspace_id: Uuid,
        subject: BrokerSubject,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let key = self.cache_key(workspace_id, &subject, role);
        self.cache.write().await.remove(&key);
        self.mint(workspace_id, subject, role, ttl).await
    }

    /// Drop the cached entry without minting. Useful when a workspace member
    /// is removed and we want to invalidate any in-memory credential they
    /// might still have.
    pub async fn evict(&self, workspace_id: Uuid, subject: &BrokerSubject, role: UserRole) {
        let key = self.cache_key(workspace_id, subject, role);
        self.cache.write().await.remove(&key);
    }

    /// Evict every cached credential for `oxy_user_id` in `workspace_id`,
    /// across all three airhouse roles. Sweeps the cache once rather than
    /// taking the write lock three times.
    ///
    /// Best-effort defense-in-depth for the "member was removed from the
    /// org" path: outstanding airhouse-side ephemerals will expire on
    /// their own (24h max), but evicting drops anything still warm in
    /// this process so the user can't pull the same token from a fresh
    /// tab.
    pub async fn evict_user_across_roles(&self, workspace_id: Uuid, oxy_user_id: Uuid) {
        let subject = BrokerSubject::User(oxy_user_id).audit_subject();
        let mut cache = self.cache.write().await;
        for role in [UserRole::Reader, UserRole::Writer, UserRole::Admin] {
            cache.remove(&(workspace_id, subject.clone(), role));
        }
    }

    /// Evict cached credentials AND remotely revoke them on airhouse for
    /// `oxy_user_id` in `workspace_id`. Use this on role changes / member
    /// removal where the OLD credential must stop working immediately —
    /// plain [`Self::evict_user_across_roles`] only drops in-process state
    /// and the underlying ephemeral keeps authenticating until its TTL
    /// expires (24h ceiling).
    ///
    /// Only revokes credentials we still have in the cache. Older mints
    /// that have rotated out of the cache are not enumerated; closing that
    /// gap requires either a list endpoint on airhouse or local mint-row
    /// tracking, both currently out of scope.
    ///
    /// Best-effort: revocation failures (network blip, airhouse down) are
    /// logged at WARN and do not propagate. The caller is typically a
    /// role-mutation HTTP handler that must not fail the user's request
    /// because the upstream revoke endpoint blinked.
    pub async fn revoke_user_across_roles(&self, workspace_id: Uuid, oxy_user_id: Uuid) {
        let subject = BrokerSubject::User(oxy_user_id).audit_subject();

        // Drain cache entries first (single write-lock acquisition) so
        // concurrent callers see "no credential" while we revoke.
        let mut to_revoke: Vec<String> = Vec::new();
        {
            let mut cache = self.cache.write().await;
            for role in [UserRole::Reader, UserRole::Writer, UserRole::Admin] {
                if let Some(entry) = cache.remove(&(workspace_id, subject.clone(), role)) {
                    to_revoke.push(entry.cred.username);
                }
            }
        }

        for username in to_revoke {
            match self.revoke_user_token(workspace_id, &username).await {
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        workspace_id = %workspace_id,
                        user_id = %oxy_user_id,
                        username = %username,
                        "airhouse credential revoke failed during role/member change: {e}"
                    );
                }
            }
        }
    }

    /// Revoke a single ephemeral credential remotely (the airhouse-side
    /// row in `cp_users`). Authenticated as the per-tenant SA that issued
    /// the credential — Airhouse rejects cross-SA revocations with 403.
    /// Returns `Ok(true)` on success, `Ok(false)` if the credential never
    /// existed or was already revoked.
    ///
    /// Use cases: a user clicks "Revoke this token" in the UI after they
    /// realised they pasted it somewhere public; org-admin tooling that
    /// kills outstanding sessions for a removed member.
    #[instrument(skip(self), fields(workspace_id = %workspace_id, username = %username))]
    pub async fn revoke_user_token(
        &self,
        workspace_id: Uuid,
        username: &str,
    ) -> Result<bool, BrokerError> {
        let TenantSecret {
            tenant_id,
            sa_bearer,
        } = self.load_tenant_secret(workspace_id).await?;
        let revoked = self
            .client
            .revoke_token(&tenant_id, username, TokenAuth::ServiceAccount(&sa_bearer))
            .await?;
        if revoked {
            info!(
                workspace_id = %workspace_id,
                tenant_id = %tenant_id,
                username = %username,
                "revoked airhouse ephemeral credential"
            );
        }
        Ok(revoked)
    }

    fn cache_key(&self, workspace_id: Uuid, subject: &BrokerSubject, role: UserRole) -> CacheKey {
        // The subject may carry its own workspace_id (system subjects do; user
        // subjects don't) — that field is informational only. The cache key
        // intentionally uses the `workspace_id` parameter, never `subject.workspace_id()`,
        // so the same user across two workspaces hashes to two distinct entries.
        (workspace_id, subject.audit_subject(), role)
    }

    async fn mint(
        &self,
        workspace_id: Uuid,
        subject: BrokerSubject,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let key = self.cache_key(workspace_id, &subject, role);

        // Fast path: cache hit.
        {
            let read = self.cache.read().await;
            if let Some(entry) = read.get(&key)
                && entry.is_fresh(Utc::now())
            {
                return Ok(entry.cred.clone());
            }
        }

        // Slow path: load tenant + SA, mint, cache.
        let TenantSecret {
            tenant_id,
            sa_bearer,
        } = self.load_tenant_secret(workspace_id).await?;
        let cred = self
            .mint_with_retry(&tenant_id, &sa_bearer, &subject, role, ttl)
            .await?;

        info!(
            workspace_id = %workspace_id,
            tenant_id = %tenant_id,
            subject = %subject.audit_subject(),
            role = %role.as_str(),
            ttl_secs = ttl.as_secs(),
            expires_at = %cred.expires_at,
            "minted airhouse ephemeral credential"
        );

        self.cache
            .write()
            .await
            .insert(key, CacheEntry { cred: cred.clone() });
        Ok(cred)
    }

    async fn mint_with_retry(
        &self,
        tenant_id: &str,
        sa_bearer: &str,
        subject: &BrokerSubject,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let ttl_secs = ttl.as_secs().min(i32::MAX as u64) as i32;
        let subject_str = subject.audit_subject();
        let write_schemas = subject.write_schemas(role);

        let mut attempt = 0;
        loop {
            match self
                .client
                .mint_token(
                    tenant_id,
                    sa_bearer,
                    &subject_str,
                    role,
                    ttl_secs,
                    write_schemas.as_deref(),
                )
                .await
            {
                Ok(cred) => return Ok(cred),
                Err(AirhouseError::RateLimited(msg)) => {
                    if attempt >= RATE_LIMIT_BACKOFFS.len() {
                        warn!(
                            tenant_id = %tenant_id,
                            attempts = attempt + 1,
                            "airhouse mint hit rate-limit ceiling: {msg}"
                        );
                        return Err(BrokerError::RateLimitExceeded(attempt));
                    }
                    let delay = jittered(RATE_LIMIT_BACKOFFS[attempt]);
                    warn!(
                        tenant_id = %tenant_id,
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        "airhouse mint rate-limited; backing off"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn load_tenant_secret(&self, workspace_id: Uuid) -> Result<TenantSecret, BrokerError> {
        let conn = establish_connection().await.map_err(|e| {
            BrokerError::Db(sea_orm::DbErr::Custom(format!(
                "failed to open oxy DB connection: {e}"
            )))
        })?;
        let row = AirhouseTenants::find()
            .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
            .one(&conn)
            .await?
            .ok_or(BrokerError::TenantNotFound(workspace_id))?;

        let bearer_ciphertext = row
            .bearer_ciphertext
            .ok_or(BrokerError::TenantHasNoServiceAccount(workspace_id))?;
        let bearer_bytes = envelope::open(&bearer_ciphertext)
            .map_err(|e| BrokerError::Crypto(workspace_id, e.to_string()))?;
        let sa_bearer = String::from_utf8(bearer_bytes)
            .map_err(|e| BrokerError::Crypto(workspace_id, format!("non-utf8 bearer: {e}")))?;

        Ok(TenantSecret {
            tenant_id: row.airhouse_tenant_id,
            sa_bearer,
        })
    }
}

struct TenantSecret {
    tenant_id: String,
    sa_bearer: String,
}

/// Add ±20% jitter so retries from N parallel callers don't synchronise
/// on the next allowed mint slot.
fn jittered(base: Duration) -> Duration {
    let base_ms = base.as_millis() as i64;
    let jitter_range = (base_ms / 5).max(1);
    let mut rng = rand::rng();
    let delta: i64 = rng.random_range(-jitter_range..=jitter_range);
    Duration::from_millis(((base_ms + delta).max(0)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_subject_renders_as_uuid() {
        let uid = Uuid::nil();
        assert_eq!(BrokerSubject::User(uid).audit_subject(), uid.to_string());
    }

    #[test]
    fn system_subject_includes_workspace_and_purpose() {
        let ws = Uuid::nil();
        let s = BrokerSubject::System {
            workspace_id: ws,
            purpose: SystemPurpose::Scheduler,
        };
        assert_eq!(
            s.audit_subject(),
            format!("system:workspace:{ws}:scheduler")
        );
    }

    #[test]
    fn airhouse_role_mapping_matches_user_provisioner() {
        assert_eq!(airhouse_role_for(WorkspaceRole::Owner), UserRole::Admin);
        assert_eq!(airhouse_role_for(WorkspaceRole::Admin), UserRole::Writer);
        assert_eq!(airhouse_role_for(WorkspaceRole::Member), UserRole::Reader);
        assert_eq!(airhouse_role_for(WorkspaceRole::Viewer), UserRole::Reader);
    }

    #[test]
    fn cache_entry_fresh_outside_buffer() {
        let cred = mock_cred(Utc::now() + chrono::Duration::seconds(3600));
        let entry = CacheEntry { cred };
        assert!(entry.is_fresh(Utc::now()));
    }

    #[test]
    fn cache_entry_stale_inside_buffer() {
        let cred = mock_cred(Utc::now() + chrono::Duration::seconds(30));
        let entry = CacheEntry { cred };
        assert!(!entry.is_fresh(Utc::now()));
    }

    #[test]
    fn cache_entry_stale_when_expired() {
        let cred = mock_cred(Utc::now() - chrono::Duration::seconds(1));
        let entry = CacheEntry { cred };
        assert!(!entry.is_fresh(Utc::now()));
    }

    #[test]
    fn jittered_stays_within_envelope() {
        let base = Duration::from_millis(1000);
        for _ in 0..1000 {
            let j = jittered(base);
            assert!(j.as_millis() >= 800);
            assert!(j.as_millis() <= 1200);
        }
    }

    fn mock_cred(expires_at: DateTime<Utc>) -> EphemeralCredential {
        EphemeralCredential {
            username: "eph_test".into(),
            password: "tk_test".into(),
            tenant: "test".into(),
            role: "reader".into(),
            expires_at,
            service_account_id: "sa_test".into(),
            write_schemas: None,
        }
    }
}
