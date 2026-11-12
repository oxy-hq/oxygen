//! Integration tests for the Slack OAuth state lifecycle.
//!
//! Drives `OauthStateService` directly against a real PostgreSQL database.
//! Skips automatically when `OXY_DATABASE_URL` is unset.
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-api-slack --test integration -E 'test(slack_oauth_state)'

use chrono::{Duration, Utc};
use entity::{org_members, organizations, slack_oauth_states, users};
use oxy::database::client::establish_connection;
use oxy_api_slack::oauth::state::{
    CreateInstallState, CreateUserLinkState, OauthStateService, StateKind,
};
use sea_orm::{ActiveModelTrait, ActiveValue};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

/// Seed a minimal user + org + membership, returning (org_id, user_id).
async fn seed_org_with_admin() -> (Uuid, Uuid) {
    let conn = establish_connection().await.expect("db connect");

    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("oauth-state-test-{}@example.com", user_id))),
        name: ActiveValue::Set("OAuth State Test User".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed user");

    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("OAuth State Test Org {}", org_id)),
        slug: ActiveValue::Set(format!("oauth-state-test-org-{}", org_id)),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed org");

    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(org_members::OrgRole::Admin),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed member");

    (org_id, user_id)
}

#[tokio::test]
async fn create_install_and_consume() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let (org_id, user_id) = seed_org_with_admin().await;

    let nonce = OauthStateService::create_install(CreateInstallState {
        org_id,
        oxy_user_id: user_id,
    })
    .await
    .expect("create_install");
    assert_eq!(nonce.len(), 64, "nonce should be 64 hex chars");

    let row = OauthStateService::consume(&nonce, StateKind::Install)
        .await
        .expect("first consume should succeed");
    assert_eq!(row.org_id, Some(org_id));
    assert_eq!(row.oxy_user_id, Some(user_id));
    assert_eq!(row.kind, "install");

    // Second consume — should fail as already consumed.
    let err = OauthStateService::consume(&nonce, StateKind::Install)
        .await
        .expect_err("second consume should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("already consumed"),
        "expected 'already consumed' error, got: {msg}"
    );
}

#[tokio::test]
async fn consume_wrong_kind_fails() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let (org_id, user_id) = seed_org_with_admin().await;

    // Create an install state but try to consume as user_link.
    let nonce = OauthStateService::create_install(CreateInstallState {
        org_id,
        oxy_user_id: user_id,
    })
    .await
    .expect("create_install");

    let err = OauthStateService::consume(&nonce, StateKind::UserLink)
        .await
        .expect_err("wrong-kind consume should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("kind mismatch"),
        "expected 'kind mismatch' error, got: {msg}"
    );
}

#[tokio::test]
async fn consume_nonexistent_nonce_fails() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let random_nonce = hex::encode(rand::random::<[u8; 32]>());
    let err = OauthStateService::consume(&random_nonce, StateKind::Install)
        .await
        .expect_err("unknown nonce should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown state nonce"),
        "expected 'unknown state nonce' error, got: {msg}"
    );
}

#[tokio::test]
async fn expired_state_cannot_be_consumed() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let (org_id, user_id) = seed_org_with_admin().await;
    let conn = establish_connection().await.expect("db connect");

    // Insert an already-expired row directly via the ActiveModel.
    let nonce = hex::encode(rand::random::<[u8; 32]>());
    let past_expires_at: sea_orm::prelude::DateTimeWithTimeZone =
        (Utc::now() - Duration::hours(1)).into();

    slack_oauth_states::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        kind: ActiveValue::Set("install".into()),
        nonce: ActiveValue::Set(nonce.clone()),
        org_id: ActiveValue::Set(Some(org_id)),
        oxy_user_id: ActiveValue::Set(Some(user_id)),
        slack_team_id: ActiveValue::Set(None),
        slack_user_id: ActiveValue::Set(None),
        slack_channel_id: ActiveValue::Set(None),
        slack_thread_ts: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        expires_at: ActiveValue::Set(past_expires_at),
        consumed_at: ActiveValue::Set(None),
    }
    .insert(&conn)
    .await
    .expect("seed expired state");

    let err = OauthStateService::consume(&nonce, StateKind::Install)
        .await
        .expect_err("expired state should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("state expired"),
        "expected 'state expired' error, got: {msg}"
    );
}

#[tokio::test]
async fn user_link_state_creation_and_consume() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);

    let nonce = OauthStateService::create_user_link(CreateUserLinkState {
        slack_team_id: team_id.clone(),
        slack_user_id: slack_user_id.clone(),
        slack_channel_id: None,
        slack_thread_ts: None,
    })
    .await
    .expect("create_user_link");

    let row = OauthStateService::consume(&nonce, StateKind::UserLink)
        .await
        .expect("consume user_link state");

    assert_eq!(row.kind, "user_link");
    assert_eq!(row.slack_team_id, Some(team_id));
    assert_eq!(row.slack_user_id, Some(slack_user_id));
    assert!(row.org_id.is_none(), "user_link state has no org_id");
    assert!(
        row.oxy_user_id.is_none(),
        "user_link state has no oxy_user_id"
    );
}
