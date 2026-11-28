//! The exchange for a token from an OIDC issuer an org registered (API-tokens
//! design §3.4, *Any OIDC issuer*). [`super::run`] sends a token here when its
//! `iss` is not GitHub's.
//!
//! The client names the service account to act as, by id, exactly as a GitHub
//! Actions run does; it never names a policy. The steps, each failing closed:
//!
//! 1. **The account the request named**, enabled, and its org. No other org
//!    is read.
//! 2. **The issuer**: the one registered in *that* org whose `issuer` is the
//!    token's `iss`, a trailing `/` ignored. None — or no such account — is
//!    401 `unknown_issuer`. The token's `iss` only selects among issuers the
//!    org's admin registered; keys are read from where the registration said
//!    (a `jwks_uri`, or the key set stored inline), never from anything the
//!    token says.
//! 3. **The envelope** (`oxy_auth::workload_identity::verify`): RS256 against
//!    that issuer's keys, this deployment's audience and no other beside it,
//!    `exp`/`nbf`/`iat`, `sub`, and a life of at most an hour.
//! 4. **Single use**, where the issuer asks for it: the token's `jti` is spent
//!    (401 `jti_required` when it has none, `replayed` when it was spent).
//! 5. **The decision**: the account's enabled policies on that issuer. The one
//!    whose conditions the claims meet mints; none is 403
//!    `no_matching_policy`, and more than one is 403 `ambiguous_policy` —
//!    never a choice between them, and never their grants merged.
//! 6. **The mint** (`oxy_auth::token::issuer_trust_mint`): the account, the
//!    issuer and the policy read again under row locks, the grants capped by
//!    the account's standing now, the verified claims on the row, and
//!    `oidc.token_exchanged` in the org's chain — one transaction.

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use oxy_app_core::audit::TxnAuditSink;
use oxy_auth::github_oidc::{ClaimReject, OidcError, jti};
use oxy_auth::token::issuer_trust_mint::{self, IssuerMint};
use oxy_auth::token::issuer_trust_policy::{self, IssuerCandidate};
use oxy_auth::token::oidc_issuer::{self, IssuerKeys};
use oxy_auth::workload_identity::verify::LEEWAY_SECONDS;
use oxy_auth::workload_identity::{self, Pick, VerifiedToken, keys};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde_json::{Value, json};

use super::mint::{EXCHANGED, actor_of};
use super::reject::{JTI_REQUIRED, Rejection, UNKNOWN_ISSUER};
use super::{ExchangeResponse, Failure, Request, audience, response};
use crate::oidc_issuer_fetch::install_key_fetcher;
use crate::user_tokens::audit::Event;

fn unknown_issuer() -> Failure {
    Failure::Refused(Rejection::Envelope(UNKNOWN_ISSUER), None)
}

fn envelope(code: &'static str) -> Failure {
    Failure::Refused(Rejection::Envelope(code), None)
}

/// Spend the token's `jti`, remembered until the token itself has expired.
/// Keyed by the issuer's row, so two issuers' `jti`s never collide.
async fn spend(
    db: &DatabaseConnection,
    issuer: &entity::oidc_issuers::Model,
    token: &VerifiedToken,
) -> Result<(), Failure> {
    let Some(id) = &token.jti else {
        return Err(envelope(JTI_REQUIRED));
    };
    let until = DateTime::<Utc>::from_timestamp(token.exp + LEEWAY_SECONDS as i64, 0)
        .unwrap_or_else(Utc::now)
        .max(Utc::now() + jti::REMEMBER_FOR);
    match jti::burn_until(db, &format!("{}:{id}", issuer.id), until).await {
        Ok(()) => Ok(()),
        Err(OidcError::Replayed) => Err(envelope("replayed")),
        Err(e) => Err(Failure::Internal(format!("{e:?}"))),
    }
}

/// Steps 1 to 5: the policy this token mints from, and its claims.
async fn chosen(
    db: &DatabaseConnection,
    request: &Request,
    iss: &str,
) -> Result<(IssuerCandidate, VerifiedToken), Failure> {
    let Some((account, org_slug)) =
        issuer_trust_policy::live_account(db, request.service_account).await?
    else {
        return Err(unknown_issuer());
    };
    let Some(issuer) = oidc_issuer::find_by_issuer(db, account.org_id, iss).await? else {
        return Err(unknown_issuer());
    };
    let cache = match IssuerKeys::of(&issuer) {
        Some(IssuerKeys::Inline(jwks)) => keys::inline(&jwks).map(std::sync::Arc::new),
        Some(other) => other.jwks_uri().map(|uri| {
            install_key_fetcher();
            keys::keys_for(uri)
        }),
        None => None,
    };
    let Some(cache) = cache else {
        tracing::error!(issuer_id = %issuer.id, "oidc exchange: a registered issuer has no usable keys");
        return Err(envelope("invalid_token"));
    };
    let audience = audience::audience();
    let token = match workload_identity::verify_token(
        &cache,
        &request.token,
        &issuer.issuer,
        &audience,
    )
    .await
    {
        Ok(token) => token,
        Err(e) => {
            tracing::warn!(error = ?e, issuer_id = %issuer.id, "oidc exchange: the token did not verify");
            return Err(envelope(e.code()));
        }
    };
    if issuer.jti_single_use {
        spend(db, &issuer, &token).await?;
    }
    let candidates = issuer_trust_policy::candidates(db, &account, &org_slug, &issuer).await?;
    match workload_identity::pick(&token.claims, &candidates, IssuerCandidate::conditions) {
        Pick::One(candidate) => Ok((candidate.clone(), token)),
        Pick::None => Err(Failure::Refused(
            Rejection::Claims(ClaimReject::NoMatchingPolicy),
            Some(token.recorded()),
        )),
        Pick::Ambiguous(count) => {
            tracing::warn!(issuer_id = %issuer.id, count, "oidc exchange: more than one trust policy matches");
            Err(Failure::Refused(
                Rejection::AmbiguousPolicy,
                Some(token.recorded()),
            ))
        }
    }
}

fn exchange_detail(
    candidate: &IssuerCandidate,
    claims: &Value,
    expires_at: Option<String>,
) -> Value {
    json!({
        "provider": "oidc_issuer",
        "trust_policy_id": candidate.policy.id,
        "issuer_id": candidate.issuer.id,
        "issuer": candidate.issuer.issuer,
        "service_account_id": candidate.account.user_id,
        "service_account": candidate.account.name,
        "claims": claims,
        "expires_at": expires_at,
    })
}

/// Step 6: mint, stamp the policy, and write the audit row — one transaction.
async fn mint(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    token: &VerifiedToken,
    chosen: &IssuerCandidate,
) -> Result<IssuerMint, Failure> {
    let recorded = token.recorded();
    let refuse = |reject| Failure::Refused(Rejection::Claims(reject), Some(token.recorded()));
    let actor = actor_of(db, chosen.account.user_id, headers)
        .await?
        .ok_or_else(|| refuse(ClaimReject::NoMatchingPolicy))?;
    let txn = db.begin().await?;
    let minted = issuer_trust_mint::mint_for_policy(&txn, chosen, token, Utc::now())
        .await?
        .map_err(refuse)?;
    let candidate = &minted.candidate;
    let expires_at = minted.minted.row.expires_at.map(|at| at.to_rfc3339());
    Event {
        action: EXCHANGED,
        token: &minted.minted.row,
        orgs: vec![candidate.account.org_id],
        detail: exchange_detail(candidate, &recorded, expires_at),
        change: None,
    }
    .record(&TxnAuditSink::request(&txn, &actor))
    .await
    .map_err(|e| Failure::Internal(format!("{e:?}")))?;
    txn.commit().await?;
    Ok(minted)
}

pub(super) async fn run(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    request: &Request,
    iss: &str,
) -> Result<ExchangeResponse, Failure> {
    let (picked, token) = chosen(db, request, iss).await?;
    let IssuerMint { minted, candidate } = mint(db, headers, &token, &picked).await?;
    tracing::info!(
        token_id = %minted.row.id,
        trust_policy_id = %candidate.policy.id,
        issuer_id = %candidate.issuer.id,
        subject = %token.sub,
        "oidc exchange: minted a ci token from a registered issuer's token"
    );
    response(db, candidate.account_name(), minted).await
}
