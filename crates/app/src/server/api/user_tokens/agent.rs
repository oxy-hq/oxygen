//! Minting an **agent token**, and what the token routes say about one
//! (API-tokens design, "The agent token (2026-10-07)"; the mint's shape is
//! `oxy_auth::token::agent`).
//!
//! An engineer's AI agent asks for a credential of its own with the PKCE
//! exchange `oxyc login` uses (`cli_login.rs`): the browser, under the
//! engineer's session, approves a `mint` of `kind: "agent"`, and the CLI
//! redeems the code for an `oxy_pat_`. That token is an **ordinary all-access
//! personal token** — org token policies and blocks, the credential cache, the
//! 403 on every session-only route and the audit trail all read it as one —
//! that is the agent's own: named for it, alive for hours, never the CLI's
//! saved login.
//!
//! ## What limits it
//!
//! Exactly what limits an all-access personal token made in Settings, by the
//! same function: both go through [`service::admit_personal`] and
//! [`service::mint_personal`]. It is asked at the approval, under the session,
//! and again at the exchange — the code is good for five minutes.
//!
//! ## Standing
//!
//! `standing: true` in the approval lets the token carry its owner's staff and
//! partner standing. What it then carries is what the owner **holds when the
//! code is redeemed** — the login's own rule ([`reach::standing_held`]).
//! Asking for a standing one does not hold is not an error: the token carries
//! none.
//!
//! ## Fixed once minted
//!
//! Nothing edits one: rename, widen, extend and regenerate answer 409
//! `agent_token_fixed` ([`refuse_edit`]). Its owner revokes it in a session,
//! and it may revoke itself, as any token.

use chrono::{DateTime, Utc};
use entity::api_tokens;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::access::Access as AskedAccess;
use oxy_auth::token::agent::{self as mint, MintRequest};
use oxy_auth::token::cli_login;
use oxy_auth::token::credential::source;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::error::TokenError;
use super::reach;
use super::service::{self, Minted, NewPersonal};
use crate::server::authz::{self, PrincipalFacts};

/// The name an agent token gets when its mint sent none.
fn default_name(hostname: &str) -> String {
    format!("agent on {hostname}")
}

fn parse(body: &Value, default_name: Option<&str>) -> Result<MintRequest, TokenError> {
    mint::parse(body, default_name).map_err(|e| TokenError::InvalidAgentToken(e.0))
}

/// The standing a token carries: what was asked for, of what is held.
fn carried(asked: bool, held: (bool, bool)) -> (bool, bool) {
    (asked && held.0, asked && held.1)
}

/// The personal token `request` mints for an owner holding `facts`, were it
/// minted `now`. The audit row names the host, and says whether the approval
/// let it carry a standing — the flags beside it say what it then carried.
fn token_for(
    request: &MintRequest,
    facts: &PrincipalFacts,
    hostname: &str,
    now: DateTime<Utc>,
) -> NewPersonal {
    let (platform, partner) = carried(request.standing, reach::standing_held(facts));
    let mut detail = Map::new();
    detail.insert("hostname".into(), json!(hostname));
    detail.insert("standing_approved".into(), json!(request.standing));
    NewPersonal {
        name: request.name.clone(),
        access: AskedAccess {
            all_access: true,
            platform,
            partner,
            grants: Vec::new(),
        },
        expires_at: Some(request.expires_at(now)),
        source: source::OXYC_AGENT,
        detail,
    }
}

/// `POST /auth/cli/authorize` with a `mint` of `kind: "agent"`: issue a code
/// that mints the token. The mint is checked here, under the session — its
/// shape, and everything a new all-access personal token is held to — and the
/// code stores what was approved. The exchange can only answer `invalid_code`,
/// so this is the last place a refusal can say why.
pub(super) async fn authorize(
    db: &DatabaseConnection,
    actor: &RequestActor,
    challenge: &str,
    hostname: &str,
    body: &Value,
) -> Result<String, TokenError> {
    let request = parse(body, Some(&default_name(hostname)))?;
    let facts = reach::facts(db, &authz::caller_of(actor)).await?;
    let token = token_for(&request, &facts, hostname, Utc::now());
    service::admit_personal(db, &facts, &token).await?;
    let stored = request.stored();
    Ok(cli_login::authorize_mint(db, actor.id, challenge, hostname, stored).await?)
}

/// Mint the token a code was approved for, as `actor` — the user the code was
/// issued to. Everything is asked again: the approval is up to five minutes
/// old, and the standing is the one held now.
pub(super) async fn redeem(
    db: &DatabaseConnection,
    actor: &RequestActor,
    approved: &Value,
    hostname: &str,
) -> Result<Minted, TokenError> {
    let request = parse(approved, None)?;
    let facts = reach::facts(db, &authz::caller_of(actor)).await?;
    let token = token_for(&request, &facts, hostname, Utc::now());
    service::mint_personal(db, actor, &facts, token).await
}

/// 409 `agent_token_fixed` for an agent token: nothing edits one.
pub(super) fn refuse_edit(row: &api_tokens::Model) -> Result<(), TokenError> {
    if mint::is_agent_token(row) {
        return Err(TokenError::AgentTokenFixed);
    }
    Ok(())
}

/// The limits a mint is held to, for the approval page.
#[derive(Debug, PartialEq, Serialize)]
pub struct AgentLimits {
    pub default_hours: i64,
    pub max_hours: i64,
}

impl AgentLimits {
    pub(super) fn current() -> Self {
        Self {
            default_hours: mint::DEFAULT_HOURS,
            max_hours: mint::MAX_HOURS,
        }
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
