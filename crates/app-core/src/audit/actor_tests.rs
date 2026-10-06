//! `AuditEntry::for_request`: what a key-performed action records, and that no
//! later setter can drop it.

use super::*;
use axum::http::Request;
use entity::users::UserStatus;
use oxy_auth::token::StoredKind;

fn user() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("ada@acme.com".into()),
        name: "Ada".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

fn credential(user_id: Uuid) -> CredentialContext {
    CredentialContext {
        token_id: Uuid::new_v4(),
        kind: StoredKind::Personal,
        principal_user_id: user_id,
        all_access: true,
        platform: true,
        partner: true,
        name: "deploy".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        legacy_api_key_id: None,
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: Vec::new(),
        app_publish: Vec::new(),
    }
}

fn keyed() -> RequestActor {
    let user = user();
    let credential = Some(credential(user.id));
    RequestActor {
        user,
        credential,
        context: AuditContext::default(),
    }
}

/// A service account acting with its `oxy_sat_` token: a `users` row with no
/// address, whose name is the account's slug.
fn service_account() -> RequestActor {
    let user = AuthenticatedUser {
        id: Uuid::new_v4(),
        email: None,
        name: "deploy-bot".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    };
    let credential = CredentialContext {
        kind: StoredKind::ServiceAccount,
        all_access: false,
        platform: false,
        partner: false,
        name: "release".into(),
        display_prefix: "oxy_sat_Ab3x".into(),
        ..credential(user.id)
    };
    RequestActor {
        user,
        credential: Some(credential),
        context: AuditContext::default(),
    }
}

#[test]
fn a_service_account_is_named_by_its_account_and_stamped_as_one() {
    // Design §3.7: an action a service account performs is attributed to the
    // account by a label that can never be mistaken for an address, and the
    // stamp says which kind of token did it.
    let actor = service_account();
    let e = AuditEntry::for_request(&actor, "workspace.secret.updated");
    assert_eq!(e.actor_type, ActorType::ApiKey);
    assert_eq!(e.actor_user_id, Some(actor.user.id));
    assert_eq!(e.actor_email, "deploy-bot");
    assert!(!e.actor_email.contains('@'));
    let meta = e.effective_metadata();
    assert_eq!(meta["token_kind"], "service_account");
    assert_eq!(meta["token_name"], "release");
    assert_eq!(meta["display_prefix"], "oxy_sat_Ab3x");
}

#[test]
fn a_session_records_a_user_and_no_token() {
    let actor = RequestActor::session(user());
    let e = AuditEntry::for_request(&actor, "org.member.removed");
    assert_eq!(e.actor_type, ActorType::User);
    assert_eq!(e.actor_user_id, Some(actor.user.id));
    assert_eq!(e.actor_email, "ada@acme.com");
    assert_eq!(e.effective_metadata(), json!({}));
}

#[test]
fn a_key_records_api_key_and_the_four_token_fields() {
    let actor = keyed();
    let cred = actor.credential.clone().unwrap();
    let e = AuditEntry::for_request(&actor, "org.member.removed");
    assert_eq!(e.actor_type, ActorType::ApiKey);
    assert_eq!(e.actor_user_id, Some(actor.user.id));
    assert_eq!(
        e.effective_metadata(),
        json!({
            "token_id": cred.token_id,
            "token_name": "deploy",
            "token_kind": "personal",
            "display_prefix": "oxy_pat_Ab3x",
        })
    );
}

#[test]
fn a_later_metadata_call_neither_drops_nor_overwrites_the_stamp() {
    let actor = keyed();
    let cred = actor.credential.clone().unwrap();
    let e = AuditEntry::for_request(&actor, "member.added")
        .metadata(json!({ "surface": "admin", "token_id": "forged" }));
    let meta = e.effective_metadata();
    assert_eq!(meta["surface"], "admin", "the caller's keys are kept");
    assert_eq!(meta["token_id"], json!(cred.token_id), "the stamp wins");
    assert_eq!(meta["token_kind"], "personal");
}

#[test]
fn assigning_the_metadata_field_directly_cannot_drop_the_stamp_either() {
    let actor = keyed();
    let mut e = AuditEntry::for_request(&actor, "x");
    e.metadata = json!({});
    assert!(e.effective_metadata().get("token_id").is_some());
}

#[test]
fn non_object_metadata_is_kept_beside_the_stamp() {
    let actor = keyed();
    let e = AuditEntry::for_request(&actor, "x").metadata(json!("note"));
    let meta = e.effective_metadata();
    assert_eq!(meta["value"], "note");
    assert!(meta.get("token_id").is_some());
}

#[test]
fn a_key_stays_api_key_through_actor_and_acting_as() {
    let actor = keyed();
    let e = AuditEntry::for_request(&actor, "x")
        .acting_as(ActorType::PartnerAdmin)
        .actor(actor.user.id, ActorType::User);
    assert_eq!(e.actor_type, ActorType::ApiKey);

    let session = RequestActor::session(user());
    let e = AuditEntry::for_request(&session, "x").acting_as(ActorType::PartnerAdmin);
    assert_eq!(e.actor_type, ActorType::PartnerAdmin);
}

#[test]
fn the_written_entry_carries_the_stamp_in_jsonb_key_order() {
    // The chain hashes metadata as text; jsonb hands keys back sorted by length
    // then bytes, so the row must be written that way or it verifies broken.
    let actor = keyed();
    let e = AuditEntry::for_request(&actor, "x").metadata(json!({ "surface": "admin" }));
    let written = super::super::canonical_entry(e);
    let keys: Vec<&str> = written
        .metadata
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "surface",
            "token_id",
            "token_kind",
            "token_name",
            "display_prefix"
        ]
    );
    assert_eq!(written.effective_metadata(), written.metadata);
}

#[test]
fn the_entry_never_contains_a_token() {
    // CredentialContext has no field for one; this pins that the audit side
    // adds nothing beyond id, name, kind and prefix.
    let actor = keyed();
    let e = AuditEntry::for_request(&actor, "x");
    let keys: Vec<String> = e
        .effective_metadata()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys.len(), 4);
    assert!(!keys.iter().any(|k| k.contains("hash") || k == "token"));
}

#[test]
fn the_extractor_reads_user_credential_and_request_context() {
    let u = user();
    let cred = credential(u.id);
    let mut req = Request::builder()
        // The caller wrote the first hop; the load balancer appended the second.
        .header("x-forwarded-for", " 6.6.6.6, 203.0.113.9")
        .header("user-agent", "oxyc/0.5.0")
        .header("x-oxy-request-id", "req-1")
        .body(())
        .unwrap();
    req.extensions_mut().insert(u.clone());
    req.extensions_mut().insert(cred.clone());
    let (parts, ()) = req.into_parts();

    let actor = RequestActor::from_parts(&parts).expect("authenticated");
    assert_eq!(actor.id, u.id, "derefs to the user");
    assert_eq!(actor.credential, Some(cred));
    assert_eq!(
        actor.context.ip.as_deref(),
        Some("203.0.113.9"),
        "the hop the load balancer appended, not the one the caller wrote"
    );
    assert_eq!(actor.context.user_agent.as_deref(), Some("oxyc/0.5.0"));
    assert_eq!(actor.context.request_id.as_deref(), Some("req-1"));

    let e = AuditEntry::for_request(&actor, "x");
    assert_eq!(e.context.ip.as_deref(), Some("203.0.113.9"));
}

#[test]
fn an_unauthenticated_request_has_no_actor() {
    let (parts, ()) = Request::new(()).into_parts();
    assert!(RequestActor::from_parts(&parts).is_none());
}

#[test]
fn a_long_forwarded_for_is_bounded_in_the_audit_context() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "9".repeat(4000).parse().unwrap());
    let actor = RequestActor::for_user(user(), &headers);
    assert_eq!(actor.context.ip.unwrap().len(), 64);
}

#[test]
fn a_request_with_no_forwarded_for_records_no_address() {
    let actor = RequestActor::for_user(user(), &HeaderMap::new());
    assert_eq!(actor.context.ip, None);
}

#[test]
fn a_long_user_agent_is_bounded() {
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", "x".repeat(4000).parse().unwrap());
    assert_eq!(user_agent(&headers).unwrap().len(), MAX_USER_AGENT_CHARS);
}
