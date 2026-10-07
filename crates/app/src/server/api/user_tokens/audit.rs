//! Lifecycle audit rows for a personal token (API-tokens design §3.7, layer 1).
//!
//! Each event is written **once per org the token reaches**, so every org's
//! chain shows the tokens that can touch it:
//!
//! - a narrowed token: the orgs its live grants name;
//! - an all-access token (and every legacy key): each org its owner belongs to
//!   at that moment.
//!
//! The rows of one event share a `metadata.event_id`, which is how Activity
//! shows the event once. The fan-out is capped at [`FANOUT_MAX_ORGS`]: past
//! that the first orgs by id get a row and each row says the event was
//! truncated and how many orgs the token reaches. A token that reaches no org
//! at all still gets one row, with no org.
//!
//! Written with `record_in_txn`, in the transaction that changes the token: the
//! change and the row that says who made it commit together. Rows name the
//! token by id and non-secret prefix — **never the secret**.
//!
//! **A row holds nothing of another org.** What is about the token alone (its
//! flags, its expiry, where it was minted from) is on every row of the event.
//! What belongs to one org (a grant, an app, a sandbox) is on that org's row
//! only: an event with such detail is written through [`Event::record_with`],
//! whose [`Own`] is asked once per row. `super::access_audit` builds the one
//! for a token's grants.

use chrono::{DateTime, FixedOffset};
use entity::prelude::OrgMembers;
use entity::{api_token_grants, api_tokens, org_members};
use oxy_app_core::audit::{self, AuditEntry, RequestActor, TOKEN_TARGET_TYPE};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::dto;
use super::error::TokenError;

/// The most orgs one lifecycle event is written to. A user in more orgs than
/// this is staff-shaped, and fifty chained inserts is already a long
/// transaction for a button press.
pub(crate) const FANOUT_MAX_ORGS: usize = 50;

pub(crate) const CREATED: &str = "token.created";
pub(crate) const EXTENDED: &str = "token.extended";
pub(crate) const REGENERATED: &str = "token.regenerated";
pub(crate) const REVOKED: &str = "token.revoked";
pub(crate) const GRANTS_CHANGED: &str = "token.grants_changed";
/// A ticket was redeemed: a browser now holds a session of this token
/// (`super::browser_session`).
pub(crate) const BROWSER_SESSION_OPENED: &str = "token.browser_session_opened";
/// An org ended a personal token's reach into it. Written to that org only.
pub(crate) const GRANT_REVOKED_BY_ORG: &str = "token.grant_revoked_by_org";
/// The sweeper expired a new-format token nobody used for a year.
pub(crate) const EXPIRED_UNUSED: &str = "token.expired_unused";
/// The sandbox sweep queued the teardown of the sandboxes an ended sandbox
/// agent token left behind (`super::sandboxes_queued`).
pub(crate) const EXPIRED_SANDBOXES_QUEUED: &str = "token.expired_sandboxes_queued";

/// The orgs a token reaches, sorted and distinct. `grants` are its rows; only
/// live ones reach anything.
pub(crate) fn reach_orgs(
    all_access: bool,
    owner_orgs: &[Uuid],
    grants: &[api_token_grants::Model],
) -> Vec<Uuid> {
    let mut orgs: Vec<Uuid> = if all_access {
        owner_orgs.to_vec()
    } else {
        grants
            .iter()
            .filter(|g| g.revoked_at.is_none())
            .map(|g| g.org_id)
            .collect()
    };
    orgs.sort_unstable();
    orgs.dedup();
    orgs
}

/// The orgs of two reaches together — an edit is recorded in the orgs the
/// token reached before it **and** the ones it reaches after, so an org that
/// lost the token sees it go.
pub(crate) fn union(mut a: Vec<Uuid>, b: Vec<Uuid>) -> Vec<Uuid> {
    a.extend(b);
    a.sort_unstable();
    a.dedup();
    a
}

/// What every row of the event carries about the token it is about.
fn base_metadata(token: &api_tokens::Model, event_id: Uuid, reach: usize) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("token_id".into(), json!(token.id));
    m.insert("token_kind".into(), json!(token.kind));
    m.insert("display_prefix".into(), json!(token.display_prefix));
    m.insert("event_id".into(), json!(event_id));
    m.insert("source".into(), json!(token.source));
    m.insert("reach_orgs".into(), json!(reach));
    if dto::is_legacy(token) {
        m.insert("api_key_id".into(), json!(dto::legacy_key_id(token)));
    }
    if reach > FANOUT_MAX_ORGS {
        m.insert("fanout_truncated".into(), json!(true));
    }
    m
}

/// One lifecycle event, ready to be written to every org it concerns.
pub(crate) struct Event<'a> {
    pub action: &'static str,
    pub token: &'a api_tokens::Model,
    /// Sorted, distinct — [`reach_orgs`]. For a service-account token, and for
    /// an org ending a token's reach, the one org concerned.
    pub orgs: Vec<Uuid>,
    /// Event-specific metadata, merged over the common keys **on every row**:
    /// only what is about the token, never what belongs to one org ([`Own`]).
    pub detail: Value,
    /// `(before, after)` for an event that changes something, on every row.
    pub change: Option<(Value, Value)>,
}

/// What the row of one org alone says, for an event whose detail belongs to an
/// org and must not be read on another's chain.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Own {
    /// Merged over that row's metadata. Anything but an object adds nothing.
    pub detail: Value,
    /// That row's `(before, after)`, in place of the event's.
    pub change: Option<(Value, Value)>,
}

impl Own {
    pub(crate) fn detail(detail: Value) -> Self {
        Self {
            detail,
            change: None,
        }
    }

    pub(crate) fn change(before: Value, after: Value) -> Self {
        Self {
            detail: Value::Null,
            change: Some((before, after)),
        }
    }
}

impl Event<'_> {
    /// The metadata of each row. `event_id` is shared by all of them.
    fn metadata(&self, event_id: Uuid) -> Value {
        let mut m = base_metadata(self.token, event_id, self.orgs.len());
        if let Value::Object(detail) = &self.detail {
            m.extend(detail.clone());
        }
        Value::Object(m)
    }

    /// The org of each row: the reach, capped — or one row with no org.
    fn row_orgs(&self) -> Vec<Option<Uuid>> {
        if self.orgs.is_empty() {
            return vec![None];
        }
        self.orgs
            .iter()
            .take(FANOUT_MAX_ORGS)
            .map(|org| Some(*org))
            .collect()
    }

    /// The event's rows, each started from `base` — the request's actor, or
    /// for a row no request wrote, the system (`super::system_audit`).
    pub(crate) fn entries_from(&self, base: impl Fn() -> AuditEntry) -> Vec<AuditEntry> {
        self.entries_with(base, |_| Own::default())
    }

    /// [`Self::entries_from`], with what `own` answers for a row's org on that
    /// row alone: its detail merged over the row's metadata, its change as
    /// the row's before and after. The rows still share one `event_id`. An
    /// `own` that answers nothing leaves the row as [`Self::entries_from`]
    /// writes it.
    pub(crate) fn entries_with(
        &self,
        base: impl Fn() -> AuditEntry,
        own: impl Fn(Option<Uuid>) -> Own,
    ) -> Vec<AuditEntry> {
        let shared = self.metadata(Uuid::new_v4());
        self.row_orgs()
            .into_iter()
            .map(|org| {
                let Own { detail, change } = own(org);
                let mut metadata = shared.clone();
                if let (Value::Object(row), Value::Object(detail)) = (&mut metadata, detail) {
                    row.extend(detail);
                }
                let mut entry = base()
                    .target(
                        TOKEN_TARGET_TYPE,
                        self.token.id.to_string(),
                        self.token.name.clone(),
                    )
                    .metadata(metadata);
                if let Some((before, after)) = change.or_else(|| self.change.clone()) {
                    entry = entry.change(before, after);
                }
                match org {
                    Some(org) => entry.org(org),
                    None => entry,
                }
            })
            .collect()
    }

    /// Write the event's rows in `txn`. A failed write fails the request: the
    /// token never changes without the row that says who changed it.
    pub(crate) async fn record<C: ConnectionTrait>(
        &self,
        txn: &C,
        actor: &RequestActor,
    ) -> Result<(), TokenError> {
        self.record_with(txn, actor, |_| Own::default()).await
    }

    /// [`Self::record`], for an event that says something of one org: `own`
    /// answers what each org's row alone holds ([`Self::entries_with`]).
    pub(crate) async fn record_with<C: ConnectionTrait>(
        &self,
        txn: &C,
        actor: &RequestActor,
        own: impl Fn(Option<Uuid>) -> Own,
    ) -> Result<(), TokenError> {
        let base = || AuditEntry::for_request(actor, self.action);
        for entry in self.entries_with(base, own) {
            audit::record_in_txn(txn, entry).await?;
        }
        Ok(())
    }
}

/// Every org `user_id` belongs to — an all-access token's reach.
pub(crate) async fn owner_orgs<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Vec<Uuid>, TokenError> {
    let rows = OrgMembers::find()
        .filter(org_members::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|m| m.org_id).collect())
}

pub(crate) fn rfc3339(at: Option<DateTime<FixedOffset>>) -> Value {
    at.map_or(Value::Null, |t| Value::String(t.to_rfc3339()))
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
