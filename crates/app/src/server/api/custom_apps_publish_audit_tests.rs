//! What an audit row says of an app publish token (`stamped`). The rows
//! themselves are driven over the route in `tests/custom_apps/publish_audit.rs`.

use entity::users::UserStatus;
use oxy_auth::types::AuthenticatedUser;
use uuid::Uuid;

use super::*;

fn minter() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("engineer@oxy.tech".to_string()),
        name: "Engineer".to_string(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

fn marker(machine_identity: Option<&str>) -> AppPublishTokenAuth {
    AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: None,
        machine_identity: machine_identity.map(str::to_string),
    }
}

fn row(marker: &AppPublishTokenAuth, name: &str) -> app_publish_tokens::Model {
    app_publish_tokens::Model {
        id: marker.token_id,
        name: name.to_string(),
        token_hash: "c0ffee".repeat(10),
        token_prefix: "oxypublish_ab12cd34".to_string(),
        created_by: Some(Uuid::new_v4()),
        created_at: chrono::Utc::now().fixed_offset(),
        last_used_at: None,
        revoked_at: None,
        app_id: None,
        expires_at: None,
    }
}

fn entry(user: AuthenticatedUser) -> AuditEntry {
    AuditEntry::for_request(&RequestActor::session(user), PUBLISHED)
}

#[test]
fn a_publish_tokens_row_says_a_key_acted_and_names_the_token() {
    let user = minter();
    let marker = marker(None);
    let token = row(&marker, "ci-publish");
    let detail = json!({ "build_id": "b-1" });

    // Without the stamp the row would read as the minter in a browser.
    assert_eq!(entry(user.clone()).actor_type, ActorType::User);

    let stamped = stamped(entry(user.clone()), &marker, Some(&token), detail);
    assert_eq!(stamped.actor_type, ActorType::ApiKey);
    assert_eq!(stamped.actor_user_id, Some(user.id), "the minter answers");
    let written = stamped.effective_metadata();
    assert_eq!(written["token_id"], json!(marker.token_id));
    assert_eq!(written["token_kind"], "app_publish_token");
    assert_eq!(written["token_name"], "ci-publish");
    assert_eq!(written["display_prefix"], "oxypublish_ab12cd34");
    assert_eq!(written["build_id"], "b-1", "the caller's detail is kept");
    assert!(!written.to_string().contains(&token.token_hash));
}

#[test]
fn a_machine_publish_records_the_workflow_and_never_the_nil_user_id() {
    let marker = marker(Some("acme/app@refs/heads/main"));
    let machine = AuthenticatedUser::machine_publisher();
    assert!(machine.id.is_nil());

    let stamped = stamped(entry(machine), &marker, None, json!({ "build_id": "b-2" }));
    assert_eq!(stamped.actor_user_id, None);
    assert_eq!(stamped.actor_type, ActorType::ApiKey);
    let written = stamped.effective_metadata();
    assert_eq!(written["token_name"], "acme/app@refs/heads/main");
    assert_eq!(written["token_id"], json!(marker.token_id));
}

#[test]
fn a_token_whose_row_cannot_be_read_is_still_named_by_id() {
    let marker = marker(None);
    let stamped = stamped(entry(minter()), &marker, None, Value::Null);
    let written = stamped.effective_metadata();
    assert_eq!(written["token_id"], json!(marker.token_id));
    assert_eq!(written["token_kind"], "app_publish_token");
    assert!(written.get("token_name").is_none());
    assert!(written.get("display_prefix").is_none());
}

#[test]
fn the_callers_detail_cannot_overwrite_the_stamp() {
    let marker = marker(None);
    let forged = json!({ "token_id": Uuid::new_v4(), "token_kind": "personal" });
    let stamped = stamped(entry(minter()), &marker, None, forged);
    let written = stamped.effective_metadata();
    assert_eq!(written["token_id"], json!(marker.token_id));
    assert_eq!(written["token_kind"], "app_publish_token");
}
