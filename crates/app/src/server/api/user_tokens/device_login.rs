//! `oxyc … --no-browser` over HTTP: the device-code flow (RFC 8628 in shape,
//! PKCE-bound; the store is `oxy_auth::token::device_login`).
//!
//! For a CLI where no browser can reach a loopback listener — a cloud
//! container, a CI runner, a remote dev box. The person approves on any
//! device; nothing travels back through their browser.
//!
//! - `POST /auth/cli/device` — **public**, rate-limited. The CLI starts a
//!   request: its S256 challenge, its hostname, and its `ask` — the
//!   `/cli-auth` parameters the loopback flow would have put in the URL. It is
//!   handed a device code (secret, for polling) and a user code (for the page).
//! - `GET /auth/cli/device/requests/{user_code}` — the page, under the
//!   **session**, reads the request to lay it out exactly as `/cli-auth` does.
//! - `POST …/{user_code}/approve` — under the session, with the same `mint`
//!   body `POST /auth/cli/authorize` takes. It is checked by the same function
//!   ([`cli_login::approval`]); a refusal is answered here, where the page can
//!   say it, **and** recorded on the request, so the CLI stops with the same
//!   reason. `…/deny` records the person's own refusal.
//! - `POST /auth/cli/device/poll` — **public**, rate-limited. The CLI polls
//!   with the device code and its PKCE verifier, and collects the approval
//!   once: minted by the same function the loopback exchange mints with
//!   ([`cli_login::mint_redeemed`]), so who may mint is asked again, and the
//!   `token.created` row records `approval: device_code` and the approver.
//!
//! **Naming who approves.** `oxyc --approver <email>` puts `approver` in the
//! start body. An email that resolves to an active user binds the request to
//! them — the three page routes answer 404 to anyone else, as for a wrong code
//! — and tells them through the notification machine (`oxy.cli_approval`:
//! push, email, Slack, the inbox), best-effort after the row is stored. The
//! notice links to the page and never carries the code: the person types it
//! from the terminal that ran `oxyc`. The answer to the start is identical
//! whether the email resolved or not, so the asker learns who has an account
//! from nothing.
//!
//! **A device-code token never carries staff or partner standing.** The
//! loopback code only ever reaches the approver's own machine; a device code
//! is collected by whoever started the request, and an approver who was sent
//! the link cannot tell who that is (RFC 8628 §5.4). So a login collected
//! here is minted without `platform` and `partner`, and an agent token's
//! approval may not ask for them ([`asks_for_standing`]). Phishing an approver
//! still yields what they can reach as a member — never their staff reach.
//!
//! **Only a browser session decides.** The three page routes take
//! `SessionOnly`, as `authorize` does: a credential never approves a
//! credential.

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::agent as agent_mint;
use oxy_auth::token::cli_login::{self as code_store, Redeemed};
use oxy_auth::token::device_login::{self, Pending, Polled};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::ManageTokens;
use super::cli_login::{self, Collected};
use super::error::TokenError;
use super::handlers::{TokenWithSecret, parse};
use crate::server::api::public_rate_limit::{self, PublicRoute};
use oxy_app_core::notifications::platform::cli_approval::notify_cli_approval;

/// What a person's own refusal records, for the CLI to print.
const DECLINED: &str = "the request was declined in the browser";

#[derive(Deserialize)]
pub struct StartBody {
    code_challenge: String,
    hostname: String,
    /// The `/cli-auth` parameters. Absent for `oxyc login`.
    ask: Option<Value>,
    /// Who should approve: an email. Absent when the CLI named nobody.
    approver: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StartResponse {
    pub device_code: String,
    /// `BCDF-GHJK`: what the CLI prints and the page shows.
    pub user_code: String,
    pub expires_in: i64,
    pub interval: i64,
}

/// The request as the page lays it out.
#[derive(Debug, Serialize)]
pub struct DeviceRequestView {
    pub user_code: String,
    pub hostname: String,
    pub ask: Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ApproveBody {
    /// What the page approved, as `POST /auth/cli/authorize` takes it. Absent
    /// for a login.
    mint: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct Decided {
    pub status: &'static str,
}

#[derive(Deserialize)]
pub struct PollBody {
    device_code: String,
    code_verifier: String,
}

/// What a poll answers when it does not refuse.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollResponse {
    Pending {
        interval: i64,
    },
    SlowDown {
        interval: i64,
    },
    /// `{ "status": "approved", "token": …, "secret": … }` — the exchange's
    /// own body, so the CLI reads it the same way.
    Approved(Box<TokenWithSecret>),
}

fn rate_limited(route: PublicRoute, headers: &HeaderMap) -> Result<(), TokenError> {
    match public_rate_limit::check(route, headers) {
        Some(retry_after_secs) => Err(TokenError::RateLimited { retry_after_secs }),
        None => Ok(()),
    }
}

/// `POST /auth/cli/device` — open a request. Ten minutes to decide.
pub async fn start(headers: HeaderMap, body: Bytes) -> Result<Json<StartResponse>, TokenError> {
    rate_limited(PublicRoute::DeviceStart, &headers)?;
    let body: StartBody = parse(&body)?;
    let challenge = code_store::clean_challenge(&body.code_challenge).ok_or_else(|| {
        TokenError::Invalid("'code_challenge' must be a base64url S256 challenge".into())
    })?;
    let hostname = code_store::clean_hostname(&body.hostname)
        .ok_or_else(|| TokenError::Invalid("'hostname' must be 1 to 255 characters".into()))?;
    let ask = device_login::clean_ask(body.ask.as_ref()).map_err(TokenError::Invalid)?;
    let approver = body.approver.as_deref().map(clean_approver).transpose()?;
    let db = establish_connection().await?;
    // A named approver always binds the request: to the person, or — when the
    // address names nobody — to a sentinel no session matches, so the asker
    // learns nothing from who may open it.
    let approver_id = match approver {
        Some(email) => Some(
            device_login::resolve_approver(&db, &email)
                .await?
                .unwrap_or(device_login::UNRESOLVED_APPROVER),
        ),
        None => None,
    };
    let started = device_login::start(&db, &challenge, &hostname, ask, approver_id).await?;
    if let Some(user_id) = approver_id.filter(|id| *id != device_login::UNRESOLVED_APPROVER) {
        tell_approver(db, user_id, started.user_code.clone(), started.created_at);
    }
    Ok(Json(StartResponse {
        device_code: started.device_code,
        user_code: device_login::display_user_code(&started.user_code),
        expires_in: started.expires_in,
        interval: started.interval,
    }))
}

/// `approver` as the start takes it: trimmed, or a 400 when it cannot be an
/// email.
fn clean_approver(raw: &str) -> Result<String, TokenError> {
    device_login::clean_approver(raw).ok_or_else(|| {
        TokenError::Invalid("'approver' must be an email address of at most 254 characters".into())
    })
}

/// The most live requests one anonymous caller can have waiting on a person,
/// and so the most notices they can aim at them before the rest go unsent.
///
/// The cap is keyed on the person, not the caller, so a flooder can mute the
/// notices to someone for as long as it keeps three requests pending (each
/// lapses in ten minutes). That is the chosen side: a notice is a convenience
/// and the code the CLI printed still works on the page, while the flood it
/// prevents goes out on every channel in Oxygen's name. The per-IP rate limit
/// on the start bounds the keeping-up.
const MAX_NOTICES_PER_APPROVER: u64 = 3;

/// Tell the approver, off the request's own clock: the start answers the same
/// whether or not the address named anyone, so nothing here may add to its
/// time. A short write that then queues delivery, never a reason to fail the
/// start — the CLI still prints the code and the person can be handed the page.
fn tell_approver(db: DatabaseConnection, user_id: Uuid, user_code: String, as_of: DateTime<Utc>) {
    tokio::spawn(async move {
        let bound = match device_login::bound_pending(&db, user_id, as_of).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "could not count cli device requests");
                return;
            }
        };
        if bound > MAX_NOTICES_PER_APPROVER {
            tracing::info!(approver = %user_id, bound, "cli approval notice withheld: over the pending cap");
            return;
        }
        if let Err(e) = notify_cli_approval(&db, user_id, &user_code).await {
            tracing::warn!(error = %e, "could not notify the named approver of a cli device request");
        }
    });
}

/// The pending request a user code names, for `user_id`: 404 for a code that
/// is malformed, unknown, decided or lapsed — and for a request bound to
/// another approver. One answer, so the page cannot probe.
async fn find(db: &DatabaseConnection, raw: &str, user_id: Uuid) -> Result<Pending, TokenError> {
    let code = device_login::clean_user_code(raw).ok_or(TokenError::NotFound)?;
    device_login::pending(db, &code)
        .await?
        .filter(|pending| pending.may_decide(user_id))
        .ok_or(TokenError::NotFound)
}

/// `GET /auth/cli/device/requests/{user_code}` — the request, for the page.
pub async fn describe(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(user_code): Path<String>,
) -> Result<Json<DeviceRequestView>, TokenError> {
    let db = establish_connection().await?;
    let pending = find(&db, &user_code, actor.id).await?;
    Ok(Json(DeviceRequestView {
        user_code: device_login::display_user_code(&pending.user_code),
        hostname: pending.hostname,
        ask: pending.ask,
        created_at: pending.created_at,
        expires_at: pending.expires_at,
    }))
}

/// Whether `mint` is the kind of token `ask` asked for: a login only as a
/// login, and each mint only as its own kind. The page never sends another;
/// checked so that no body can turn an agent's request into something wider.
fn mint_matches_ask(ask: &Value, mint: Option<&Value>) -> bool {
    match (ask.get("kind").and_then(Value::as_str), mint) {
        (None, None) => true,
        (Some("agent"), Some(mint)) => agent_mint::asked_for(mint),
        (Some("sandbox_agent"), Some(mint)) => !agent_mint::asked_for(mint),
        _ => false,
    }
}

/// Whether an approval asks for the approver's staff or partner standing,
/// which no device-code token carries.
fn asks_for_standing(mint: Option<&Value>) -> bool {
    mint.and_then(|mint| mint.get("standing")) == Some(&Value::Bool(true))
}

/// What a refusal under the session records for the CLI. An app that is not
/// mintable answers a bare "not found" over HTTP; the CLI is told more.
fn denial_reason(refusal: &TokenError) -> String {
    match refusal {
        TokenError::AppNotFound(_) => {
            "an app it names is not one the approver may create sandbox agent tokens for".into()
        }
        other => other.message(),
    }
}

/// `POST /auth/cli/device/requests/{user_code}/approve` — approve as the
/// session's user. A refusal of the mint also refuses the request.
pub async fn approve(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(user_code): Path<String>,
    body: Bytes,
) -> Result<Json<Decided>, TokenError> {
    let body: ApproveBody = parse(&body)?;
    let db = establish_connection().await?;
    let pending = find(&db, &user_code, actor.id).await?;
    if !mint_matches_ask(&pending.ask, body.mint.as_ref()) {
        return Err(TokenError::Invalid(
            "the approval is not for the kind of token the request asked for".into(),
        ));
    }
    if asks_for_standing(body.mint.as_ref()) {
        return Err(TokenError::InvalidAgentToken(
            "a token approved by device code never carries staff or partner access".into(),
        ));
    }
    let checked = cli_login::approval(&db, &actor, &pending.hostname, body.mint.as_ref()).await;
    match checked {
        Ok(stored) => {
            if !device_login::approve(&db, &pending, actor.id, stored).await? {
                return Err(TokenError::NotFound);
            }
            Ok(Json(Decided {
                status: device_login::APPROVED,
            }))
        }
        Err(e @ TokenError::Internal(_)) => Err(e),
        Err(refusal) => {
            device_login::deny(&db, &pending.user_code, actor.id, &denial_reason(&refusal)).await?;
            Err(refusal)
        }
    }
}

/// `POST /auth/cli/device/requests/{user_code}/deny` — the person declines.
pub async fn deny(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(user_code): Path<String>,
) -> Result<Json<Decided>, TokenError> {
    let db = establish_connection().await?;
    let pending = find(&db, &user_code, actor.id).await?;
    if !device_login::deny(&db, &pending.user_code, actor.id, DECLINED).await? {
        return Err(TokenError::NotFound);
    }
    Ok(Json(Decided {
        status: device_login::DENIED,
    }))
}

/// What the `token.created` row adds for a token collected by polling.
fn via_device_code(redeemed: &Redeemed) -> Map<String, Value> {
    let mut via = Map::new();
    via.insert("approval".into(), json!("device_code"));
    via.insert("approver".into(), json!(redeemed.user_id));
    via
}

/// `POST /auth/cli/device/poll` — the CLI polls, and collects an approval
/// once. A refusal says why; a lapse and an unknown code each say so; a wrong
/// verifier is `invalid_code`, and the request is spent.
pub async fn poll(headers: HeaderMap, body: Bytes) -> Result<Json<PollResponse>, TokenError> {
    rate_limited(PublicRoute::DevicePoll, &headers)?;
    let body: PollBody = parse(&body)?;
    let db = establish_connection().await?;
    let interval = device_login::POLL_INTERVAL_SECS;
    match device_login::poll(&db, &body.device_code, &body.code_verifier).await? {
        Polled::Pending => Ok(Json(PollResponse::Pending { interval })),
        Polled::SlowDown => Ok(Json(PollResponse::SlowDown { interval })),
        Polled::Denied(reason) => Err(TokenError::AccessDenied(reason)),
        Polled::Expired => Err(TokenError::ExpiredRequest),
        Polled::Invalid => Err(TokenError::InvalidCode),
        Polled::Approved(redeemed) => {
            // Never the approver's standing: see the module docs.
            let collected = Collected {
                via: via_device_code(&redeemed),
                standing: false,
            };
            let minted = cli_login::mint_redeemed(&db, &redeemed, &headers, collected).await?;
            Ok(Json(PollResponse::Approved(Box::new(minted.into()))))
        }
    }
}

#[cfg(test)]
#[path = "device_login_tests.rs"]
mod tests;
