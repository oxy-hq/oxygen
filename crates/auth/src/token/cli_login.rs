//! The `oxyc login` PKCE loopback exchange (API-tokens design §6).
//!
//! 1. The browser, under its session, asks for a **code** bound to the CLI's
//!    S256 challenge ([`authorize`]). Single use, five minutes.
//! 2. The browser redirects to `http://127.0.0.1:<port>/callback?code=…`. No
//!    credential is ever in a URL — the code alone is worthless.
//! 3. The CLI redeems `code + verifier` ([`redeem`]) and the handler mints the
//!    token.
//!
//! Only the code's SHA-256 is stored. **Any** exchange attempt spends the
//! code — a wrong verifier included — so a party that intercepted the code
//! cannot guess at the verifier, and a replay finds nothing.
//!
//! ## A code that mints something else
//!
//! The same exchange mints a sandbox agent token when the browser asked for
//! one ([`authorize_mint`]): the code then carries **what** it mints, and the
//! handler mints that instead of the login token.
//!
//! Such a code is stored under a *different* hash — the code prefixed with
//! [`MINT_DOMAIN`] — and that is a safety property, not tidiness. A binary one
//! release back knows only the login hash and mints an all-access login token
//! for any code it finds. Under its own hash a mint code is one that binary
//! cannot find, so redeeming it there answers `invalid_code` instead of
//! handing an agent its operator's whole reach. A row is honoured only under
//! the hash its own `mint` column says it belongs to.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use entity::cli_auth_codes;
use entity::prelude::CliAuthCodes;
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// How long a code can be redeemed.
pub const CODE_TTL_SECS: i64 = 5 * 60;
/// Spent and lapsed codes are swept this long after they expire.
const SWEEP_AFTER_SECS: i64 = 60 * 60;
/// An S256 challenge is 32 bytes, base64url without padding.
const CHALLENGE_LEN: usize = 43;
/// RFC 7636 bounds on a verifier.
const VERIFIER_LEN: std::ops::RangeInclusive<usize> = 43..=128;
const MAX_HOSTNAME_CHARS: usize = 255;
/// What a mint code is prefixed with before it is hashed. See the module docs.
const MINT_DOMAIN: &str = "oxy-cli-mint:";

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

fn is_base64url(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `base64url(sha256(verifier))`, no padding — RFC 7636's S256.
pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A well-formed S256 challenge, as stored: base64url with any `=` padding
/// dropped, since clients differ on sending it. `None` when it is not one.
/// Shape only — it is opaque until redeemed.
pub fn clean_challenge(challenge: &str) -> Option<String> {
    let bare = challenge.trim().trim_end_matches('=');
    (bare.len() == CHALLENGE_LEN && is_base64url(bare)).then(|| bare.to_string())
}

/// The hostname as it will appear in the token's name, or `None` when it is
/// empty, too long or carries a control character.
pub fn clean_hostname(raw: &str) -> Option<String> {
    let host = raw.trim();
    let ok = !host.is_empty()
        && host.chars().count() <= MAX_HOSTNAME_CHARS
        && !host.chars().any(char::is_control);
    ok.then(|| host.to_string())
}

/// The name of the token `oxyc login` mints on `hostname`. Logging in again
/// from the same host retires the token of this name.
pub fn token_name(hostname: &str) -> String {
    format!("oxyc on {hostname}")
}

pub(super) fn hash_code(code: &str) -> Vec<u8> {
    Sha256::digest(code.as_bytes()).to_vec()
}

/// The hash a **mint** code is stored under: never the one a login code is.
pub(super) fn hash_mint_code(code: &str) -> Vec<u8> {
    Sha256::digest(format!("{MINT_DOMAIN}{code}").as_bytes()).to_vec()
}

/// Issue a login code for `user_id`, redeemable once against `code_challenge`
/// until it expires. The caller has validated both inputs.
pub async fn authorize<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    code_challenge: &str,
    hostname: &str,
) -> Result<String, OxyError> {
    issue(db, user_id, code_challenge, hostname, None).await
}

/// Issue a code that mints `mint` instead of the login token — what the
/// browser approved, already checked against what the session may mint.
pub async fn authorize_mint<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    code_challenge: &str,
    hostname: &str,
    mint: Value,
) -> Result<String, OxyError> {
    issue(db, user_id, code_challenge, hostname, Some(mint)).await
}

async fn issue<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    code_challenge: &str,
    hostname: &str,
    mint: Option<Value>,
) -> Result<String, OxyError> {
    let now = Utc::now();
    sweep(db, now).await;
    let code = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let code_hash = match mint {
        Some(_) => hash_mint_code(&code),
        None => hash_code(&code),
    };
    cli_auth_codes::ActiveModel {
        code_hash: Set(code_hash),
        user_id: Set(user_id),
        code_challenge: Set(code_challenge.to_string()),
        hostname: Set(hostname.to_string()),
        created_at: Set(now.fixed_offset()),
        expires_at: Set((now + Duration::seconds(CODE_TTL_SECS)).fixed_offset()),
        consumed_at: Set(None),
        mint: Set(mint),
    }
    .insert(db)
    .await
    .map_err(db_err("store cli auth code"))?;
    Ok(code)
}

/// Best-effort: the table holds only rows a login is waiting on. Shared with
/// `browser_session`, whose tickets are rows of the same table.
pub(super) async fn sweep<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) {
    let cutoff = (now - Duration::seconds(SWEEP_AFTER_SECS)).fixed_offset();
    if let Err(e) = CliAuthCodes::delete_many()
        .filter(cli_auth_codes::Column::ExpiresAt.lt(cutoff))
        .exec(db)
        .await
    {
        tracing::warn!(error = %e, "could not sweep lapsed cli auth codes");
    }
}

/// Who a redeemed code belongs to, the host it was issued for, and what it
/// mints when that is not the login token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redeemed {
    pub user_id: Uuid,
    pub hostname: String,
    /// `None` for an `oxyc login` code.
    pub mint: Option<Value>,
}

/// Mark the code stored under `hash` spent and return its row. `None` when
/// there is no such unspent code. One conditional `UPDATE`, so two concurrent
/// exchanges cannot both win.
pub(super) async fn spend<C: ConnectionTrait>(
    db: &C,
    hash: Vec<u8>,
    now: DateTime<Utc>,
) -> Result<Option<cli_auth_codes::Model>, OxyError> {
    let spent = CliAuthCodes::update_many()
        .col_expr(
            cli_auth_codes::Column::ConsumedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(cli_auth_codes::Column::CodeHash.eq(hash.clone()))
        .filter(cli_auth_codes::Column::ConsumedAt.is_null())
        .exec(db)
        .await
        .map_err(db_err("redeem cli auth code"))?;
    if spent.rows_affected != 1 {
        return Ok(None);
    }
    CliAuthCodes::find_by_id(hash)
        .one(db)
        .await
        .map_err(db_err("read cli auth code"))
}

/// The row `code` names, spent: a login code under the login hash, or a mint
/// code under the mint hash. A row whose `mint` disagrees with the hash it
/// was found under is no code at all.
async fn spend_code<C: ConnectionTrait>(
    db: &C,
    code: &str,
    now: DateTime<Utc>,
) -> Result<Option<cli_auth_codes::Model>, OxyError> {
    if let Some(row) = spend(db, hash_code(code), now).await? {
        return Ok(row.mint.is_none().then_some(row));
    }
    let row = spend(db, hash_mint_code(code), now).await?;
    Ok(row.filter(|row| row.mint.is_some()))
}

/// Redeem `code` with `code_verifier`. `Ok(None)` for every failure — an
/// unknown code, a spent one, an expired one, a verifier that does not match —
/// which the caller answers identically.
///
/// The code is marked spent **first**, in one conditional `UPDATE`, so two
/// concurrent exchanges cannot both win and a failed attempt still burns it.
pub async fn redeem<C: ConnectionTrait>(
    db: &C,
    code: &str,
    code_verifier: &str,
) -> Result<Option<Redeemed>, OxyError> {
    let now = Utc::now();
    let Some(row) = spend_code(db, code, now).await? else {
        return Ok(None);
    };
    let live = DateTime::<Utc>::from(row.expires_at) > now;
    let verifier_ok = VERIFIER_LEN.contains(&code_verifier.len())
        && challenge_of(code_verifier) == row.code_challenge;
    if !live || !verifier_ok {
        return Ok(None);
    }
    Ok(Some(Redeemed {
        user_id: row.user_id,
        hostname: row.hostname,
        mint: row.mint,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_rfc_7636_s256() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = challenge_of(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(clean_challenge(&challenge), Some(challenge.clone()));
        // Padding is accepted and dropped, so the stored form matches what
        // `challenge_of` computes at redemption.
        assert_eq!(clean_challenge(&format!("{challenge}=")), Some(challenge));
    }

    #[test]
    fn a_malformed_challenge_is_refused() {
        for bad in [
            "",
            "short",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw+cM",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-c",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cMx",
        ] {
            assert_eq!(clean_challenge(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_mint_code_is_stored_under_a_hash_a_login_lookup_never_computes() {
        // The property a binary one release back depends on: it looks a code
        // up by `hash_code` alone, so it must not find a mint code.
        let code = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_ne!(hash_code(code), hash_mint_code(code));
        assert_eq!(hash_code(code), Sha256::digest(code.as_bytes()).to_vec());
        assert_eq!(hash_mint_code(code).len(), 32);
    }

    #[test]
    fn a_hostname_is_trimmed_and_bounded() {
        assert_eq!(
            clean_hostname("  laptop.local "),
            Some("laptop.local".into())
        );
        assert_eq!(clean_hostname("   "), None);
        assert_eq!(clean_hostname("a\nb"), None);
        assert_eq!(clean_hostname(&"h".repeat(256)), None);
        assert_eq!(token_name("laptop.local"), "oxyc on laptop.local");
    }
}
