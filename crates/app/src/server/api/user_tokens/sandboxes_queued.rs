//! `token.expired_sandboxes_queued`: the sandbox maintenance loop queued the
//! teardown of the sandboxes an ended sandbox agent token left behind
//! (sandbox agent credential design §2, "Audit").
//!
//! One event per token, written with the system as its actor in the
//! transaction that marks the sandboxes (`custom_apps_sandboxes::token_ended`).
//! Like every lifecycle event it gets one row per org that concerns it, here
//! the org of each app the token was granted, and the rows share one
//! `event_id`.
//!
//! **A row names only its own org's sandboxes.** A token may be granted apps
//! of several orgs, and an app id, a sandbox name or a teardown run id of one
//! org is not for another org's chain to hold. So the list is built per row:
//! org X's row lists the sandboxes of org X's apps and nothing of any other
//! org's, not even how many there were. An org whose app was granted and had
//! no sandbox left still gets its row, with an empty list.

use chrono::{DateTime, Utc};
use entity::{api_token_grants, api_tokens};
use oxy_app_core::audit::{AuditContext, AuditEntry};
use oxy_auth::token::personal;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::audit::{EXPIRED_SANDBOXES_QUEUED, Event, Own};
use super::error::TokenError;
use super::system_audit::system_entry;

/// `metadata.reason`, and the teardown's reason.
const REASON: &str = "token_ended";

/// One sandbox whose teardown the pass queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedSandbox {
    pub org_id: Uuid,
    pub app_id: Uuid,
    pub environment: String,
    pub run_id: String,
}

/// The orgs the event is written to: the org of every app the token was
/// granted, and of every sandbox queued. Sorted and distinct.
fn concerned_orgs(grants: &[api_token_grants::Model], queued: &[QueuedSandbox]) -> Vec<Uuid> {
    let mut orgs: Vec<Uuid> = grants
        .iter()
        .filter(|grant| grant.kind == api_token_grants::KIND_APP_SANDBOX)
        .map(|grant| grant.org_id)
        .chain(queued.iter().map(|sandbox| sandbox.org_id))
        .collect();
    orgs.sort_unstable();
    orgs.dedup();
    orgs
}

/// `revoked` when the revocation is what ended the token, `expired` otherwise.
fn ended_by(token: &api_tokens::Model, ended_at: DateTime<Utc>) -> &'static str {
    let revoked = token.revoked_at.map(DateTime::<Utc>::from);
    if revoked == Some(ended_at) {
        "revoked"
    } else {
        "expired"
    }
}

/// What every row says: about the token, nothing about any sandbox.
fn shared_detail(token: &api_tokens::Model, ended_at: DateTime<Utc>) -> Value {
    json!({
        "reason": REASON,
        "ended_by": ended_by(token, ended_at),
        "ended_at": ended_at.to_rfc3339(),
    })
}

/// What the row of `org` alone says: the sandboxes queued of that org's apps.
/// A row with no org lists none.
fn own_detail(org: Option<Uuid>, queued: &[QueuedSandbox]) -> Own {
    let sandboxes: Vec<Value> = queued
        .iter()
        .filter(|sandbox| Some(sandbox.org_id) == org)
        .map(|sandbox| {
            json!({
                "app_id": sandbox.app_id,
                "environment": sandbox.environment,
                "run_id": sandbox.run_id,
            })
        })
        .collect();
    Own::detail(json!({ "sandboxes": sandboxes }))
}

/// The event's rows, one per concerned org, each started from `base`.
fn entries(
    token: &api_tokens::Model,
    ended_at: DateTime<Utc>,
    grants: &[api_token_grants::Model],
    queued: &[QueuedSandbox],
    base: impl Fn() -> AuditEntry,
) -> Vec<AuditEntry> {
    Event {
        action: EXPIRED_SANDBOXES_QUEUED,
        token,
        orgs: concerned_orgs(grants, queued),
        detail: shared_detail(token, ended_at),
        change: None,
    }
    .entries_with(base, |org| own_detail(org, queued))
}

/// Write the event for `token`, which ended at `ended_at`, in `txn`.
pub(crate) async fn record<C: ConnectionTrait>(
    txn: &C,
    token: &api_tokens::Model,
    ended_at: DateTime<Utc>,
    queued: &[QueuedSandbox],
) -> Result<(), TokenError> {
    let grants = personal::grants_for(txn, &[token.id]).await?;
    let context = AuditContext::default();
    let base = || system_entry(EXPIRED_SANDBOXES_QUEUED, &context);
    for entry in entries(token, ended_at, &grants, queued, base) {
        oxy_app_core::audit::record_in_txn(txn, entry).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "sandboxes_queued_tests.rs"]
mod tests;
