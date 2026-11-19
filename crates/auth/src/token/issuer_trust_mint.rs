//! The mint's last look at an OIDC issuer trust policy (API-tokens design
//! §3.4, *Any OIDC issuer*).
//!
//! The same discipline as a GitHub policy's mint ([`super::ci_mint`]): the
//! exchange chose a policy from rows read outside any transaction, so the
//! mint **reads the account, the issuer and the policy again inside its own
//! transaction, under row locks a disable or a delete has to wait for**, and
//! mints only if what it finds would still admit the token. Whichever commits
//! first, either the change is seen here and nothing is minted, or the change
//! waits and its revoke sees the token minted here.
//!
//! **The locks, in the order they are always taken:**
//!
//! | Row | Lock | What has to wait |
//! | --- | --- | --- |
//! | the account | `FOR SHARE` | disabling or deleting it |
//! | the issuer | `FOR SHARE` | deleting it, which takes it `FOR UPDATE` first and then each of its policies |
//! | the policy | `FOR UPDATE` | disabling, editing or deleting it |

use chrono::{DateTime, Utc};
use entity::prelude::{OidcIssuers, ServiceAccounts};
use oxy_shared::errors::OxyError;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, QuerySelect};

use super::ci::{self, NewCiToken};
use super::issuer_trust_policy::{self, IssuerCandidate, conditions_of};
use super::personal::Minted;
use super::service_account;
use crate::github_oidc::ClaimReject;
use crate::workload_identity::{VerifiedToken, conditions};

fn db_err(what: &'static str) -> impl FnOnce(DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// What a minted token is named: the issuer as its org calls it, and the
/// token's subject. The rest of its claims are recorded on the row.
pub fn machine_identity(candidate: &IssuerCandidate, token: &VerifiedToken) -> String {
    format!("oidc:{} {}", candidate.issuer.name, token.sub)
}

/// The account, the issuer, then the policy, each under its lock — and only
/// if all three are still there, enabled, and still belong together.
async fn lock_live<C: ConnectionTrait>(
    db: &C,
    chosen: &IssuerCandidate,
) -> Result<Option<IssuerCandidate>, OxyError> {
    let account = ServiceAccounts::find_by_id(chosen.account.user_id)
        .lock_shared()
        .one(db)
        .await
        .map_err(db_err("lock service account"))?;
    let Some(account) = account.filter(|a| a.disabled_at.is_none()) else {
        return Ok(None);
    };
    let issuer = OidcIssuers::find_by_id(chosen.issuer.id)
        .lock_shared()
        .one(db)
        .await
        .map_err(db_err("lock oidc issuer"))?;
    let Some(issuer) = issuer.filter(|i| i.org_id == account.org_id) else {
        return Ok(None);
    };
    let policy = issuer_trust_policy::lock(db, chosen.policy.id).await?;
    let Some(policy) = policy
        .filter(|p| p.disabled_at.is_none())
        .filter(|p| p.service_account_id == account.user_id && p.org_id == account.org_id)
        .filter(|p| p.issuer_id == issuer.id)
    else {
        return Ok(None);
    };
    Ok(Some(IssuerCandidate {
        policy,
        issuer,
        account,
        org_slug: chosen.org_slug.clone(),
    }))
}

/// A token minted from an issuer policy, with the rows it was minted from
/// **as they were under lock**.
#[derive(Debug)]
pub struct IssuerMint {
    pub minted: Minted,
    pub candidate: IssuerCandidate,
}

/// Mint an `oxy_ci_` token for `token` from the policy the exchange `chosen`,
/// if it would still admit the token now. Only the conditions of that one
/// policy are checked again here: whether another policy also matched was
/// the exchange's question, asked of the rows it read.
///
/// **Run it in a transaction, and commit that transaction.** `Ok(Err(_))` is
/// a refusal: nothing was written, and the caller drops the transaction.
pub async fn mint_for_policy<C: ConnectionTrait>(
    db: &C,
    chosen: &IssuerCandidate,
    token: &VerifiedToken,
    now: DateTime<Utc>,
) -> Result<Result<IssuerMint, ClaimReject>, OxyError> {
    let Some(candidate) = lock_live(db, chosen).await? else {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    };
    if !conditions::matches(&token.claims, &conditions_of(&candidate.policy)) {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    }
    let Some(role) = service_account::role_of(&candidate.account) else {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    };
    let rows = issuer_trust_policy::grants_for(db, &[candidate.policy.id]).await?;
    let grants = ci::capped_grants(&rows, role);
    if grants.is_empty() {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    }
    let new = NewCiToken {
        account_id: candidate.account.user_id,
        policy_id: candidate.policy.id,
        name: machine_identity(&candidate, token),
        grants,
        claims: token.recorded(),
        now,
    };
    let minted = ci::mint(db, new).await?;
    issuer_trust_policy::mark_used(db, candidate.policy.clone(), now).await?;
    Ok(Ok(IssuerMint { minted, candidate }))
}
