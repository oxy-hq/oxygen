//! `SeaORM` Entity for `app_platform_grants` — one platform action an org admin
//! approved for a custom app (`internal-docs/proposals/app-platform-access.md`).
//!
//! Each row hangs off the app's `api_tokens` row of kind `app`, which has no
//! secret: the token row is what the inventory lists and a revoke ends, and
//! these rows say which actions it holds. Foreign keys and the live-row unique
//! index live in the migration (`m20261118_000050_app_platform_access`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "app_platform_grants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// The app's `api_tokens` row (kind `app`).
    pub token_id: Uuid,
    pub app_id: Uuid,
    pub org_id: Uuid,
    /// The action's stable id (`oxy_authz::Action::app_action_id`). A row
    /// naming anything else grants nothing.
    pub action: String,
    /// What one operation may act on inside the org, for an action that has
    /// such a thing (a pipeline, for an upload). `None` approves the action
    /// across the org. Never a workspace: approvals are the org's and the app's.
    pub target: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    /// The org admin who approved it. Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub revoked_by: Option<Uuid>,
}

impl ActiveModelBehavior for ActiveModel {}
