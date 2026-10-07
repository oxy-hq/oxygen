//! A browser session opened with a personal access token.
//!
//! Something that holds a token but is not a person — a coding agent driving
//! Playwright, a post-deploy check — cannot finish an OAuth consent screen or
//! read a magic-link inbox, so on a deployment it had no way to a signed-in
//! browser. This is that way, in two steps:
//!
//! 1. the token asks for a **ticket** ([`issue`]): single use, five minutes,
//!    only its hash stored;
//! 2. a browser redeems the ticket ([`redeem`]) and is handed a **token
//!    session** ([`mint`]): a JWT the web app and the `oxy_session` cookie
//!    carry exactly as they carry a login's.
//!
//! ## The session is the token
//!
//! A token session authenticates as the token's own `CredentialContext`
//! (`dispatch::authenticate_browser_session`), so it decides every request as
//! the token would as a bearer: the same grants and ceilings, the same refusal
//! on every session-only route, the same row in the audit log. Nothing here
//! mints authority — it changes how an existing credential is carried, from a
//! header an agent can set to the cookie and `localStorage` entry a browser
//! needs. Revoking the token ends its sessions with it, and a session never
//! outlives [`SESSION_TTL_SECS`] or the token.
//!
//! That is also why a ticket is worth having instead of putting the token in
//! the URL: what an agent reads back and pastes into a navigation is spent in
//! seconds, and the long-lived secret stays in its environment.
//!
//! ## Signed with the token's own key
//!
//! The JWT is HS256 under a key derived from `api_tokens.token_hash`
//! ([`signing_key`]), and names the token in its `kid`. So:
//!
//! - it can be minted only by something that holds the token's row — knowing a
//!   token's id, which every audit row shows, is not enough;
//! - a binary one release back, which knows one session key and no `kid`,
//!   fails its signature and answers 401. It cannot mistake a token session
//!   for a login and hand it the user's whole reach.
//!
//! ## The ticket lives in `cli_auth_codes`
//!
//! It is the same thing as an `oxyc login` code — a five-minute, single-use
//! handoff between a CLI and a browser — pointed the other way, so it shares
//! the table and its sweep. It is stored under its own hash
//! ([`TICKET_DOMAIN`]), for the reason a mint code is (`cli_login`): the login
//! exchange, in this release or the last, never computes that hash, so a
//! ticket can not be redeemed there for a token.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use entity::prelude::ApiTokens;
use entity::{api_tokens, cli_auth_codes};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::cli_login;
use super::format::TokenFormat;
use super::store::{Resolved, invalid, resolve_row};

/// The longest a token session lasts: a working day of an agent's loop, not
/// the thirty days a person's login gets. The token's own expiry caps it
/// further ([`mint`]).
pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;

/// What the `kid` of a token session starts with; the token's id follows.
const KID_PREFIX: &str = "tok:";
/// What a token's hash is prefixed with before it is hashed into its key.
const KEY_DOMAIN: &[u8] = b"oxy-browser-session-key:v1:";
/// What a ticket is prefixed with before it is hashed. See the module docs.
const TICKET_DOMAIN: &str = "oxy-browser-ticket:";
/// `cli_auth_codes.mint.kind` of a ticket's row.
const TICKET_KIND: &str = "browser_session";

/// `sub`, `email`, `exp` and `iat` are a login session's claims, so the web
/// app reads a token session as it reads any other. `tid` is the token.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    exp: usize,
    iat: usize,
    tid: Uuid,
}

/// The HS256 key of one token's sessions. `token_hash` is the SHA-256 of 256
/// random bits, so the key is as unguessable as the token.
fn signing_key(token_hash: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(KEY_DOMAIN);
    hasher.update(token_hash);
    hasher.finalize().into()
}

/// A freshly minted token session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub jwt: String,
    pub expires_at: DateTime<Utc>,
}

impl Session {
    /// How long the browser should keep the cookie: until the JWT lapses and
    /// no longer, so it never presents a credential the server will refuse.
    pub fn max_age_secs(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds().max(0)
    }
}

/// Mint a session for `row`, acting as `email`'s user. The caller has resolved
/// the row as a live personal token; this only signs.
pub fn mint(row: &api_tokens::Model, email: &str, now: DateTime<Utc>) -> Result<Session, OxyError> {
    let cap = now + Duration::seconds(SESSION_TTL_SECS);
    let lapses = row
        .expires_at
        .map(DateTime::<Utc>::from)
        .map_or(cap, |at| at.min(cap));
    let claims = Claims {
        sub: row.principal_user_id.to_string(),
        email: email.to_string(),
        exp: lapses.timestamp().max(0) as usize,
        iat: now.timestamp().max(0) as usize,
        tid: row.id,
    };
    let header = Header {
        kid: Some(format!("{KID_PREFIX}{}", row.id)),
        ..Header::default()
    };
    let key = EncodingKey::from_secret(&signing_key(&row.token_hash));
    let jwt = encode(&header, &claims, &key)
        .map_err(|e| OxyError::RuntimeError(format!("sign a token session: {e}")))?;
    // What the JWT says, to the second: the cookie's lifetime is read off this.
    let expires_at = DateTime::from_timestamp(claims.exp as i64, 0).unwrap_or(lapses);
    Ok(Session { jwt, expires_at })
}

/// The token a JWT claims to be a session of, read from its `kid` and
/// **trusting nothing**: this only routes the JWT to [`verify`]. `None` for a
/// login session, which carries no `kid`, and for anything that is not a JWT.
pub fn token_id_of(jwt: &str) -> Option<Uuid> {
    let kid = decode_header(jwt).ok()?.kid?;
    Uuid::parse_str(kid.strip_prefix(KID_PREFIX)?).ok()
}

/// Check `jwt` against the token row its `kid` named, and answer when it
/// lapses. Signature, expiry (no leeway: the cookie and the cache are both cut
/// to the same second) and that the claims name this row and its user.
pub fn verify(jwt: &str, row: &api_tokens::Model) -> Result<DateTime<Utc>, &'static str> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    let key = DecodingKey::from_secret(&signing_key(&row.token_hash));
    let claims = decode::<Claims>(jwt, &key, &validation)
        .map_err(|_| "a token session that is expired or not signed by its token")?
        .claims;
    if claims.tid != row.id || claims.sub != row.principal_user_id.to_string() {
        return Err("a token session that names another token");
    }
    DateTime::from_timestamp(claims.exp as i64, 0).ok_or("a token session with no expiry")
}

/// Resolve the token session `jwt`, which claims to be a session of
/// `token_id`. The row is found by that id, the JWT is checked against the
/// row's own key, and the row is then admitted exactly as its `oxy_pat_` would
/// be as a bearer — so a revoked, expired, narrowed or blocked token decides
/// its sessions as it decides itself.
///
/// Only a personal token opens a session, and never a legacy credential: a
/// key that mirrors an `api_keys` row gains nothing it did not have (API-tokens
/// design §3.5). The answer expires with the JWT when that is sooner than the
/// token.
pub(super) async fn resolve(
    db: &DatabaseConnection,
    jwt: &str,
    token_id: Uuid,
) -> Result<Resolved, OxyError> {
    let row = ApiTokens::find_by_id(token_id)
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("api token lookup: {e}")))?
        .ok_or_else(|| invalid("no token row for a token session"))?;
    let lapses = verify(jwt, &row).map_err(invalid)?;
    let mut resolved = resolve_row(db, row, TokenFormat::Personal).await?;
    if resolved.credential.is_legacy() {
        return Err(invalid("a legacy credential opens no browser session"));
    }
    resolved.expires_at = Some(resolved.expires_at.map_or(lapses, |at| at.min(lapses)));
    Ok(resolved)
}

/// What a request's token session is cached under: a hash no presented key or
/// token can collide with, because a header value cannot hold the NUL bytes.
pub(super) fn cache_key(jwt: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"\0oxy-browser-session\0");
    hasher.update(jwt.as_bytes());
    hasher.finalize().to_vec()
}

/// A ticket, as handed to the token that asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub ticket: String,
    pub expires_at: DateTime<Utc>,
}

fn hash_ticket(ticket: &str) -> Vec<u8> {
    Sha256::digest(format!("{TICKET_DOMAIN}{ticket}").as_bytes()).to_vec()
}

/// Issue a ticket that opens a session of `token_id`, which acts as `user_id`.
/// The caller has checked that the request authenticated with that token.
pub async fn issue<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    token_id: Uuid,
) -> Result<Ticket, OxyError> {
    let now = Utc::now();
    cli_login::sweep(db, now).await;
    let ticket = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let expires_at = now + Duration::seconds(cli_login::CODE_TTL_SECS);
    cli_auth_codes::ActiveModel {
        code_hash: Set(hash_ticket(&ticket)),
        user_id: Set(user_id),
        // No PKCE: whoever is handed the ticket is who it is for. Empty is a
        // challenge no verifier hashes to, were the row ever found as a code.
        code_challenge: Set(String::new()),
        hostname: Set(String::new()),
        created_at: Set(now.fixed_offset()),
        expires_at: Set(expires_at.fixed_offset()),
        consumed_at: Set(None),
        mint: Set(Some(json!({ "kind": TICKET_KIND, "token_id": token_id }))),
    }
    .insert(db)
    .await
    .map_err(|e| OxyError::DBError(format!("store browser ticket: {e}")))?;
    Ok(Ticket { ticket, expires_at })
}

/// The token a ticket opens a session of. `None` for every failure — unknown,
/// spent, expired, or a row that is not a ticket — which the caller answers
/// identically. Spent **first**, as a login code is: two redemptions cannot
/// both win, and a failed one still burns it.
pub async fn redeem<C: ConnectionTrait>(db: &C, ticket: &str) -> Result<Option<Uuid>, OxyError> {
    let now = Utc::now();
    let Some(row) = cli_login::spend(db, hash_ticket(ticket), now).await? else {
        return Ok(None);
    };
    if DateTime::<Utc>::from(row.expires_at) <= now {
        return Ok(None);
    }
    Ok(ticketed_token(row.mint.as_ref()))
}

/// The token a row's `mint` names, when the row is a ticket's.
fn ticketed_token(mint: Option<&Value>) -> Option<Uuid> {
    let mint = mint?;
    if mint.get("kind")?.as_str()? != TICKET_KIND {
        return None;
    }
    Uuid::parse_str(mint.get("token_id")?.as_str()?).ok()
}

#[cfg(test)]
#[path = "browser_session_tests.rs"]
mod tests;
