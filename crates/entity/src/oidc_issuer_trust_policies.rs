//! `SeaORM` Entity for `oidc_issuer_trust_policies` — *a token from this
//! registered issuer whose claims meet these conditions may act as this Oxygen
//! service account* (API-tokens design §3.4, *Any OIDC issuer*).
//!
//! `conditions` is a JSON list of `{"claim": …, "value": …}`, checked when
//! written (`oxy_auth::workload_identity::conditions`). Foreign keys live in
//! the migration (`m20261119_000010_oidc_issuer_trust_policies`).

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "oidc_issuer_trust_policies")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    /// `service_accounts.user_id` — the Oxygen account a matching token acts as.
    pub service_account_id: Uuid,
    /// `oidc_issuers.id`.
    pub issuer_id: Uuid,
    /// `[{"claim": "sub", "value": "arn:aws:iam::123456789012:role/ferret-task"}]`.
    #[sea_orm(column_type = "JsonBinary")]
    pub conditions: Json,
    /// Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    /// Set = the policy mints nothing.
    pub disabled_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
