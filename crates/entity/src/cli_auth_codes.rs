//! `SeaORM` Entity for `cli_auth_codes` — the single-use code of the
//! `oxyc login` PKCE loopback flow (API-tokens design §6). Only the code's
//! SHA-256 is stored, beside the S256 challenge it must be redeemed against.
//!
//! `oxyc login-link`'s one-time ticket is a row here too: the same five-minute,
//! single-use handoff between a CLI and a browser, pointed the other way. Each
//! kind of row is stored under its own hash domain, so one is never found as
//! another.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "cli_auth_codes")]
pub struct Model {
    /// SHA-256 of the code.
    #[sea_orm(primary_key, auto_increment = false)]
    pub code_hash: Vec<u8>,
    /// The browser session's user: who the minted token acts as.
    pub user_id: Uuid,
    /// `base64url(sha256(code_verifier))`, no padding.
    pub code_challenge: String,
    /// Names the token: `oxyc on <hostname>`.
    pub hostname: String,
    pub created_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    /// Set by the first exchange, valid or not: a code is spent by any attempt.
    pub consumed_at: Option<DateTimeWithTimeZone>,
    /// What the code mints when it is not an `oxyc login`: the kind, the app
    /// ids, the lifetime and the name of a sandbox agent token — or, for the
    /// ticket of a token's sign-in link (`oxy_auth::token::browser_session`),
    /// `browser_session` and the token whose session it opens. A ticket's row
    /// has no challenge and no hostname. `None` for a login code.
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub mint: Option<Json>,
}

impl ActiveModelBehavior for ActiveModel {}
