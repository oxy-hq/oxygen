//! The envelope: is this a JWT GitHub signed, for us, now, and not yet used?
//!
//! - **RS256 only.** The header's `alg` is never trusted.
//! - **Issuer pinned** to GitHub Actions.
//! - **The audience is the caller's.** Each exchange requires its own and
//!   refuses the other's, so a token requested for a publish cannot be traded
//!   for trusted access, nor the reverse.
//! - **`exp` and `nbf`**, each with [`LEEWAY_SECONDS`] of leeway.
//! - **`jti` single use**, burned only after everything above held — so a
//!   token refused for its audience is not spent by the refusal.

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use sea_orm::ConnectionTrait;

use super::claims::{GITHUB_OIDC_ISSUER, GithubOidcClaims};
use super::jti;
use super::keys::JwksCache;

/// Clock skew allowed on `exp` and `nbf`.
pub const LEEWAY_SECONDS: u64 = 30;

/// Why the envelope was refused. Distinct from the claim-matching refusals so
/// the caller can answer precisely.
#[derive(Debug)]
pub enum OidcError {
    /// Could not reach or parse GitHub's JWKS and had no cached copy.
    JwksUnavailable,
    /// Header had no `kid`, or no key matched even after a refresh.
    UnknownKey,
    /// Signature, issuer, `nbf` or shape verification failed.
    InvalidToken(String),
    /// A good token, minted for another audience.
    WrongAudience,
    /// A good token, past its `exp`.
    Expired,
    /// The `jti` was already spent — a replay.
    Replayed,
    /// A DB error recording the `jti`. Fails closed (we do not accept a token we
    /// could not mark used).
    Db(String),
}

impl OidcError {
    /// The wire code of a refusal that answers 401, and the metric's reason
    /// label. `None` for a failure of ours ([`OidcError::Db`]), which is a 500.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::JwksUnavailable | Self::UnknownKey | Self::InvalidToken(_) => {
                Some("invalid_token")
            }
            Self::WrongAudience => Some("wrong_audience"),
            Self::Expired => Some("expired"),
            Self::Replayed => Some("replayed"),
            Self::Db(_) => None,
        }
    }
}

fn validation(audience: &str) -> Validation {
    // RS256 only — never trust the header's alg. Pin issuer + the required
    // audience.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[GITHUB_OIDC_ISSUER]);
    validation.set_audience(&[audience]);
    // `iat` is required by `GithubOidcClaims` itself: this list ignores it.
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    validation.validate_nbf = true;
    validation.leeway = LEEWAY_SECONDS;
    validation
}

/// Verify the signature and the registered claims with `key`, for `audience`.
/// Pure: no network, no database, and the `jti` is not burned.
pub fn decode_claims(
    token: &str,
    key: &DecodingKey,
    audience: &str,
) -> Result<GithubOidcClaims, OidcError> {
    match decode::<GithubOidcClaims>(token, key, &validation(audience)) {
        Ok(data) => Ok(data.claims),
        Err(e) => Err(match e.kind() {
            ErrorKind::InvalidAudience => OidcError::WrongAudience,
            ErrorKind::ExpiredSignature => OidcError::Expired,
            _ => OidcError::InvalidToken(e.to_string()),
        }),
    }
}

/// Verify a GitHub Actions OIDC JWT end to end for `audience`, and burn its
/// `jti` so it cannot be replayed. Returns the decoded claims for the caller's
/// matcher.
///
/// `db` is used only to record the `jti`. Fails closed on any DB error — a
/// token we cannot mark used is a token we do not accept.
pub async fn verify_token<C: ConnectionTrait>(
    db: &C,
    keys: &JwksCache,
    token: &str,
    audience: &str,
) -> Result<GithubOidcClaims, OidcError> {
    let header = decode_header(token).map_err(|e| OidcError::InvalidToken(e.to_string()))?;
    let kid = header.kid.ok_or(OidcError::UnknownKey)?;
    let key = keys.decoding_key(&kid).await?;
    let claims = decode_claims(token, &key, audience)?;
    jti::burn(db, &claims.jti).await?;
    Ok(claims)
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
