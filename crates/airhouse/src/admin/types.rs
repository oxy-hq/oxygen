use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Internal deserialization type that includes the pg_url field returned by Airhouse.
/// Never expose this type or the pg_url value in API responses.
#[derive(Deserialize)]
pub(crate) struct TenantRecordRaw {
    pub id: String,
    pub pg_url: String,
    pub bucket: String,
    pub prefix: Option<String>,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// A provisioned Airhouse tenant.
///
/// The `pg_url` field is intentionally private. Access it only in internal worker
/// code that needs to connect to the DuckLake catalog directly. Never expose it in
/// API responses or user-facing surfaces — end users connect via the wire-protocol
/// port with their own credentials.
#[derive(Clone)]
pub struct TenantRecord {
    pub id: String,
    pub bucket: String,
    pub prefix: Option<String>,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pg_url: String,
}

impl TenantRecord {
    /// Returns the internal DuckLake connection URL.
    ///
    /// **Internal use only.** Never expose this in API responses or user-facing surfaces.
    /// End users connect via the wire-protocol port (`host`, `port`, `dbname`, `user`, `password`).
    pub fn pg_url(&self) -> &str {
        &self.pg_url
    }
}

impl fmt::Debug for TenantRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TenantRecord")
            .field("id", &self.id)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("role", &self.role)
            .field("status", &self.status)
            .field("created_at", &self.created_at)
            .field("pg_url", &"[redacted]")
            .finish()
    }
}

impl From<TenantRecordRaw> for TenantRecord {
    fn from(raw: TenantRecordRaw) -> Self {
        Self {
            id: raw.id,
            bucket: raw.bucket,
            prefix: raw.prefix,
            role: raw.role,
            status: raw.status,
            created_at: raw.created_at,
            pg_url: raw.pg_url,
        }
    }
}

/// Role granted to an Airhouse tenant user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Reader,
    Writer,
    Admin,
}

impl UserRole {
    /// String form used by Airhouse on the wire and in audit fields.
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::Reader => "reader",
            UserRole::Writer => "writer",
            UserRole::Admin => "admin",
        }
    }
}

/// A user within an Airhouse tenant.
#[derive(Debug, Clone, Deserialize)]
pub struct UserRecord {
    pub id: String,
    pub tenant_id: String,
    pub username: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// A registered Airhouse service account. Returned by list / create endpoints
/// — the raw bearer is never included here; it appears once on
/// [`CreatedServiceAccount`] at create time only.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceAccountRecord {
    pub id: String,
    pub name: String,
    pub tenant_id: String,
    pub max_role: String,
    pub max_ttl_secs: i32,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Response from [`crate::AirhouseAdminClient::create_service_account`]. The
/// `bearer` is the only handle to this SA — Airhouse persists only its hash,
/// so a lost bearer cannot be recovered, only rotated. Treat it like a
/// platform secret; pass it straight into your secret manager.
#[derive(Debug, Clone)]
pub struct CreatedServiceAccount {
    pub record: ServiceAccountRecord,
    pub bearer: String,
}

/// Raw (server-generated) ephemeral wire-protocol credential returned by the
/// mint endpoint. Username and password are opaque — pass them straight to a
/// pgwire client. Past `expires_at` the same credential stops authenticating
/// with `28P01` and the caller should mint a fresh one.
#[derive(Debug, Clone, Deserialize)]
pub struct EphemeralCredential {
    pub username: String,
    pub password: String,
    pub tenant: String,
    pub role: String,
    pub expires_at: DateTime<Utc>,
    pub service_account_id: String,
    /// The schemas Airhouse confined this credential's writes to, echoed from
    /// the mint request. `None` both when none were asked for and when the
    /// Airhouse predates scoped credentials and ignored the request — which is
    /// why a caller that asked must treat `None` as unscoped.
    #[serde(default)]
    pub write_schemas: Option<Vec<String>>,
}

/// Auth selector for the per-token revoke endpoint.
///
/// Airhouse accepts either the deployment's admin token (revokes any
/// credential in any tenant) or the SA bearer that originally issued the
/// credential (revokes only what that SA minted). Other SA bearers get 403.
#[derive(Debug, Clone)]
pub enum TokenAuth<'a> {
    Admin,
    ServiceAccount(&'a str),
}

/// Current state of a tenant's DuckLake catalog hot-path indexes, from
/// `GET /admin/v1/tenants/{id}/catalog-indexes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndexesResponse {
    /// Aggregate state derived server-side: `"ready"` (all present + valid),
    /// `"building"` (present but a `CONCURRENTLY` build is still in flight), or
    /// `"absent"` (indexes not created / toggled off).
    pub state: String,
    pub indexes: Vec<CatalogIndexState>,
}

/// Presence + validity of one catalog hot-path index. `valid` mirrors Postgres
/// `indisvalid` — false while a `CONCURRENTLY` build is in flight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndexState {
    pub name: String,
    pub present: bool,
    pub valid: bool,
}
