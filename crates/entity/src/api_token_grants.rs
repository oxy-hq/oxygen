//! `SeaORM` Entity for `api_token_grants` — what a token with
//! `all_access = false` covers (API-tokens design §3.2, §7).
//!
//! `kind` and `role_ceiling` are plain text on purpose: the validator refuses
//! a value it does not know (§4.7), so a newer release can add one without an
//! older binary ever honouring it. Foreign keys live in the migration
//! (`m20261001_000003_api_token_grants`); nothing joins through them in Rust.

use sea_orm::entity::prelude::*;

/// `(org, workspace | every workspace in the org, role ceiling)`.
pub const KIND_WORKSPACE: &str = "workspace";
/// `(app, publish)` — Phase 4. Stored, refused by this release's validator.
pub const KIND_APP_PUBLISH: &str = "app_publish";
/// `(app, its sandboxes)` — the one grant a `sandbox_agent` token holds
/// (sandbox agent credential design §2). `org_id` and `app_id` are set;
/// `workspace_id` and `role_ceiling` are not.
pub const KIND_APP_SANDBOX: &str = "app_sandbox";
/// `(app, its staging)` — beside the `app_sandbox` grant of the same app, on
/// a `sandbox_agent` token minted with `staging` (sandbox agent credential
/// design, "Staging option"). Same columns as `app_sandbox`. Alone it grants
/// nothing: admission refuses one with no `app_sandbox` twin, and refuses the
/// kind on any other token. A binary that predates it refuses the whole token
/// (an unknown grant kind), so a revert fails the token closed.
pub const KIND_APP_STAGING: &str = "app_staging";

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "api_token_grants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub token_id: Uuid,
    /// `workspace | app_publish`.
    pub kind: String,
    pub org_id: Uuid,
    /// `None` = every workspace in the org, including ones created later, and
    /// the org's own routes.
    pub workspace_id: Option<Uuid>,
    /// `viewer | member | admin | owner`; `None` for `app_publish`.
    pub role_ceiling: Option<String>,
    /// `app_publish` only.
    pub app_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    /// Set when the org ends this grant; the token's other grants stand.
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub revoked_by: Option<Uuid>,
}

impl ActiveModelBehavior for ActiveModel {}
