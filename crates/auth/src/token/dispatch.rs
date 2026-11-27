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
//! 0. **A service token** (`oxysvc_`, [`super::service`]) in `Authorization:
//!    Bearer` or `X-API-Key` decides the request: the service its hash names,
//!    or 401. Read from configuration, never from the database or the cache.
//! 1. **A new-prefix token** (`oxy_pat_`/`oxy_sat_`/`oxy_ci_`/`oxy_sbx_`) in
//!    `Authorization: Bearer` or `X-API-Key` decides the request. If it fails,
//!    the answer is 401 — it never falls through to a cookie. That is new, and
//!    deliberately limited to new prefixes: nothing sends one today.
//! 2. **A session** (Session surface only): the `Authorization` JWT, else the
//!    `oxy_session` cookie — exactly today's `BuiltInAuthenticator` order.
//!    A **token session** — the JWT a browser holds after redeeming a
//!    token's ticket (`super::browser_session`) — is read from the same two
//!    places and decides as its token does: the request carries the token's
//!    `CredentialContext`, and a session that fails answers 401.
//! There is no fourth step. **Legacy API keys (`oxy_<32 hex>`) were removed**:
//! `X-API-Key` still carries a token — that is how `/external/api` is reached —
//! but a value of the old shape authenticates nothing. It is recognised only to
//! answer 401 with [`LEGACY_KEY_REMOVED`] instead of "no API key found", so an
//! integration that has been sending one for a year learns what happened.
//!
//! A **sandbox agent token** (`oxy_sbx_`) is refused wherever the entry point
//! has not said it may authenticate there ([`SandboxAgent`]) — before its row
//! is read. It is never served from the credential cache: a revoked or expired
//! one, a revoked grant and a deactivated minter stop it on the next request,
//! on every pod (sandbox agent credential design §4).
//!
//! `oxypublish_` bearers are not handled here. `auth_middleware` routes them to
//! the publish-token path before calling this, exactly as before; everywhere
//! else they fail as they always have (they are not a JWT and match no key).

use crate::ports::{AuthDb, AuthPlatform};
use axum::http::HeaderMap;
use oxy_shared::errors::OxyError;

use super::browser_session;
use super::cache;
use super::credential::CredentialContext;
use super::format::{
    TokenFormat, hash_token, is_removed_legacy_key, new_prefix_format, verify_checksum,
};
use super::service;
use super::store;
use crate::api_key_infra::require_active_owner;
use crate::built_in::{auth_configured, extract_session_cookie, guest_identity, session_identity};
use crate::constants::{AUTHENTICATION_HEADER_KEY, DEFAULT_API_KEY_HEADER};
use crate::session_key::{self, Purpose};
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

/// Whether an entry point admits a **sandbox agent token** (`oxy_sbx_`).
///
/// An argument with no default, so each call site states its answer and a new
/// one does not compile until it chooses (sandbox agent credential design
/// §3.1). `Refuse` answers the token 401, exactly as an unknown one.
///
/// Three entry points pass `Admit`: the protected `/api` tree, `/fn` and
/// `/logs`. Each sits behind the route allow-list (`app_grant_scope`); without
/// it the token would reach every route its minter's staff standing does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxAgent {
    Admit,
    Refuse,
}

/// Who the request is, and — when a key or token said so — which credential.
pub type Authenticated = (Identity, Option<CredentialContext>);

/// Authenticate a request's headers on `surface`. See the module docs for the
/// order. `sandbox_agent` is whether this entry point admits an `oxy_sbx_`
/// token; it changes nothing for any other credential.
pub async fn authenticate_request(
    auth: &AuthPlatform,
    headers: &HeaderMap,
    surface: AuthSurface,
    sandbox_agent: SandboxAgent,
) -> Result<Authenticated, OxyError> {
    // Zero-config local installs run as the guest, keys or no keys, as before.
    if surface == AuthSurface::Session && !auth_configured() {
        return Ok((guest_identity(), None));
    }

    if let Some(token) = service_token(headers) {
        return authenticate_service(&token);
    }

    if let Some((token, format)) = new_prefix_token(headers) {
        refuse_sandbox_agent(format, sandbox_agent)?;
        let (identity, credential) = authenticate_token(auth, &token, format).await?;
        return Ok((identity, Some(credential)));
    }

    if surface == AuthSurface::Session {
        if let Some(jwt) = session_jwt(headers)
            && browser_session::token_id_of(&jwt).is_some()
        {
            let (identity, credential) = authenticate_browser_session(auth, &jwt).await?;
            return Ok((identity, Some(credential)));
        }
        match login_session(auth, headers).await {
            Ok(identity) => return Ok((identity, None)),
            Err(err) => tracing::debug!("JWT validation failed, will try API key: {}", err),
        }
    }

    if presents_removed_legacy_key(headers) {
        tracing::info!("refused a removed legacy API key (oxy_<32 hex>)");
        return Err(OxyError::AuthenticationError(
            LEGACY_KEY_REMOVED.to_string(),
        ));
    }
    Err(OxyError::AuthenticationError(format!(
        "No API key found in headers (expected: {DEFAULT_API_KEY_HEADER})"
    )))
}

/// What a caller still sending `oxy_<32 hex>` is told. Names the replacement,
/// because the whole point of recognising a removed format is to shorten the
/// distance between the 401 and the fix.
pub const LEGACY_KEY_REMOVED: &str = "Legacy API keys (oxy_…) were removed. Mint an API token instead, under \
     Settings → Organization → API tokens: a personal access token, or a \
     service account's token. Both are sent the same way, in Authorization: \
     Bearer or X-API-Key.";

/// Whether the request's only credential is a removed legacy key, by shape
/// alone — no database read, and nothing is admitted either way.
fn presents_removed_legacy_key(headers: &HeaderMap) -> bool {
    [bearer(headers), api_key_header(headers)]
        .into_iter()
        .flatten()
        .any(|value| is_removed_legacy_key(&value))
}

/// Whether the request presents a sandbox agent token (`oxy_sbx_`), judged by
/// its prefix alone. For a route fence that must decide before authentication
/// runs (the custom-app serve tree); it reads no database and trusts nothing.
pub fn presents_sandbox_agent(headers: &HeaderMap) -> bool {
    matches!(
        new_prefix_token(headers),
        Some((_, TokenFormat::SandboxAgent))
    )
}

/// Whether the request presents the **platform checker's** service token: a
/// configured `oxysvc_` service whose role is `app_checker`
/// (`PlatformRole::AppChecker`). Read from the headers and the installed
/// service list, no database: for the custom-app serve tree's fence, which
/// keeps the checker to reading a page before anything else runs, and for
/// naming that credential in the answer to a promote-checks claim. It names
/// the credential and decides nothing: every door the checker passes asks
/// `oxy-authz` (`Action::AppCheck`).
pub fn presents_app_checker(headers: &HeaderMap) -> bool {
    service_token(headers)
        .and_then(|token| service::authenticate(&token))
        .is_some_and(|entry| entry.role == oxy_authz::PlatformRole::AppChecker)
}

/// Whether the request presents a **new-format API token** — `oxy_pat_`,
/// `oxy_sat_`, `oxy_ci_` or `oxy_sbx_` — judged by its prefix alone, exactly
/// where [`authenticate_request`] looks for one. False for a session, an
/// anonymous request, a removed legacy key and an `oxypublish_` bearer.
///
/// For a path that authenticates inside its handler and wants to know, before
/// it does and with no database read, whether there will be a token to count
/// the request against. It says nothing about whether the token is valid.
pub fn presents_api_token(headers: &HeaderMap) -> bool {
    new_prefix_token(headers).is_some()
}

/// A sandbox agent token at an entry point that does not admit one: 401, the
/// answer an unknown token gets, with no database read.
fn refuse_sandbox_agent(format: TokenFormat, admitted: SandboxAgent) -> Result<(), OxyError> {
    if format == TokenFormat::SandboxAgent && admitted == SandboxAgent::Refuse {
        tracing::debug!("sandbox agent token refused: this entry point does not admit one");
        return Err(OxyError::AuthenticationError("Invalid API key".to_string()));
    }
    Ok(())
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

/// A login session: the JWT [`session_jwt`] finds, verified with this
/// deployment's session key. The key is asked for only when there is a JWT to
/// check, so a request that carries none never waits on it.
async fn login_session(auth: &AuthPlatform, headers: &HeaderMap) -> Result<Identity, OxyError> {
    if session_jwt(headers).is_none() {
        return Err(OxyError::AuthenticationError(
            "Missing or invalid authentication header".to_string(),
        ));
    }
    let key = session_key::decoding_key(auth, Purpose::Session).await?;
    session_identity(auth, headers, &key)
}

/// What a session is read from: the `Authorization` value, else the
/// `oxy_session` cookie — `BuiltInAuthenticator`'s own order.
fn session_jwt(headers: &HeaderMap) -> Option<String> {
    bearer(headers).or_else(|| extract_session_cookie(headers))
}

/// A service token from `Authorization: Bearer`, else from `X-API-Key`.
fn service_token(headers: &HeaderMap) -> Option<String> {
    [bearer(headers), api_key_header(headers)]
        .into_iter()
        .flatten()
        .find(|value| value.starts_with(service::PREFIX))
}

/// The service a token names, as its principal and credential; 401 when it
/// names none — a wrong token, an empty body, or no services configured. No
/// database read, no cache and no recorded use: the service has no row to
/// count against, and its `users` row is read by the caller as any token's is.
fn authenticate_service(token: &str) -> Result<Authenticated, OxyError> {
    let Some(service) = service::authenticate(token) else {
        tracing::debug!("service token refused: it matches no configured service");
        return Err(OxyError::AuthenticationError("Invalid API key".to_string()));
    };
    let identity = Identity {
        user_id: Some(service.user_id()),
        email: String::new(),
        name: Some(service.principal()),
        picture: None,
    };
    Ok((identity, Some(service.credential())))
}

/// A new-prefix token from `Authorization: Bearer`, else from `X-API-Key`.
fn new_prefix_token(headers: &HeaderMap) -> Option<(String, TokenFormat)> {
    [bearer(headers), api_key_header(headers)]
        .into_iter()
        .flatten()
        .find_map(|value| new_prefix_format(&value).map(|format| (value, format)))
}

/// Validate one token: checksum offline, then the ≤30 s cache, then the
/// database. Records the use either way.
///
/// A sandbox agent token skips the cache in both directions: its row, its
/// grants and its minter are read on every request.
async fn authenticate_token(
    auth: &AuthPlatform,
    token: &str,
    format: TokenFormat,
) -> Result<(Identity, CredentialContext), OxyError> {
    if format.is_new() && !verify_checksum(token) {
        return Err(OxyError::AuthenticationError("Invalid API key".to_string()));
    }
    let cached = format != TokenFormat::SandboxAgent;
    let hash = hash_token(token);
    if cached && let Some((identity, credential)) = cache::get(&hash) {
        let credential = live_principal(auth, credential).await?;
        record_use(auth, &credential).await;
        return Ok((identity, credential));
    }

    let db = connect(auth).await?;
    let resolved = store::resolve(&db, token, format).await?;
    if cached {
        cache::put(
            hash,
            resolved.identity.clone(),
            resolved.credential.clone(),
            resolved.expires_at,
        );
    }
    record_use(auth, &resolved.credential).await;
    Ok((resolved.identity, resolved.credential))
}

/// Authenticate a **token session** (`super::browser_session`): the JWT a
/// browser holds after redeeming a personal token's ticket. It decides as that
/// token — the same row, the same grants, the same ≤30 s cache and the same
/// record of use — and `Err` for anything else, a login session included.
///
/// `pub` for the one caller outside the dispatch: cookie hydration, which
/// reads the cookie alone and must not turn a token session into a login.
pub async fn authenticate_browser_session(
    auth: &AuthPlatform,
    jwt: &str,
) -> Result<(Identity, CredentialContext), OxyError> {
    let token_id = browser_session::token_id_of(jwt)
        .ok_or_else(|| OxyError::AuthenticationError("Not a token session".to_string()))?;
    let key = browser_session::cache_key(jwt);
    if let Some((identity, credential)) = cache::get(&key) {
        let credential = live_principal(auth, credential).await?;
        record_use(auth, &credential).await;
        return Ok((identity, credential));
    }

    let db = connect(auth).await?;
    let resolved = browser_session::resolve(&db, jwt, token_id).await?;
    cache::put(
        key,
        resolved.identity.clone(),
        resolved.credential.clone(),
        resolved.expires_at,
    );
    record_use(auth, &resolved.credential).await;
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
async fn live_principal(
    auth: &AuthPlatform,
    credential: CredentialContext,
) -> Result<CredentialContext, OxyError> {
    let token_id = credential.token_id;
    let db = connect(auth).await?;
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

async fn connect(auth: &AuthPlatform) -> Result<sea_orm::DatabaseConnection, OxyError> {
    auth.connect().await.map_err(|e| {
        tracing::error!(
            "Failed to establish database connection for API key validation: {}",
            e
        );
        OxyError::AuthenticationError("Failed to validate API key".to_string())
    })
}

async fn record_use(auth: &AuthPlatform, credential: &CredentialContext) {
    if !cache::claim_touch(credential.token_id) {
        return;
    }
    match auth.connect().await {
        Ok(db) => store::touch_last_used(&db, credential).await,
        Err(e) => tracing::warn!(error = %e, "could not record token use"),
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
