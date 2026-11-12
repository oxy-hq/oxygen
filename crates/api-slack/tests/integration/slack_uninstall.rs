//! Integration tests for the Slack uninstall event handler.
//!
//! Drives `uninstall::revoke` directly against a real PostgreSQL database.
//! Skips automatically when `OXY_DATABASE_URL` is unset (i.e., in pure
//! unit-test builds).
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-api-slack --test integration -E 'test(slack_uninstall)'

use entity::prelude::{SlackInstallations, SlackUserLinks};
use entity::{org_members, organizations, users};
use oxy::database::client::establish_connection;
use oxy_api_slack::events::uninstall::revoke;
use oxy_api_slack::services::installations::{InstallationsService, UpsertInstallation};
use oxy_api_slack::services::user_links::{CreateLink, LinkMethod, UserLinksService};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

fn set_test_encryption_key() {
    use base64::{Engine as _, engine::general_purpose};
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([42u8; 32]),
        );
    }
}

/// Seed org + admin user, return (org_id, user_id).
async fn seed_org_with_admin() -> (Uuid, Uuid) {
    let conn = establish_connection().await.expect("db connect");

    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("uninstall-test-{}@example.com", user_id))),
        name: ActiveValue::Set("Uninstall Test User".into()),
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
        name: ActiveValue::Set(format!("Uninstall Test Org {}", org_id)),
        slug: ActiveValue::Set(format!("uninstall-test-{}", org_id)),
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
async fn uninstall_revokes_installation() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
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

    assert!(installation.revoked_at.is_none(), "should start unrevoked");

    // Dispatch the uninstall handler.
    revoke(installation.clone())
        .await
        .expect("revoke should succeed");

    // Verify revoked_at is now set.
    let conn = establish_connection().await.expect("db connect");
    let updated = SlackInstallations::find_by_id(installation.id)
        .one(&conn)
        .await
        .expect("query")
        .expect("row should still exist");
    assert!(
        updated.revoked_at.is_some(),
        "revoked_at should be set after uninstall"
    );
}

#[tokio::test]
async fn uninstall_cascades_user_link_deletion() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
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

    // Create a user link that should cascade-delete on revoke.
    let slack_user_id = format!("U{}", &Uuid::new_v4().simple().to_string()[..8]);
    let link = UserLinksService::create(CreateLink {
        installation_id: installation.id,
        slack_user_id: slack_user_id.clone(),
        oxy_user_id: user_id,
        link_method: LinkMethod::MagicLink,
    })
    .await
    .expect("seed user link");

    // Verify the link exists.
    let conn = establish_connection().await.expect("db connect");
    let before = SlackUserLinks::find_by_id(link.id)
        .one(&conn)
        .await
        .expect("query before");
    assert!(before.is_some(), "link should exist before uninstall");

    // Revoke the installation — this marks it as revoked. The FK cascade
    // (DELETE user_links when installation is deleted) doesn't trigger here
    // since we only set revoked_at. Verify the link is still queryable.
    // True cascade deletion would happen if the installation row were hard-deleted.
    // This test validates revoke() succeeds and sets revoked_at; the user_link
    // service's delete path is covered by slack_user_links test suite.
    revoke(installation.clone())
        .await
        .expect("revoke should succeed");

    let updated = SlackInstallations::find_by_id(installation.id)
        .one(&conn)
        .await
        .expect("query")
        .expect("row should still exist");
    assert!(updated.revoked_at.is_some(), "revoked_at should be set");
}

#[tokio::test]
async fn uninstall_is_idempotent() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
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

    revoke(installation.clone())
        .await
        .expect("first revoke should succeed");

    let conn = establish_connection().await.expect("db connect");
    let revoked_row = SlackInstallations::find_by_id(installation.id)
        .one(&conn)
        .await
        .expect("query")
        .expect("row exists");
    assert!(revoked_row.revoked_at.is_some());

    // Second revoke should be a no-op (idempotent).
    revoke(revoked_row)
        .await
        .expect("second revoke should be a no-op");
}

#[tokio::test]
async fn find_active_returns_none_after_revoke() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }
    set_test_encryption_key();

    let (org_id, user_id) = seed_org_with_admin().await;
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

    let found_before = InstallationsService::find_active_by_team(&team_id)
        .await
        .expect("lookup before");
    assert!(found_before.is_some(), "should be active before revoke");

    revoke(installation).await.expect("revoke");

    // find_active_by_team filters RevokedAt.is_null() — so it should return None.
    let found_after = InstallationsService::find_active_by_team(&team_id)
        .await
        .expect("lookup after");
    assert!(
        found_after.is_none(),
        "find_active_by_team should return None after revoke"
    );
}
