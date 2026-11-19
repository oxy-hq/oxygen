//! `SeaORM` Entity for `oidc_issuers` — a token issuer an org trusts, which
//! its admin registered (API-tokens design §3.4, *Any OIDC issuer*).
//!
//! Where its keys come from is [`Model::key_source`]: a `jwks_uri` read from
//! its discovery document at registration, a `jwks_uri` the admin gave, or the
//! key set itself, inline. The exchange reads keys from that and from nowhere
//! a token names. Foreign keys and the check that the key columns agree with
//! `key_source` live in the migration
//! (`m20261119_000010_oidc_issuer_trust_policies`).

use sea_orm::entity::prelude::*;

/// Keys from the `jwks_uri` the issuer's discovery document named.
pub const KEY_SOURCE_DISCOVERY: &str = "discovery";
/// Keys from a `jwks_uri` the admin gave.
pub const KEY_SOURCE_JWKS_URI: &str = "jwks_uri";
/// Keys stored on the row; nothing is fetched.
pub const KEY_SOURCE_INLINE: &str = "inline";

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "oidc_issuers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    /// What the org calls it — `oxy-prod`. Display only.
    pub name: String,
    /// The `iss` its tokens carry, without a trailing `/`.
    pub issuer: String,
    /// `discovery | jwks_uri | inline`.
    pub key_source: String,
    /// `discovery` only: where the document was read, when not from the
    /// issuer itself.
    pub discovery_url: Option<String>,
    /// `discovery` and `jwks_uri`: where its keys are read.
    pub jwks_uri: Option<String>,
    /// `inline` only: the JSON Web Key Set.
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub jwks: Option<Json>,
    /// Whether each token's `jti` may be exchanged once only.
    pub jti_single_use: bool,
    /// Which settings-form preset filled it in. Display only: verification
    /// never reads it.
    pub preset: Option<String>,
    /// Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
