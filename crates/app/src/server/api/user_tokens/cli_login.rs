//! `oxyc login` over HTTP: the PKCE loopback exchange (API-tokens design §6;
//! the code and its single use live in `oxy_auth::token::cli_login`).
//!
//! - `POST /auth/cli/authorize` — the browser, under its **session**, trades
//!   the CLI's S256 challenge for a one-time code. A token cannot ask: a token
//!   never mints a token.
//! - `POST /auth/cli/exchange` — **public**. The CLI trades `code + verifier`
//!   for an `oxy_pat_`. The code is the only thing that names the user, so
//!   every failure answers the same 400 `invalid_code`.
//!
//! The token is `oxyc on <hostname>`: all-access, a year, carrying `platform`
//! and `partner` only where its owner holds that standing. Logging in again
//! from the same host retires the earlier token of that name.
//!
//! ## Minting a sandbox agent token the same way
//!
//! `oxyc tokens create --sandbox-agent` uses the same two routes, because a
//! token is minted under a browser session and the CLI has none. The browser's
//! `authorize` body then carries a `mint` object — the kind, the apps, the
//! lifetime, the name — and:
//!
//! - **who may mint is checked at `authorize`, under the session**: a body the
//!   session may not mint answers there (400 `invalid_sandbox_token`, 404
//!   `app_not_found`) and no code is issued;
//! - the code stores what was approved, and `exchange` mints **that** — an
//!   `oxy_sbx_` — after checking who may mint again, since the code is good
//!   for five minutes;
//! - no earlier token is retired: the `oxyc on <hostname>` replacement rule is
//!   the login's.
//!
//! An `authorize` body with no `mint` is `oxyc login`, exactly as before.

use axum::Json;
use axum::body::Bytes;
use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use entity::prelude::Users;
use entity::users::UserStatus;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::cli_login::{self, Redeemed};
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, LOGIN_LIFETIME_DAYS, NewToken};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::ManageTokens;
use super::access_audit::{self, Access};
use super::audit::{self, Event};
use super::error::TokenError;
use super::handlers::{TokenWithSecret, parse};
use super::service::{self, Minted};
use super::{reach, sandbox, view};
use crate::server::authz;

/// Why an earlier `oxyc on <hostname>` token was revoked.
const SUPERSEDED: &str = "superseded by a new oxyc login";

#[derive(Deserialize)]
pub struct AuthorizeBody {
    code_challenge: String,
    hostname: String,
    /// What the code mints instead of the login token. Absent for `oxyc login`.
    mint: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct AuthorizeResponse {
    pub code: String,
}

#[derive(Deserialize)]
pub struct ExchangeBody {
    code: String,
    code_verifier: String,
}

/// `POST /auth/cli/authorize` — issue a one-time code for the session's user,
/// bound to the CLI's challenge. Five minutes, single use.
pub async fn authorize(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    body: Bytes,
) -> Result<Json<AuthorizeResponse>, TokenError> {
    let body: AuthorizeBody = parse(&body)?;
    let challenge = cli_login::clean_challenge(&body.code_challenge).ok_or_else(|| {
        TokenError::Invalid("'code_challenge' must be a base64url S256 challenge".into())
    })?;
    let hostname = cli_login::clean_hostname(&body.hostname)
        .ok_or_else(|| TokenError::Invalid("'hostname' must be 1 to 255 characters".into()))?;
    let db = establish_connection().await?;
    let code = match &body.mint {
        None => cli_login::authorize(&db, actor.id, &challenge, &hostname).await?,
        Some(mint) => authorize_mint(&db, &actor, &challenge, &hostname, mint).await?,
    };
    Ok(Json(AuthorizeResponse { code }))
}

/// The name a CLI-minted sandbox agent token gets when the CLI sent none.
fn sandbox_token_name(hostname: &str) -> String {
    format!("sandbox agent on {hostname}")
}

/// Issue a code that mints a sandbox agent token. The mint is checked here,
/// under the session — its shape, who may mint for each app, and each org's
/// lifetime cap — and the code stores it with the apps resolved to ids. The
/// exchange can only answer `invalid_code`, so this is the last place a
/// refusal can say why.
async fn authorize_mint(
    db: &DatabaseConnection,
    actor: &RequestActor,
    challenge: &str,
    hostname: &str,
    mint: &serde_json::Value,
) -> Result<String, TokenError> {
    let default_name = sandbox_token_name(hostname);
    let checked = sandbox::check(db, actor, mint, Some(&default_name)).await?;
    sandbox::within_policy(db, &checked, checked.request.expires_at(Utc::now())).await?;
    let stored = checked.request.stored(&checked.app_ids());
    Ok(cli_login::authorize_mint(db, actor.id, challenge, hostname, stored).await?)
}

/// `POST /auth/cli/exchange` — redeem the code and mint the CLI's token.
pub async fn exchange(
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TokenWithSecret>, TokenError> {
    let body: ExchangeBody = parse(&body)?;
    let db = establish_connection().await?;
    let redeemed = cli_login::redeem(&db, &body.code, &body.code_verifier)
        .await?
        .ok_or(TokenError::InvalidCode)?;
    let actor = redeemer(&db, &redeemed, &headers).await?;
    let minted = match &redeemed.mint {
        None => mint(&db, &actor, &redeemed.hostname).await?,
        Some(approved) => mint_sandbox(&db, &actor, approved, &redeemed.hostname).await?,
    };
    Ok(Json(minted.into()))
}

/// Mint the sandbox agent token a code was approved for. Who may mint is
/// checked again: the approval is up to five minutes old. Whatever refuses it
/// now, the CLI is told what it is told for any code it cannot redeem.
async fn mint_sandbox(
    db: &DatabaseConnection,
    actor: &RequestActor,
    approved: &serde_json::Value,
    hostname: &str,
) -> Result<Minted, TokenError> {
    let refused = |e: TokenError| match e {
        TokenError::Internal(detail) => TokenError::Internal(detail),
        other => {
            tracing::info!(reason = ?other, "an approved sandbox agent mint can no longer be honoured");
            TokenError::InvalidCode
        }
    };
    let checked = sandbox::check(db, actor, approved, None)
        .await
        .map_err(refused)?;
    let detail = json!({ "hostname": hostname });
    sandbox::create(db, actor, &checked, source::OXYC, detail)
        .await
        .map_err(refused)
}

/// The user the code was issued to, as the actor of this request. A user
/// deleted since the code was issued redeems nothing.
async fn redeemer(
    db: &DatabaseConnection,
    redeemed: &Redeemed,
    headers: &HeaderMap,
) -> Result<RequestActor, TokenError> {
    let user = Users::find_by_id(redeemed.user_id)
        .one(db)
        .await?
        .filter(|u| u.status == UserStatus::Active)
        .ok_or(TokenError::InvalidCode)?;
    Ok(RequestActor::for_user(user.into(), headers))
}

/// Retire the earlier login from this host and mint its successor, in one
/// transaction: a failed mint leaves the earlier token working.
async fn mint(
    db: &DatabaseConnection,
    actor: &RequestActor,
    hostname: &str,
) -> Result<Minted, TokenError> {
    let facts = reach::facts(db, &authz::caller_of(actor)).await?;
    let (platform, partner) = reach::standing_held(&facts);
    let name = cli_login::token_name(hostname);

    let txn = db.begin().await?;
    let retired = retire_earlier(&txn, actor, &name).await?;
    let new = NewToken {
        user_id: actor.id,
        name,
        all_access: true,
        platform,
        partner,
        grants: Vec::new(),
        expires_at: Some(Utc::now() + Duration::days(LOGIN_LIFETIME_DAYS)),
        source: source::OXYC_LOGIN,
    };
    let minted = personal::create(&txn, new).await?;
    // All access: the token holds no grant, so every row lists none.
    let access = Access::of(&minted.row, &[]);
    Event {
        action: audit::CREATED,
        token: &minted.row,
        orgs: service::reach_of(&txn, &minted.row).await?,
        detail: json!({
            "expires_at": audit::rfc3339(minted.row.expires_at),
            "hostname": hostname,
        }),
        change: None,
    }
    .record_with(&txn, actor, access_audit::created(&access))
    .await?;
    txn.commit().await?;

    for token_id in retired {
        service::invalidate(token_id);
    }
    Ok(Minted {
        token: view::token(db, &minted.row, actor.label()).await?,
        secret: minted.secret,
    })
}

/// Revoke every live `oxyc login` token of this name the user owns. Returns
/// their ids, to drop from the credential cache once the transaction commits.
async fn retire_earlier<C: ConnectionTrait>(
    txn: &C,
    actor: &RequestActor,
    name: &str,
) -> Result<Vec<Uuid>, TokenError> {
    let earlier = personal::live_named(txn, actor.id, name, source::OXYC_LOGIN).await?;
    let mut retired = Vec::with_capacity(earlier.len());
    for row in earlier {
        let Some(token) = personal::revoke(txn, row, actor.id, SUPERSEDED).await? else {
            continue;
        };
        Event {
            action: audit::REVOKED,
            token: &token,
            orgs: service::reach_of(txn, &token).await?,
            detail: json!({ "reason": SUPERSEDED }),
            change: None,
        }
        .record(txn, actor)
        .await?;
        retired.push(token.id);
    }
    Ok(retired)
}
