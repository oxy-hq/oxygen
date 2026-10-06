//! Liveness of assume-role sessions — the pure, `AppState`-free query cluster.
//!
//! Extracted from `oxy-app`'s `server::api::admin::assume` so the authz fact loader and
//! the partner tier (which live in this crate) can read assume-session liveness without a
//! dependency back on `oxy-app`. The handlers, the audit writes, and `may_act_as` stay in
//! `oxy-app`; only these `(&DatabaseConnection, …) -> …` reads over
//! `entity::admin_assume_sessions` move here. Each fails CLOSED: a DB error yields "no
//! live session" rather than silently granting cross-tenant reach.
//!
//! `oxy-app`'s `assume` module imports [`MAX_SESSION`] / [`live_filter`] back for its
//! handlers and re-exports [`is_session_live`] / [`live_assumed_org_ids`] /
//! [`live_sessions_for`] so its external callers keep resolving `assume::…` unchanged.

//!
//! ## A session belongs to the credential that opened it
//!
//! Every read here takes the [`Caller`] and sees only the sessions that caller's
//! credential may use ([`usable_by`]): a new-format API token sees the sessions
//! *it* opened, and nothing a browser opened; a browser session — and a legacy key,
//! which has always inherited the browser's — sees the ones opened in a browser
//! (API-tokens design §3.2, §3.5). Without this a token would silently pick up
//! whatever tenant its owner happened to be acting as in another tab.

use chrono::Utc;
use entity::admin_assume_sessions;
use entity::prelude::AdminAssumeSessions;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::caller::Caller;

/// Hard ceiling on a session. Non-renewable — an operator who needs longer starts
/// a new (separately audited) session, so a long investigation leaves a trail of
/// deliberate re-entries rather than one silently-extended grant.
pub const MAX_SESSION: i64 = 60;

/// A session is live when it hasn't been ended and hasn't expired.
pub fn live_filter(now: chrono::DateTime<chrono::FixedOffset>) -> Condition {
    Condition::all()
        .add(admin_assume_sessions::Column::EndedAt.is_null())
        .add(admin_assume_sessions::Column::ExpiresAt.gt(now))
}

/// The sessions `caller`'s credential may use: its own, for a new-format token;
/// the browser-opened ones (`token_id IS NULL`) for a session or a legacy key.
/// Always combined with the actor filter — this narrows a user's sessions, it does
/// not select them.
pub fn usable_by(caller: &Caller) -> Condition {
    let binding = match caller.assume_binding() {
        Some(token_id) => admin_assume_sessions::Column::TokenId.eq(token_id),
        None => admin_assume_sessions::Column::TokenId.is_null(),
    };
    Condition::all()
        .add(admin_assume_sessions::Column::ActorUserId.eq(caller.user_id))
        .add(binding)
}

/// **The enforcement primitive.** `org_context` / `workspace_context` call this
/// before synthesizing an Owner membership: no live session for `(caller, org)`,
/// no override. Fails CLOSED — a DB error denies the override rather than
/// silently granting cross-tenant reach.
pub async fn is_session_live(db: &DatabaseConnection, caller: &Caller, org_id: Uuid) -> bool {
    let now = Utc::now().fixed_offset();
    match AdminAssumeSessions::find()
        .filter(usable_by(caller))
        .filter(admin_assume_sessions::Column::OrgId.eq(org_id))
        .filter(live_filter(now))
        .one(db)
        .await
    {
        Ok(row) => row.is_some(),
        Err(e) => {
            tracing::error!("admin/assume: liveness check failed (denying): {e}");
            false
        }
    }
}

/// Every org the caller currently has a LIVE assume session for, on this credential.
///
/// The batch form of [`is_session_live`], for callers that need the whole set rather
/// than one answer — the authz fact loader, which has to know what an operator is
/// standing in before any specific org is named. Liveness stays defined here, once.
/// Fails CLOSED: a DB error yields no sessions, so the override is not granted.
pub async fn live_assumed_org_ids(db: &DatabaseConnection, caller: &Caller) -> Vec<Uuid> {
    let now = Utc::now().fixed_offset();
    match AdminAssumeSessions::find()
        .filter(usable_by(caller))
        .filter(live_filter(now))
        .all(db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| r.org_id).collect(),
        Err(e) => {
            tracing::error!("admin/assume: live-session listing failed (denying): {e}");
            Vec::new()
        }
    }
}

/// Every live session the caller holds on this credential. `resolve_scope` and the
/// admin guard both read it, so "am I acting?" has exactly one answer.
///
/// Fails CLOSED for authorization (empty ⇒ no synthesized reach) — but note the
/// admin guard below reads it in the opposite direction, where empty means
/// *allowed*. That asymmetry is deliberate and safe: a DB error there means staff
/// keep their normal admin access, which is the status quo, not an escalation.
pub async fn live_sessions_for(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Vec<admin_assume_sessions::Model> {
    let now = Utc::now().fixed_offset();
    AdminAssumeSessions::find()
        .filter(usable_by(caller))
        .filter(live_filter(now))
        .all(db)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("admin/assume: live session lookup failed: {e}");
            Vec::new()
        })
}
