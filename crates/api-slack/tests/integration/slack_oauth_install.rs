//! Integration tests for the Slack OAuth install service layer.
//!
//! These tests drive `InstallationsService` directly against a real PostgreSQL
//! database. They skip automatically when `OXY_DATABASE_URL` is unset (i.e.,
//! in pure unit-test builds).
//!
//! To run locally: `OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-api-slack --test integration -E 'test(slack_oauth_install)'`

use entity::{org_members, organizations, users};
use oxy::database::client::establish_connection;
use oxy_api_slack::services::installations::{InstallationsService, UpsertInstallation};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use uuid::Uuid;

/// Skip helper — returns true when OXY_DATABASE_URL is not set.
fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

/// Set a test-stable encryption key so OrgSecretsService can encrypt/decrypt.
fn set_test_encryption_key() {
    use base64::{Engine as _, engine::general_purpose};
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([42u8; 32]),
        );
    }
}

/// Seed a minimal user + org + membership, returning (org_id, user_id).
async fn seed_org_with_admin() -> (Uuid, Uuid) {
    let conn = establish_connection().await.expect("db connect");

    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("test-{}@example.com", user_id))),
        name: ActiveValue::Set("Test User".into()),
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
        name: ActiveValue::Set(format!("Test Org {}", org_id)),
        slug: ActiveValue::Set(format!("test-org-{}", org_id)),
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

fn make_upsert(org_id: Uuid, user_id: Uuid, team_id: &str) -> UpsertInstallation {
    UpsertInstallation {
        org_id,
        team_id: team_id.to_string(),
        team_name: format!("Team {team_id}"),
        enterprise_id: None,
        bot_user_id: format!("B_{team_id}"),
        bot_token: format!("xoxb-fake-token-{}", Uuid::new_v4()),
        scopes: "chat:write,users:read".into(),
        installed_by_user_id: user_id,
        installed_by_slack_user_id: format!("U{}", &user_id.simple().to_string()[..8]),
    }
}

#[tokio::test]
async fn install_happy_path() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);

    let row = InstallationsService::upsert(make_upsert(org_id, user_id, &team_id))
        .await
        .expect("upsert should succeed");

    assert_eq!(row.org_id, org_id);
    assert_eq!(row.slack_team_id, team_id);
    assert!(row.revoked_at.is_none(), "should not be revoked");

    // Verify we can look it up.
    let found = InstallationsService::find_active_by_org(org_id)
        .await
        .expect("lookup");
    assert!(found.is_some(), "should find active installation");
    assert_eq!(found.unwrap().id, row.id);
}

#[tokio::test]
async fn install_cross_org_conflict() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_a, user_a) = seed_org_with_admin().await;
    let (org_b, user_b) = seed_org_with_admin().await;
    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);

    InstallationsService::upsert(make_upsert(org_a, user_a, &team_id))
        .await
        .expect("first upsert should succeed");

    // Try installing same team_id into org B — should fail with cross-org message.
    let err = InstallationsService::upsert(make_upsert(org_b, user_b, &team_id))
        .await
        .expect_err("cross-org install should fail");

    let msg = format!("{err}");
    assert!(
        msg.contains("already connected to a different Oxygen org"),
        "error should mention cross-org conflict, got: {msg}"
    );
}

#[tokio::test]
async fn reinstall_same_org_updates() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);

    let row1 = InstallationsService::upsert(make_upsert(org_id, user_id, &team_id))
        .await
        .expect("first upsert");

    // Re-install with different token.
    let mut reinstall = make_upsert(org_id, user_id, &team_id);
    reinstall.bot_token = "xoxb-updated-token".to_string();
    let row2 = InstallationsService::upsert(reinstall)
        .await
        .expect("re-upsert should succeed");

    // Same row id — it updated in place.
    assert_eq!(row1.id, row2.id, "should update the existing row");

    // Only one active install.
    let conn = establish_connection().await.expect("db");
    use entity::prelude::SlackInstallations;
    use entity::slack_installations;
    use sea_orm::{ColumnTrait, QueryFilter};
    let all = SlackInstallations::find()
        .filter(slack_installations::Column::OrgId.eq(org_id))
        .filter(slack_installations::Column::RevokedAt.is_null())
        .all(&conn)
        .await
        .expect("query");
    assert_eq!(
        all.len(),
        1,
        "exactly one active installation after reinstall"
    );

    let decrypted = InstallationsService::decrypt_bot_token(&row2)
        .await
        .expect("decrypt");
    assert_eq!(decrypted, "xoxb-updated-token");
}
