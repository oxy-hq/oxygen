//! The lifecycle audit rows: what they name, and that they never carry a token.

use super::*;
use entity::users::UserStatus;
use oxy_app_core::audit::{ActorType, AuditContext};
use oxy_auth::token::{CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;

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

fn created(plaintext: &str) -> CreateApiKeyResponse {
    CreateApiKeyResponse {
        id: Uuid::new_v4(),
        key: plaintext.to_string(),
        name: "deploy".into(),
        expires_at: None,
        created_at: Utc::now(),
        kind: StoredKind::LegacyKey,
        display_prefix: oxy_auth::token::format::display_prefix(plaintext),
    }
}

#[test]
fn a_created_row_names_the_token_and_its_scope_but_never_the_token() {
    let request = RequestActor::session(user());
    let org = Uuid::new_v4();
    let ws = Uuid::new_v4();
    let actor = Actor {
        request: &request,
        workspace_id: ws,
        org_id: Some(org),
    };
    let secret = oxy_auth::token::format::generate_legacy_key();
    let c = created(&secret);
    let e = created_entry(&actor, &c);

    assert_eq!(e.action, "token.created");
    assert_eq!(e.target_type.as_deref(), Some(TOKEN_TARGET_TYPE));
    assert_eq!(e.target_id, Some(c.id.to_string()));
    assert_eq!(e.org_id, Some(org));
    assert_eq!(e.workspace_id, Some(ws));
    assert_eq!(e.actor_user_id, Some(request.id));
    assert_eq!(e.actor_type, ActorType::User);
    let meta = e.effective_metadata();
    assert_eq!(meta["token_id"], json!(c.id), "the token it is about");
    assert_eq!(meta["token_kind"], "legacy_key");
    assert_eq!(meta["source"], "legacy_endpoint");
    // The whole entry, every field, must not contain the secret.
    assert!(
        !format!("{e:?} {meta}").contains(&secret),
        "an audit row must never carry the token"
    );
}

#[test]
fn a_call_made_with_a_legacy_key_is_recorded_as_that_key() {
    let u = user();
    let cred = CredentialContext {
        token_id: Uuid::new_v4(),
        kind: StoredKind::LegacyKey,
        principal_user_id: u.id,
        all_access: true,
        platform: true,
        partner: true,
        name: "old".into(),
        display_prefix: "oxy_".into(),
        legacy_api_key_id: None,
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: Vec::new(),
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
    };
    let request = RequestActor {
        user: u,
        credential: Some(cred.clone()),
        context: AuditContext::default(),
    };
    let actor = Actor {
        request: &request,
        workspace_id: Uuid::new_v4(),
        org_id: None,
    };
    let c = created("oxy_0123456789abcdef0123456789abcdef");
    let e = created_entry(&actor, &c);
    assert_eq!(e.actor_type, ActorType::ApiKey);
    let meta = e.effective_metadata();
    // The stamp names the key that ACTED; the key acted ON stays the target.
    assert_eq!(meta["token_id"], json!(cred.token_id));
    assert_eq!(meta["token_kind"], "legacy_key");
    assert_eq!(meta["api_key_id"], json!(c.id));
    assert_eq!(e.target_id, Some(c.id.to_string()));
    assert_eq!(
        e.org_id, None,
        "no org means an unchained event, not a guess"
    );
}

#[test]
fn an_extended_row_records_the_old_and_new_expiry() {
    let request = RequestActor::session(user());
    let actor = Actor {
        request: &request,
        workspace_id: Uuid::new_v4(),
        org_id: Some(Uuid::new_v4()),
    };
    let now = Utc::now();
    let before = now - chrono::Duration::days(1);
    let after = now + chrono::Duration::days(30);
    let key_id = Uuid::new_v4();
    let extended = ExtendedApiKey {
        api_key: entity::api_keys::Model {
            id: key_id,
            user_id: request.id,
            key_hash: "oxy_0123456789abcdef0123456789abcdef".into(),
            name: "deploy".into(),
            expires_at: Some(after.fixed_offset()),
            last_used_at: None,
            created_at: before.fixed_offset(),
            updated_at: now.fixed_offset(),
            is_active: true,
            project_id: Uuid::new_v4(),
            app_id: None,
        },
        previous_expires_at: Some(before),
        expires_at: Some(after),
        mirror: None,
    };
    let e = extended_entry(&actor, &extended);
    assert_eq!(e.action, "token.extended");
    assert_eq!(e.target_id, Some(key_id.to_string()));
    assert_eq!(e.before, Some(json!({ "expires_at": before.to_rfc3339() })));
    assert_eq!(e.after, Some(json!({ "expires_at": after.to_rfc3339() })));
    // What the activity endpoint shows as "extended from X to Y".
    let meta = e.effective_metadata();
    assert_eq!(meta["token_id"], json!(key_id));
    assert_eq!(meta["old_expires_at"], json!(before.to_rfc3339()));
    assert_eq!(meta["new_expires_at"], json!(after.to_rfc3339()));
    assert!(!format!("{e:?}").contains("0123456789abcdef0123456789abcdef"));
}

#[test]
fn extending_to_never_records_null() {
    assert_eq!(rfc3339(None), Value::Null);
}
