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

#[sea_orm::model]
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
    /// The platform-schema version the branch is known to carry on the cut
    /// [`Self::platform_schema_cut_at`] names, and only on that one: 0 after
    /// every cut and reset (a fresh copy of production at a version nobody
    /// read), brought current by the events outbox's first grant or claim
    /// there (`events::env_branch`).
    pub platform_schema_version: i32,
    /// The cut (`COALESCE(last_reset_at, created_at)`) the version was
    /// recorded on. Any other cut — a reset by a binary that does not know
    /// the version, which leaves it as it was — reads as version 0. `None`
    /// until the outbox records the branch current.
    pub platform_schema_cut_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(
        belongs_to,
        from = "tenant_row_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub tenants: BelongsTo<super::tenants::Entity>,
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub env_branch_roles: HasMany<super::env_branch_roles::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
