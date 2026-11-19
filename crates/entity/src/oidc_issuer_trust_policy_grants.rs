//! `SeaORM` Entity for `oidc_issuer_trust_policy_grants` — what a token matching
//! an OIDC issuer trust policy is granted.
//!
//! The same columns and `kind` values as `oidc_trust_policy_grants`; only the
//! parent differs. A policy's grants are replaced as a set, never revoked one
//! by one.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "oidc_issuer_trust_policy_grants")]
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
