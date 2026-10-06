//! The mint's last look at its policy (API-tokens design §3.4).
//!
//! The exchange chooses a policy from rows it read outside any transaction,
//! and mints a moment later. In between, an admin can disable that policy —
//! and a disable revokes what the policy minted, so that the run holding a
//! token stops with it. Without a second look, both could win: the disable
//! revokes the tokens it can see, and the mint, already past its check,
//! commits one more that nobody revokes for fifteen minutes.
//!
//! So the mint **re-reads the policy and its account inside its own
//! transaction, under row locks a disable has to wait for**, and mints only if
//! what it finds would still admit the run. Then exactly one of two things is
//! true, whichever of them commits first:
//!
//! - the change committed first — the mint sees it here and refuses; or
//! - the mint holds the locks — the change waits, and when it runs its revoke
//!   sees the token the mint committed.
//!
//! **The locks, in the order they are always taken** (an account's delete
//! takes them in the same order, so the two cannot deadlock):
//!
//! | Row | Lock | What has to wait |
//! | --- | --- | --- |
//! | the account | `FOR SHARE` | disabling or deleting it — and not another mint, so two policies of one account mint side by side |
//! | the policy | `FOR UPDATE` | disabling, editing or deleting it; the mint stamps it as used, so it takes the write lock from the start |
//!
//! A policy's own writes ([`lock_policy`]) take the policy's lock before they
//! read or revoke anything, so an edit that changes no column of the row —
//! its grants alone — waits too. **And they decide from the row the lock
//! hands back**, never from one read before the transaction: two requests
//! that cross (off, on, off in quick succession) are each judged against
//! what the other left, in the order they took the lock. An account's writes
//! do the same with [`super::service_account::lock`].

use chrono::{DateTime, Utc};
use entity::prelude::{OidcTrustPolicies, ServiceAccounts};
use entity::{oidc_trust_policies, service_accounts};
use oxy_shared::errors::OxyError;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, QuerySelect};
use uuid::Uuid;

use super::ci::{self, NewCiToken};
use super::exchange::{self, Decision};
use super::personal::Minted;
use super::trust_policy::{self, Candidate};
use super::{policy_store, service_account};
use crate::github_oidc::{self, ClaimReject, GithubOidcClaims};

fn db_err(what: &'static str) -> impl FnOnce(DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// Take the policy's row lock, and hand back the row as it is now.
///
/// What a change to a policy — an edit, a disable, a delete — does **first**
/// in its transaction: a mint in flight for the policy holds this lock, so its
/// token is committed, and visible to the revoke that follows, before the
/// change goes on. `None` when the policy is gone.
///
/// **The row it returns is the one to decide from** — the only copy that
/// cannot be out of date. A row read before the transaction can be: another
/// request may have committed in between, and a disable that judged "already
/// disabled" from the old copy would then do nothing, and say it had. Hence
/// `must_use`: taking the lock and deciding from something else is the bug.
#[must_use = "decide from the row this returns: it is the policy as it is under the lock"]
pub async fn lock_policy<C: ConnectionTrait>(
    db: &C,
    policy_id: Uuid,
) -> Result<Option<oidc_trust_policies::Model>, OxyError> {
    OidcTrustPolicies::find_by_id(policy_id)
        .lock_exclusive()
        .one(db)
        .await
        .map_err(db_err("lock trust policy"))
}

/// The account, then the policy, each under its lock — and only if both are
/// still enabled and still belong together.
async fn lock_live<C: ConnectionTrait>(
    db: &C,
    chosen: &Candidate,
) -> Result<Option<(oidc_trust_policies::Model, service_accounts::Model)>, OxyError> {
    let account = ServiceAccounts::find_by_id(chosen.account.user_id)
        .lock_shared()
        .one(db)
        .await
        .map_err(db_err("lock service account"))?;
    let Some(account) = account.filter(|a| a.disabled_at.is_none()) else {
        return Ok(None);
    };
    let policy = lock_policy(db, chosen.policy.id).await?;
    Ok(policy
        .filter(|p| p.disabled_at.is_none())
        .filter(|p| p.service_account_id == account.user_id && p.org_id == account.org_id)
        .map(|policy| (policy, account)))
}

/// A token minted from a policy, with the policy and account it was minted
/// from **as they were under lock** — what the audit row and the answer name.
#[derive(Debug)]
pub struct PolicyMint {
    pub minted: Minted,
    pub candidate: Candidate,
}

/// Mint an `oxy_ci_` token for `claims` from the policy the exchange `chosen`,
/// if that policy would still admit the run now.
///
/// **Run it in a transaction, and commit that transaction.** `chosen` is what
/// was read before it began; nothing in it is trusted here but which rows to
/// look at. Inside the transaction, under the locks this module describes:
///
/// 1. the account and the policy are read again — disabled, deleted or no
///    longer each other's, and the answer is [`ClaimReject::NoMatchingPolicy`],
///    exactly as if the policy had never been a candidate;
/// 2. the run is matched against the policy **as it is now**, with the org's
///    environment requirement as it is now — an edit that landed in between is
///    judged, not skipped;
/// 3. the grants are read and capped by the account's standing now;
/// 4. the token is minted and the policy stamped as used.
///
/// `Ok(Err(_))` is a refusal: nothing was written, and the caller drops the
/// transaction.
pub async fn mint_for_policy<C: ConnectionTrait>(
    db: &C,
    chosen: &Candidate,
    claims: &GithubOidcClaims,
    now: DateTime<Utc>,
) -> Result<Result<PolicyMint, ClaimReject>, OxyError> {
    let Some((policy, account)) = lock_live(db, chosen).await? else {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    };
    let candidate = Candidate {
        policy,
        account,
        org_slug: chosen.org_slug.clone(),
    };
    let environment_required = policy_store::load(db, candidate.account.org_id)
        .await?
        .require_environment_on_trust_policies;
    if let Decision::Reject(reject) =
        exchange::decide(claims, vec![candidate.clone()], environment_required)
    {
        return Ok(Err(reject));
    }
    // An account with a role this release does not know mints nothing — as it
    // authenticates nothing. Nor does a policy whose every grant names
    // something that is gone.
    let Some(role) = service_account::role_of(&candidate.account) else {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    };
    let rows = trust_policy::grants_for(db, &[candidate.policy.id]).await?;
    let grants = ci::capped_grants(&rows, role);
    if grants.is_empty() {
        return Ok(Err(ClaimReject::NoMatchingPolicy));
    }
    let new = NewCiToken {
        account_id: candidate.account.user_id,
        policy_id: candidate.policy.id,
        name: github_oidc::machine_identity(claims),
        grants,
        claims: claims.recorded(),
        now,
    };
    let minted = ci::mint(db, new).await?;
    trust_policy::mark_used(db, candidate.policy.clone(), &claims.repository, now).await?;
    Ok(Ok(PolicyMint { minted, candidate }))
}
