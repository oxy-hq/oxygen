//! The envelope: is this a token the registered issuer signed, for this
//! deployment alone, now, and short-lived?
//!
//! - **RS256 only.** The header's `alg` is never trusted.
//! - **The issuer is the registered one.** The caller has already found the
//!   registered issuer whose `issuer` equals the token's `iss`, a trailing `/`
//!   ignored on both ([`unverified_issuer`] is how it reads which to look
//!   for); the signature is then checked against that issuer's keys and no
//!   others, and `iss` is compared again here.
//! - **The audience is this deployment's, and only it.** A token that names a
//!   second audience beside ours is refused: one minted for us is good
//!   nowhere else, and one good somewhere else is not ours to accept.
//! - **`exp` and `nbf`**, each with [`LEEWAY_SECONDS`] of leeway; **`iat`**
//!   required and not in the future; **`sub`** required and not empty.
//! - **A lifetime of at most [`MAX_LIFETIME_SECONDS`].** Where an issuer's
//!   tokens are not single use, a token's life is its replay window. A token
//!   with no `exp` is refused for that.
//!
//! Nothing here reads which preset registered the issuer: what a token must
//! say beyond this is its policy's conditions.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::{Map, Value};

use super::claims::VerifiedToken;
use crate::github_oidc::{JwksCache, OidcError};

/// Clock skew allowed on `exp`, `nbf` and `iat`.
pub const LEEWAY_SECONDS: u64 = 30;

/// The longest life a token may have been issued with: `exp - iat`. An hour
/// is the longest AWS STS issues one for, and longer than any CI provider's.
pub const MAX_LIFETIME_SECONDS: i64 = 3600;

/// Why the envelope was refused.
#[derive(Debug)]
pub enum IssuerTokenError {
    /// Could not read the issuer's keys and had no cached copy.
    KeysUnavailable,
    /// No `kid`, or no key of the issuer's has it, even after a refresh.
    UnknownKey,
    /// Signature, issuer, `nbf`, `iat`, `sub` or the shape.
    Invalid(String),
    /// A good token, for another audience — or for ours and another.
    WrongAudience,
    /// A good token, past its `exp`.
    Expired,
    /// A good token issued to live longer than [`MAX_LIFETIME_SECONDS`].
    LifetimeTooLong,
}

impl IssuerTokenError {
    /// The wire code, and the metric's reason label.
    pub fn code(&self) -> &'static str {
        match self {
            Self::KeysUnavailable | Self::UnknownKey | Self::Invalid(_) => "invalid_token",
            Self::WrongAudience => "wrong_audience",
            Self::Expired => "expired",
            Self::LifetimeTooLong => "token_lifetime_too_long",
        }
    }
}

impl From<OidcError> for IssuerTokenError {
    fn from(e: OidcError) -> Self {
        match e {
            OidcError::JwksUnavailable => Self::KeysUnavailable,
            OidcError::UnknownKey => Self::UnknownKey,
            other => Self::Invalid(format!("{other:?}")),
        }
    }
}

/// An issuer as it is compared: without a trailing `/`.
pub fn normalized_issuer(iss: &str) -> &str {
    iss.trim_end_matches('/')
}

/// The `iss` a token claims, **unverified**: which registered issuer to
/// verify it against, and nothing more. Never authority — the signature is
/// checked against that issuer's keys, and a token that lies about its
/// issuer fails there.
pub fn unverified_issuer(token: &str) -> Option<String> {
    let mut parts = token.trim().split('.');
    let (_header, payload) = (parts.next()?, parts.next()?);
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("iss")?.as_str().map(str::to_string)
}

fn validation(audience: &str) -> Validation {
    let mut validation = Validation::new(Algorithm::RS256);
    // `iss` is compared below, a trailing '/' ignored; here it must be there.
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = LEEWAY_SECONDS;
    validation
}

/// `aud` holds `audience` and nothing else.
fn only_audience(aud: Option<&Value>, audience: &str) -> bool {
    match aud {
        Some(Value::String(one)) => one == audience,
        Some(Value::Array(many)) => many.len() == 1 && many[0].as_str() == Some(audience),
        _ => false,
    }
}

fn invalid(why: &str) -> IssuerTokenError {
    IssuerTokenError::Invalid(why.into())
}

/// Verify the signature and the claims with `key`, for the registered
/// `issuer` and this deployment's `audience`, at `now` (seconds since the
/// epoch). Pure.
pub fn decode_claims(
    token: &str,
    key: &DecodingKey,
    issuer: &str,
    audience: &str,
    now: i64,
) -> Result<VerifiedToken, IssuerTokenError> {
    let claims = match decode::<Map<String, Value>>(token, key, &validation(audience)) {
        Ok(data) => data.claims,
        Err(e) => {
            return Err(match e.kind() {
                ErrorKind::InvalidAudience => IssuerTokenError::WrongAudience,
                ErrorKind::ExpiredSignature => IssuerTokenError::Expired,
                _ => IssuerTokenError::Invalid(e.to_string()),
            });
        }
    };
    let text = |name: &str| claims.get(name).and_then(Value::as_str).map(str::to_string);
    let number = |name: &str| claims.get(name).and_then(Value::as_i64);
    let iss = text("iss").ok_or_else(|| invalid("no 'iss'"))?;
    if normalized_issuer(&iss) != normalized_issuer(issuer) {
        return Err(invalid("issued by another issuer"));
    }
    if !only_audience(claims.get("aud"), audience) {
        return Err(IssuerTokenError::WrongAudience);
    }
    let sub = text("sub")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("no 'sub'"))?;
    let iat = number("iat").ok_or_else(|| invalid("no 'iat'"))?;
    let exp = number("exp").ok_or_else(|| invalid("no 'exp'"))?;
    if iat > now + LEEWAY_SECONDS as i64 {
        return Err(invalid("issued in the future"));
    }
    if exp - iat > MAX_LIFETIME_SECONDS {
        return Err(IssuerTokenError::LifetimeTooLong);
    }
    Ok(VerifiedToken {
        iss,
        sub,
        iat,
        exp,
        jti: text("jti").filter(|j| !j.is_empty()),
        claims,
    })
}

/// Verify a token end to end against `keys` — the keys of the registered
/// issuer whose `issuer` is `issuer`.
pub async fn verify_token(
    keys: &JwksCache,
    token: &str,
    issuer: &str,
    audience: &str,
) -> Result<VerifiedToken, IssuerTokenError> {
    let token = token.trim();
    let header = decode_header(token).map_err(|e| IssuerTokenError::Invalid(e.to_string()))?;
    let kid = header.kid.ok_or(IssuerTokenError::UnknownKey)?;
    let key = keys.decoding_key(&kid).await?;
    let now = chrono::Utc::now().timestamp();
    decode_claims(token, &key, issuer, audience, now)
}
