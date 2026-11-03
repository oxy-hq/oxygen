//! `oltp_tenants` — one row per **org**.
//!
//! Grain is deliberate and differs from `airhouse_tenants` (per workspace): a
//! workspace is a git repository, and transactional business data is not owned
//! by a repo. See the design doc for the full argument.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    /// Project exists provider-side and baseline hardening has been applied.
    #[sea_orm(string_value = "active")]
    Active,
    /// A provision attempt failed part-way. The row is kept so operators can
    /// see it; the next provision call reconciles rather than duplicating.
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "pending_delete")]
    PendingDelete,
}

impl TenantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantStatus::Active => "active",
            TenantStatus::Failed => "failed",
            TenantStatus::PendingDelete => "pending_delete",
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_tenants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub org_id: Uuid,

    /// Which provider backs this tenant (`mock`, `neon`). Persisted rather
    /// than read from config so a config change can't silently re-point an
    /// existing tenant at a different provider's id space.
    pub provider: String,
    /// Provider-side project id.
    pub project_id: String,
    /// Provider-side default branch id. Roles are scoped to a branch.
    pub branch_id: String,
    /// Provider-visible project name. Unique, and deterministic from the org,
    /// so an orphaned remote project can be found after a local-DB wipe.
    #[sea_orm(unique)]
    pub project_name: String,

    pub region: String,
    pub pg_version: i16,
    pub host: String,
    pub database_name: String,

    /// Role that owns the database. Oxy runs all schema/grant DDL as this role.
    pub owner_role: String,
    /// AES-GCM-sealed owner password (same envelope as `org_secrets`).
    ///
    /// Unlike airhouse — whose credentials are ephemeral and never stored —
    /// provider roles are durable and their password is disclosed exactly once
    /// at create time. Losing this ciphertext means the password must be
    /// **reset**, not recovered.
    pub owner_password_ciphertext: Option<Vec<u8>>,

    /// AES-GCM-sealed login password for this tenant's `oxy_analyst_ro` role —
    /// the read-only role every human and agent query resolves to.
    ///
    /// Lives here rather than in `oltp_roles` because the analyst is not a
    /// writer: no schema, no writer kind, exactly one per database. `None`
    /// means it has not been minted yet.
    pub analyst_password_ciphertext: Option<Vec<u8>>,

    pub status: TenantStatus,

    /// Version of Oxy's own in-tenant objects this database carries — see
    /// [`crate::platform`]. `0` means no platform step has run yet.
    ///
    /// Recorded here rather than only inside the tenant so Oxy can answer
    /// "which tenants are behind?" with one query against its own database,
    /// instead of waking N scale-to-zero Postgres instances to find out.
    pub platform_schema_version: i32,

    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::roles::Entity")]
    Roles,
    #[sea_orm(has_many = "super::env_branches::Entity")]
    EnvBranches,
}

impl Related<super::roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Roles.def()
    }
}

impl Related<super::env_branches::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EnvBranches.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
