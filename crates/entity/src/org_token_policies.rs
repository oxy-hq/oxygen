//! `SeaORM` Entity for `org_token_policies` — what an org asks of the API
//! tokens that reach it (API-tokens design §5).
//!
//! One row per org that changed a default; no row means the defaults. It binds
//! new-format tokens only — never a legacy key. The foreign keys and the
//! `max_lifetime_days` CHECK (1–3650) live in the migration
//! (`m20261003_000001_org_token_policies`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "org_token_policies")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    /// The longest a token reaching the org may live, `expires_at -
    /// created_at`, in days. `None` = no cap.
    pub max_lifetime_days: Option<i32>,
    /// Whether an all-access personal token reaches the org.
    pub allow_all_access_tokens: bool,
    /// Whether a trust policy in the org must name a deployment environment.
    pub require_environment_on_trust_policies: bool,
    /// Audit only; never authority.
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
