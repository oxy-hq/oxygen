//! Step 4 of the exchange: mint, stamp the policy, and write the audit row —
//! one transaction (API-tokens design §3.4).
//!
//! The policy was chosen from rows read before this transaction began. What
//! is minted from is what `oxy_auth::token::ci_mint` finds when it reads them
//! again **under lock**, so a policy or account disabled in between mints
//! nothing — and a disable that arrives while this is in flight waits, then
//! revokes the token this committed. The audit row and the answer name the
//! rows as they were under that lock.

use axum::http::HeaderMap;
use chrono::Utc;
use entity::prelude::Users;
use entity::users::UserStatus;
use oxy_app_core::audit::RequestActor;
use oxy_auth::github_oidc::{ClaimReject, GithubOidcClaims};
use oxy_auth::token::ci_mint::{self, PolicyMint};
use oxy_auth::token::trust_policy::Candidate;
use sea_orm::{DatabaseConnection, EntityTrait, TransactionTrait};
use serde_json::{Value, json};

use super::Failure;
use super::reject::Rejection;
use crate::server::api::user_tokens::audit::Event;

const EXCHANGED: &str = "oidc.token_exchanged";

/// The account's `users` row as the audit actor: the token acts as it, and the
/// exchange is the account's own act.
async fn actor_of(
    db: &DatabaseConnection,
    candidate: &Candidate,
    headers: &HeaderMap,
) -> Result<Option<RequestActor>, Failure> {
    let user = Users::find_by_id(candidate.account.user_id)
        .one(db)
        .await?
        .filter(|u| u.status == UserStatus::Active);
    Ok(user.map(|u| RequestActor::for_user(u.into(), headers)))
}

fn exchange_detail(candidate: &Candidate, claims: &Value, expires_at: Option<String>) -> Value {
    json!({
        "trust_policy_id": candidate.policy.id,
        "service_account_id": candidate.account.user_id,
        "service_account": candidate.account.name,
        "claims": claims,
        "expires_at": expires_at,
    })
}

/// Mint for the policy the exchange `chosen`, if it still admits the run.
pub(super) async fn mint(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    claims: &GithubOidcClaims,
    chosen: &Candidate,
) -> Result<PolicyMint, Failure> {
    let recorded = claims.recorded();
    let refuse = |reject| Failure::Refused(Rejection::Claims(reject), Some(claims.recorded()));
    // An account with no usable `users` row mints nothing — as it
    // authenticates nothing.
    let actor = actor_of(db, chosen, headers)
        .await?
        .ok_or_else(|| refuse(ClaimReject::NoMatchingPolicy))?;

    let txn = db.begin().await?;
    // A refusal returns here with nothing written; dropping `txn` rolls back
    // and lets go of the locks.
    let minted = ci_mint::mint_for_policy(&txn, chosen, claims, Utc::now())
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
    .record(&txn, &actor)
    .await
    .map_err(|e| Failure::Internal(format!("{e:?}")))?;
    txn.commit().await?;
    Ok(minted)
}
