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

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use entity::cli_auth_codes;
use entity::prelude::CliAuthCodes;
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
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

fn hash_code(code: &str) -> Vec<u8> {
    Sha256::digest(code.as_bytes()).to_vec()
}

/// Issue a code for `user_id`, redeemable once against `code_challenge` until
/// it expires. The caller has validated both inputs.
pub async fn authorize<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    code_challenge: &str,
    hostname: &str,
) -> Result<String, OxyError> {
    let now = Utc::now();
    sweep(db, now).await;
    let code = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    cli_auth_codes::ActiveModel {
        code_hash: Set(hash_code(&code)),
        user_id: Set(user_id),
        code_challenge: Set(code_challenge.to_string()),
        hostname: Set(hostname.to_string()),
        created_at: Set(now.fixed_offset()),
        expires_at: Set((now + Duration::seconds(CODE_TTL_SECS)).fixed_offset()),
        consumed_at: Set(None),
    }
    .insert(db)
    .await
    .map_err(db_err("store cli auth code"))?;
    Ok(code)
}

/// Best-effort: the table holds only rows a login is waiting on.
async fn sweep<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) {
    let cutoff = (now - Duration::seconds(SWEEP_AFTER_SECS)).fixed_offset();
    if let Err(e) = CliAuthCodes::delete_many()
        .filter(cli_auth_codes::Column::ExpiresAt.lt(cutoff))
        .exec(db)
        .await
    {
        tracing::warn!(error = %e, "could not sweep lapsed cli auth codes");
    }
}

/// Who a redeemed code belongs to, and the host it was issued for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redeemed {
    pub user_id: Uuid,
    pub hostname: String,
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
    let hash = hash_code(code);
    let now = Utc::now();
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
    let Some(row) = CliAuthCodes::find_by_id(hash)
        .one(db)
        .await
        .map_err(db_err("read cli auth code"))?
    else {
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
