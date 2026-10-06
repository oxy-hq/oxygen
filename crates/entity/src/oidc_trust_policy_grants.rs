//! `SeaORM` Entity for `oidc_trust_policy_grants` — what a run matching a
//! trust policy is granted (API-tokens design §7).
//!
//! The same shape as `api_token_grants`, and the same `kind` values
//! (`api_token_grants::KIND_WORKSPACE` / `KIND_APP_PUBLISH`): a minted `ci`
//! token's grants are these rows, copied. There is no per-grant revocation —
//! a policy's grants are replaced as a set.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "oidc_trust_policy_grants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub policy_id: Uuid,
    /// `workspace | app_publish`.
    pub kind: String,
    pub org_id: Uuid,
    /// `workspace` only. `None` = every workspace in the org.
    pub workspace_id: Option<Uuid>,
    /// `workspace` only.
    pub role_ceiling: Option<String>,
    /// `app_publish` only.
    pub app_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
