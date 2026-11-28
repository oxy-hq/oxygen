//! `token.expired_sandboxes_queued`: the sandbox maintenance loop queued the
//! teardown of the sandboxes an ended sandbox agent token left behind
//! (sandbox agent credential design §2, "Audit").
//!
//! One event per token, written with the system as its actor in the
//! transaction that marks the sandboxes (`oxy-api-custom-apps`'
//! `custom_apps_sandboxes::token_ended`, which hands in the sink bound to
//! both: `TxnAuditSink::system`).
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
use entity::audit_sink::{AuditRow, AuditSink};
use entity::{api_token_grants, api_tokens};
use oxy_auth::token::personal;
use oxy_shared::errors::OxyError;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{EXPIRED_SANDBOXES_QUEUED, Event, Own, write};

/// `metadata.reason`, and the teardown's reason.
const REASON: &str = "token_ended";

/// One sandbox whose teardown the pass queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedSandbox {
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

/// The event's rows, one per concerned org.
fn rows(
    token: &api_tokens::Model,
    ended_at: DateTime<Utc>,
    grants: &[api_token_grants::Model],
    queued: &[QueuedSandbox],
) -> Vec<AuditRow> {
    Event {
        action: EXPIRED_SANDBOXES_QUEUED,
        token,
        orgs: concerned_orgs(grants, queued),
        detail: shared_detail(token, ended_at),
        change: None,
    }
    .rows_with(|org| own_detail(org, queued))
}

/// Write the event for `token`, which ended at `ended_at`: its grants are
/// read in `txn`, and its rows written through `sink`, which the caller binds
/// to that same transaction and to the system as the actor.
pub async fn record<C: ConnectionTrait>(
    txn: &C,
    sink: &dyn AuditSink,
    token: &api_tokens::Model,
    ended_at: DateTime<Utc>,
    queued: &[QueuedSandbox],
) -> Result<(), OxyError> {
    let grants = personal::grants_for(txn, &[token.id]).await?;
    write(sink, rows(token, ended_at, &grants, queued))
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))
}

#[cfg(test)]
#[path = "sandboxes_queued_tests.rs"]
mod tests;
