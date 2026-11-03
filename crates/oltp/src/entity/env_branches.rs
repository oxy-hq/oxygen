//! `oltp_env_branches` — one custom-app environment's own branch of the org's
//! OLTP database.
//!
//! One row per `(tenant, app, env_name)`. Beside [`super::branches`] (the org's
//! shared staging branch) and shaped like it — where to connect, the owner's
//! sealed password — plus the fork point and expiry that make a per-environment
//! branch disposable. The roles inside carry their own sealed credentials in
//! [`super::env_branch_roles`].
//!
//! A separate table from `oltp_branches`, not a new `kind`: a reverted binary
//! decodes every `oltp_branches.kind` and would fail on one it does not know.
//!
//! `app_id` has no foreign key: deleting an app must not drop the row of a live
//! provider branch. The provider's `expires_at` backstops an orphan.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// How the branch's data was seeded.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, DeriveActiveEnum,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
pub enum EnvBranchMode {
    /// Forked from the parent branch: schema and data as of `fork_at`.
    #[sea_orm(string_value = "branch")]
    Branch,
    /// Schema only: the parent's structure with none of its rows.
    #[sea_orm(string_value = "empty")]
    Empty,
}

impl EnvBranchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvBranchMode::Branch => "branch",
            EnvBranchMode::Empty => "empty",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum EnvBranchStatus {
    /// Recorded; the provider branch or its credentials are not all made yet.
    #[sea_orm(string_value = "cutting")]
    Cutting,
    /// Cut, and the app's OLTP migrations are being applied. The migration
    /// apply accepts this state; a function call is not served in it.
    #[sea_orm(string_value = "migrating")]
    Migrating,
    /// Cut, credentials minted, migrations applied. The only state a function
    /// call is served in.
    #[sea_orm(string_value = "ready")]
    Ready,
    /// A reset is in flight or failed part way. Held here until it completes so
    /// nothing pairs a half-reset branch with credentials the reset replaced.
    #[sea_orm(string_value = "resetting")]
    Resetting,
    /// A step failed; `error` says which. The next attempt resumes from it.
    #[sea_orm(string_value = "failed")]
    Failed,
}

impl EnvBranchStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvBranchStatus::Cutting => "cutting",
            EnvBranchStatus::Migrating => "migrating",
            EnvBranchStatus::Ready => "ready",
            EnvBranchStatus::Resetting => "resetting",
            EnvBranchStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_env_branches")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_row_id: Uuid,
    /// The custom app. No FK — see the module doc.
    pub app_id: Uuid,
    /// The environment's name (`dev-<handle>`, `preview-…`).
    pub env_name: String,

    pub mode: EnvBranchMode,
    pub status: EnvBranchStatus,
    /// What failed, when `status` is `failed`.
    pub error: Option<String>,

    /// Provider-side branch id (Neon's `br-…`). `None` until the provider has
    /// created the branch.
    pub provider_branch_id: Option<String>,
    /// The branch this one was cut from.
    pub parent_branch_id: String,
    pub host: Option<String>,
    pub database_name: Option<String>,
    pub owner_role: Option<String>,
    /// AES-GCM-sealed owner password for THIS branch — same envelope as
    /// `oltp_branches.owner_password_ciphertext`. `None` until minted, or on a
    /// provider whose branches share the tenant's roles.
    pub owner_password_ciphertext: Option<Vec<u8>>,

    /// The point of the parent the branch was cut at.
    pub fork_at: DateTimeWithTimeZone,
    /// When the provider reaps the branch if nothing renews it.
    pub expires_at: DateTimeWithTimeZone,

    pub created_at: DateTimeWithTimeZone,
    pub last_reset_at: Option<DateTimeWithTimeZone>,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::tenants::Entity",
        from = "Column::TenantRowId",
        to = "super::tenants::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Tenants,
    #[sea_orm(has_many = "super::env_branch_roles::Entity")]
    EnvBranchRoles,
}

impl Related<super::tenants::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tenants.def()
    }
}

impl Related<super::env_branch_roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EnvBranchRoles.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
