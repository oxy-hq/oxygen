//! `SeaORM` Entity for `api_tokens` — every API credential, hashed at rest.
//!
//! Design: `internal-docs/2026-09-30-api-tokens-design.md` §7. Phase 1 carries
//! the columns it enforces; grant and trust-policy columns arrive with the
//! phases that read them. `kind` and `source` are plain text on purpose: a
//! validator refuses a kind it does not know (§4.7), so a newer release can add
//! one without an older binary ever honouring it.
//!
//! Foreign keys live in the migration (`m20261001_000001_api_tokens`); no
//! relation fields are declared because nothing joins through them in Rust.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "api_tokens")]
pub struct Model {
    /// The public token id named in audit rows. For a row linked to a legacy
    /// `api_keys` row it is that row's id, so the id the API Keys UI already
    /// holds is the token id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// `personal | service_account | ci | legacy_key | legacy_publish`.
    pub kind: String,
    /// The user the token acts as.
    pub principal_user_id: Uuid,
    pub name: String,
    pub display_prefix: String,
    pub last_four: String,
    /// SHA-256 of the whole token. Unique.
    pub token_hash: Vec<u8>,
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    pub expires_at: Option<DateTimeWithTimeZone>,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    /// Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub revoked_by: Option<Uuid>,
    pub revoke_reason: Option<String>,
    /// `ui | oxyc_login | oxyc | oxyc_agent | oidc | legacy_backfill |
    /// legacy_lazy | legacy_endpoint` (`oxy_auth::token::credential::source`).
    pub source: String,
    /// The `api_keys` row this token mirrors, while that table still exists.
    /// Validation honours its `is_active`, because a pod one release back
    /// revokes by writing `api_keys` alone.
    pub legacy_api_key_id: Option<Uuid>,
    /// On a `ci` token: the trust policy that minted it. `None` once the
    /// policy is deleted, and for every other kind.
    pub trust_policy_id: Option<Uuid>,
    /// On a `ci` token: the verified claims of the run it was minted for
    /// (`jti`, `run_id`, `sha`, `ref`, …). Never the JWT.
    pub oidc_claims: Option<Json>,
}

impl ActiveModelBehavior for ActiveModel {}
