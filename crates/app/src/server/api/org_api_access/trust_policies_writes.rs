//! The transactional half of each trust-policy write.
//!
//! **Everything here takes the transaction, and only the transaction.** No
//! function in this file is handed the pool, so none can ask it for a second
//! connection while it holds the first and a row lock — which is how a burst
//! of concurrent edits wedges a pool: each holds one connection, each waits
//! for another, none can release. Whatever a write needs from the pool (a
//! target check, a GitHub lookup, the answer's DTO) its handler in
//! [`super`] does before `begin` or after `commit`.
//!
//! And each write that changes an existing policy **decides from the row it
//! locks** ([`locked`]), never from one read before the transaction:
//!
//! - a mint in flight for the policy holds its row lock
//!   (`oxy_auth::token::ci_mint`), so the mint's token is committed — and seen
//!   by the revoke here — before the write goes on; the lock is taken even
//!   when an edit changes no column of the row (its grants alone);
//! - a row read earlier can be stale: off, on, off in quick succession, and
//!   the second "off" would see the policy still disabled, change nothing,
//!   revoke nothing, and answer 200 for a policy that is enabled and minting.
//!   A second delete crossing the first finds the policy gone here, before
//!   anything is revoked, deleted or audited.

use entity::{api_tokens, oidc_trust_policies, service_accounts};
use oxy_app_core::audit::{self, RequestActor};
use oxy_auth::token::trust_policy::{self, NewPolicy};
use oxy_auth::token::trust_policy_access::PolicyEdit;
use oxy_auth::token::{ci, ci_mint};
use sea_orm::DatabaseTransaction;
use serde_json::json;
use uuid::Uuid;

use super::{CREATED, DELETED, UPDATED, entry, summary};
use crate::server::api::user_tokens::error::TokenError;

/// The policy **of this account** as it is now, under its row lock: what a
/// write decides from. Taken first. Gone, or another account's, is not found.
async fn locked(
    txn: &DatabaseTransaction,
    account: &service_accounts::Model,
    policy_id: Uuid,
) -> Result<oidc_trust_policies::Model, TokenError> {
    ci_mint::lock_policy(txn, policy_id)
        .await?
        .filter(|policy| policy.service_account_id == account.user_id)
        .ok_or(TokenError::NotFound)
}

/// Store the policy and its `created` row.
pub(super) async fn create(
    txn: &DatabaseTransaction,
    actor: &RequestActor,
    account: &service_accounts::Model,
    new: NewPolicy,
) -> Result<oidc_trust_policies::Model, TokenError> {
    // Asked again where the policy is written: the org may have turned the
    // requirement on since the handler's check. (The exchange asks too, so one
    // that slipped through would mint nothing — this keeps it from being
    // stored at all.)
    if new.environment.is_none() && trust_policy::environment_required(txn, new.org_id).await? {
        return Err(TokenError::EnvironmentRequired);
    }
    let row = trust_policy::create(txn, new).await?;
    let grants = trust_policy::grants_for(txn, &[row.id]).await?;
    let detail = summary(account, &row, &grants);
    audit::record_in_txn(txn, entry(actor, CREATED, &row, detail)).await?;
    Ok(row)
}

/// Apply `edit` to the policy as it is under its lock. The policy after, and
/// the tokens revoked because it changed.
pub(super) async fn patch(
    txn: &DatabaseTransaction,
    actor: &RequestActor,
    account: &service_accounts::Model,
    policy_id: Uuid,
    edit: &PolicyEdit,
) -> Result<(oidc_trust_policies::Model, Vec<api_tokens::Model>), TokenError> {
    let before = locked(txn, account, policy_id).await?;
    // Asked only when the edit itself clears the environment: an edit that
    // leaves it alone never fails over a requirement it did not touch.
    let clears_environment = edit.environment == Some(None);
    if clears_environment && trust_policy::environment_required(txn, account.org_id).await? {
        return Err(TokenError::EnvironmentRequired);
    }
    let grants_before = trust_policy::grants_for(txn, &[policy_id]).await?;
    let after = trust_policy::update(txn, before.clone(), edit).await?;
    let grants_after = trust_policy::grants_for(txn, &[policy_id]).await?;
    let was = summary(account, &before, &grants_before);
    let now = summary(account, &after, &grants_after);
    let mut revoked = Vec::new();
    if was != now {
        revoked = ci::revoke_for_policy(txn, policy_id, actor.id).await?;
        let detail = json!({ "revoked_tokens": revoked.len() });
        let row = entry(actor, UPDATED, &after, detail).change(was, now);
        audit::record_in_txn(txn, row).await?;
    }
    Ok((after, revoked))
}

/// Delete the policy as it is under its lock, revoking what it minted. The
/// tokens revoked.
pub(super) async fn delete(
    txn: &DatabaseTransaction,
    actor: &RequestActor,
    account: &service_accounts::Model,
    policy_id: Uuid,
) -> Result<Vec<api_tokens::Model>, TokenError> {
    let row = locked(txn, account, policy_id).await?;
    let grants = trust_policy::grants_for(txn, &[policy_id]).await?;
    let revoked = ci::revoke_for_policy(txn, policy_id, actor.id).await?;
    trust_policy::delete(txn, policy_id).await?;
    let detail = json!({ "revoked_tokens": revoked.len() });
    let was = summary(account, &row, &grants);
    let deleted = entry(actor, DELETED, &row, detail).change(was, json!(null));
    audit::record_in_txn(txn, deleted).await?;
    Ok(revoked)
}
