//! `authenticate_request`: the one dispatch behind every entry point.
//!
//! Callers: `auth_middleware` (`/api`), `api_key_only_middleware`
//! (`/external/api`), and `BuiltInAuthenticator::authenticate`, which the six
//! direct callers (custom-app serving, headers, gates and auth, `GET /api/user`,
//! kiosk enrol) go through. One function, so a token behaves the same on every
//! surface (design §4.1).
//!
//! The order, which is the whole contract:
//!
//! 1. **A new-prefix token** (`oxy_pat_`/`oxy_sat_`/`oxy_ci_`) in
//!    `Authorization: Bearer` or `X-API-Key` decides the request. If it fails,
//!    the answer is 401 — it never falls through to a cookie. That is new, and
//!    deliberately limited to new prefixes: nothing sends one today.
//! 2. **A session** (Session surface only): the `Authorization` JWT, else the
//!    `oxy_session` cookie — exactly today's `BuiltInAuthenticator` order.
//! 3. **A legacy key** from `X-API-Key` — today's fallback, so a valid cookie
//!    still beats a bad legacy key — or, newly, an `oxy_<32 hex>` bearer.
//!
//! `oxypublish_` bearers are not handled here. `auth_middleware` routes them to
//! the publish-token path before calling this, exactly as before; everywhere
//! else they fail as they always have (they are not a JWT and match no key).

use axum::http::HeaderMap;
use oxy_platform::db::establish_connection;
use oxy_shared::errors::OxyError;

use super::cache;
use super::credential::CredentialContext;
use super::format::{TokenFormat, hash_token, new_prefix_format, parse_format, verify_checksum};
use super::store;
use crate::api_key_infra::require_active_owner;
use crate::built_in::{auth_configured, guest_identity, session_identity};
use crate::constants::{AUTHENTICATION_HEADER_KEY, DEFAULT_API_KEY_HEADER};
use crate::types::Identity;

/// Which credentials a surface accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSurface {
    /// A session (JWT header or cookie) or a key/token. `/api` and every
    /// direct `BuiltInAuthenticator` caller.
    Session,
    /// A key or token only — never a cookie or session JWT. `/external/api`,
    /// whose `*` CORS is safe precisely because no ambient credential works.
    ApiKeyOnly,
}

/// Who the request is, and — when a key or token said so — which credential.
pub type Authenticated = (Identity, Option<CredentialContext>);

/// Authenticate a request's headers on `surface`. See the module docs for the
/// order.
pub async fn authenticate_request(
    headers: &HeaderMap,
    surface: AuthSurface,
) -> Result<Authenticated, OxyError> {
    // Zero-config local installs run as the guest, keys or no keys, as before.
    if surface == AuthSurface::Session && !auth_configured() {
        return Ok((guest_identity(), None));
    }

    if let Some((token, format)) = new_prefix_token(headers) {
        let (identity, credential) = authenticate_token(&token, format).await?;
        return Ok((identity, Some(credential)));
    }

    if surface == AuthSurface::Session {
        match session_identity(headers) {
            Ok(identity) => return Ok((identity, None)),
            Err(err) => tracing::debug!("JWT validation failed, will try API key: {}", err),
        }
    }

    let Some(key) = legacy_key(headers) else {
        return Err(OxyError::AuthenticationError(format!(
            "No API key found in headers (expected: {DEFAULT_API_KEY_HEADER})"
        )));
    };
    let (identity, credential) = authenticate_token(&key, TokenFormat::LegacyKey).await?;
    Ok((identity, Some(credential)))
}

/// The bearer value with an optional (case-insensitive) `Bearer ` scheme
/// stripped, as `BuiltInAuthenticator` has always read it.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(AUTHENTICATION_HEADER_KEY)
        .and_then(|v| v.to_str().ok())?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    (!token.is_empty()).then(|| token.to_string())
}

fn api_key_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(DEFAULT_API_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A new-prefix token from `Authorization: Bearer`, else from `X-API-Key`.
fn new_prefix_token(headers: &HeaderMap) -> Option<(String, TokenFormat)> {
    [bearer(headers), api_key_header(headers)]
        .into_iter()
        .flatten()
        .find_map(|value| new_prefix_format(&value).map(|format| (value, format)))
}

/// A legacy key: `X-API-Key` as today (any value — rows were matched raw), else
/// a bearer that is exactly `oxy_<32 hex>`. The bearer form is new and strictly
/// additive: before, such a request answered 401.
fn legacy_key(headers: &HeaderMap) -> Option<String> {
    api_key_header(headers)
        .or_else(|| bearer(headers).filter(|b| parse_format(b) == Some(TokenFormat::LegacyKey)))
}

/// Validate one token: checksum offline, then the ≤30 s cache, then the
/// database. Records the use either way.
async fn authenticate_token(
    token: &str,
    format: TokenFormat,
) -> Result<(Identity, CredentialContext), OxyError> {
    if format.is_new() && !verify_checksum(token) {
        return Err(OxyError::AuthenticationError("Invalid API key".to_string()));
    }
    let hash = hash_token(token);
    if let Some((identity, credential)) = cache::get(&hash) {
        let credential = live_principal(credential).await?;
        record_use(&credential).await;
        return Ok((identity, credential));
    }

    let db = connect().await?;
    let resolved = store::resolve(&db, token, format).await?;
    cache::put(
        hash,
        resolved.identity.clone(),
        resolved.credential.clone(),
        resolved.expires_at,
    );
    record_use(&resolved.credential).await;
    Ok((resolved.identity, resolved.credential))
}

/// A cached credential, with its principal re-read. The cache bounds a
/// *token's* revocation to 30 s across the fleet; a **principal** that was
/// switched off stops at once, everywhere, so its rows are never served from
/// memory.
///
/// The same two questions the database path asks, in the same order: is the
/// principal's `users` row still active — for a person, a legacy key's owner
/// and a service account alike — and, for a service account, is the account
/// still enabled.
async fn live_principal(credential: CredentialContext) -> Result<CredentialContext, OxyError> {
    let token_id = credential.token_id;
    let db = connect().await?;
    reread_principal(&db, credential)
        .await
        .inspect_err(|_| cache::invalidate_token(token_id))
}

async fn reread_principal(
    db: &sea_orm::DatabaseConnection,
    credential: CredentialContext,
) -> Result<CredentialContext, OxyError> {
    require_active_owner(db, credential.principal_user_id).await?;
    if credential.is_service_account() {
        return store::refresh_account(db, credential).await;
    }
    Ok(credential)
}

async fn connect() -> Result<sea_orm::DatabaseConnection, OxyError> {
    establish_connection().await.map_err(|e| {
        tracing::error!(
            "Failed to establish database connection for API key validation: {}",
            e
        );
        OxyError::AuthenticationError("Failed to validate API key".to_string())
    })
}

async fn record_use(credential: &CredentialContext) {
    if !cache::claim_touch(credential.token_id) {
        return;
    }
    match establish_connection().await {
        Ok(db) => store::touch_last_used(&db, credential).await,
        Err(e) => tracing::warn!(error = %e, "could not record token use"),
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
