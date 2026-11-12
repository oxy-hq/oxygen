//! Integration tests for the Slack user links service layer.
//!
//! These tests drive `UserLinksService` directly against a real PostgreSQL
//! database. They skip automatically when `OXY_DATABASE_URL` is unset (i.e.,
//! in pure unit-test builds).
//!
//! To run locally: `OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-api-slack --test integration -E 'test(slack_user_links)'`

use entity::{org_members, organizations, users};
use oxy::database::client::establish_connection;
use oxy_api_slack::services::installations::{InstallationsService, UpsertInstallation};
use oxy_api_slack::services::user_links::{CreateLink, LinkMethod, UserLinksService};
use sea_orm::{ActiveModelTrait, ActiveValue};
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

/// Seed a minimal user + org + membership + slack installation.
/// Returns (installation_id, org_id, user_id).
async fn seed_installation() -> (Uuid, Uuid, Uuid) {
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

    let team_id = format!("T{}", &Uuid::new_v4().simple().to_string()[..8]);
    let installation = InstallationsService::upsert(UpsertInstallation {
        org_id,
        team_id: team_id.clone(),
        team_name: format!("Team {team_id}"),
        enterprise_id: None,
        bot_user_id: format!("B_{team_id}"),
        bot_token: format!("xoxb-fake-token-{}", Uuid::new_v4()),
        scopes: "chat:write,users:read".into(),
        installed_by_user_id: user_id,
        installed_by_slack_user_id: format!("U{}", &user_id.simple().to_string()[..8]),
    })
    .await
    .expect("seed installation");

    (installation.id, org_id, user_id)
}

#[tokio::test]
async fn create_find_touch_delete() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (installation_id, _org_id, user_id) = seed_installation().await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);

    // create
    let link = UserLinksService::create(CreateLink {
        installation_id,
        slack_user_id: slack_user_id.clone(),
        oxy_user_id: user_id,
        link_method: LinkMethod::EmailAuto,
    })
    .await
    .expect("create link");

    assert_eq!(link.installation_id, installation_id);
    assert_eq!(link.slack_user_id, slack_user_id);
    assert_eq!(link.oxy_user_id, user_id);
    assert_eq!(link.link_method, "email_auto");

    let found = UserLinksService::find(installation_id, &slack_user_id)
        .await
        .expect("find link");
    assert!(found.is_some(), "should find the link");
    let found = found.unwrap();
    assert_eq!(found.id, link.id);

    // touch_last_seen (just verify it doesn't error)
    UserLinksService::touch_last_seen(link.id)
        .await
        .expect("touch_last_seen");

    UserLinksService::delete(link.id)
        .await
        .expect("delete link");

    let after_delete = UserLinksService::find(installation_id, &slack_user_id)
        .await
        .expect("find after delete");
    assert!(after_delete.is_none(), "link should be gone after delete");
}

#[tokio::test]
async fn find_missing_returns_none() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (installation_id, _org_id, _user_id) = seed_installation().await;
    let result = UserLinksService::find(installation_id, "U_nonexistent_user")
        .await
        .expect("find should not error");
    assert!(result.is_none(), "missing link should return None");
}

#[tokio::test]
async fn magic_link_method_stored_correctly() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (installation_id, _org_id, user_id) = seed_installation().await;
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);

    let link = UserLinksService::create(CreateLink {
        installation_id,
        slack_user_id: slack_user_id.clone(),
        oxy_user_id: user_id,
        link_method: LinkMethod::MagicLink,
    })
    .await
    .expect("create magic link");

    assert_eq!(link.link_method, "magic_link");

    UserLinksService::delete(link.id).await.expect("cleanup");
}
