//! `TxnAuditSink::entry`: the actor is the sink's, everything else is the
//! row's. The rows themselves are `oxy-identity`'s tests; what they are
//! stored as is read back in `crates/server/tests/integration/token_auth/`.

use entity::audit_sink::{AuditRow, TOKEN_TARGET_TYPE};
use entity::users::UserStatus;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

use super::*;

fn row(org_id: Option<Uuid>) -> AuditRow {
    AuditRow {
        action: "token.revoked",
        org_id,
        target_type: Some(TOKEN_TARGET_TYPE.into()),
        target_id: Some("70".into()),
        target_label: Some("deploy".into()),
        metadata: json!({ "event_id": "e", "reason": "leak" }),
        before: Some(json!({ "at": "old" })),
        after: Some(json!({ "at": "new" })),
    }
}

fn user() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::from_u128(1),
        email: Some("ada@acme.com".into()),
        name: "Ada".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

fn assert_carries_the_row(entry: &AuditEntry, org_id: Option<Uuid>) {
    assert_eq!(entry.action, "token.revoked");
    assert_eq!(entry.org_id, org_id);
    assert_eq!(entry.target_type.as_deref(), Some("api_token"));
    assert_eq!(entry.target_id.as_deref(), Some("70"));
    assert_eq!(entry.target_label.as_deref(), Some("deploy"));
    assert_eq!(entry.metadata, json!({ "event_id": "e", "reason": "leak" }));
    assert_eq!(entry.before, Some(json!({ "at": "old" })));
    assert_eq!(entry.after, Some(json!({ "at": "new" })));
}

/// A row no request wrote is the system's, with the context it was given
/// (moved here from `oxy-identity`'s sandbox sweep test, with `system_entry`).
#[test]
fn a_system_row_is_the_systems_with_its_context() {
    let db = DatabaseConnection::default();
    let context = AuditContext {
        ip: Some("203.0.113.9".into()),
        user_agent: Some("scanner".into()),
        request_id: None,
    };
    let org = Some(Uuid::from_u128(0xA1));
    let entry = TxnAuditSink::system(&db, &context).entry(row(org));
    assert_eq!(entry.actor_email, "system");
    assert_eq!(entry.actor_type, ActorType::System);
    assert_eq!(entry.actor_user_id, None);
    assert_eq!(entry.context.ip.as_deref(), Some("203.0.113.9"));
    assert_eq!(entry.context.user_agent.as_deref(), Some("scanner"));
    assert_carries_the_row(&entry, org);
}

/// A row a request wrote is built by `for_request`, so it names the user and
/// the request's context; and the row's fields all land on it.
#[test]
fn a_request_row_is_the_request_actors() {
    let db = DatabaseConnection::default();
    let mut actor = RequestActor::session(user());
    actor.context.request_id = Some("req-1".into());
    let entry = TxnAuditSink::request(&db, &actor).entry(row(None));
    assert_eq!(entry.actor_user_id, Some(Uuid::from_u128(1)));
    assert_eq!(entry.actor_type, ActorType::User);
    assert_eq!(entry.context.request_id.as_deref(), Some("req-1"));
    assert_carries_the_row(&entry, None);
}

/// A row with no change and no org leaves both unset, as the builder did.
#[test]
fn an_unset_field_stays_unset() {
    let db = DatabaseConnection::default();
    let context = AuditContext::default();
    let mut bare = row(None);
    bare.before = None;
    bare.after = None;
    let entry = TxnAuditSink::system(&db, &context).entry(bare);
    assert!(entry.before.is_none() && entry.after.is_none());
    assert!(entry.org_id.is_none());
}

/// The sink is used as `&dyn AuditSink`, so it must stay dyn-compatible and
/// shareable across the handler's awaits.
#[test]
fn the_sink_is_a_dyn_audit_sink() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn AuditSink>();
    let db = DatabaseConnection::default();
    let context = AuditContext::default();
    let sink = TxnAuditSink::system(&db, &context);
    let _port: &dyn AuditSink = &sink;
}
