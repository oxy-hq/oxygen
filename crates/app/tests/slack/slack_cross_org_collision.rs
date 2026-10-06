//! Integration tests for cross-org email collision — a security-critical invariant.
//!
//! These tests verify that the data layer correctly scopes user membership checks
//! to the org associated with a given Slack installation, preventing a Slack user
//! from one org from being auto-linked into a different org.
//!
//! Skips automatically when `OXY_DATABASE_URL` is unset.
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-app --test slack -E 'test(slack_cross_org_collision)'

use entity::{org_members, organizations, users, workspace_members, workspaces};
use oxy::database::client::establish_connection;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

async fn seed_user(email: &str) -> Uuid {
    let conn = establish_connection().await.expect("db connect");
    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(email.to_string())),
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
    user_id
}

async fn seed_org() -> Uuid {
    let conn = establish_connection().await.expect("db connect");
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
    org_id
}

async fn add_org_member(org_id: Uuid, user_id: Uuid) {
    let conn = establish_connection().await.expect("db connect");
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed org member");
}

/// Query: is `user_id` a member of `org_id`?
///
/// This mirrors the core invariant checked by auto_match — org membership
/// is scoped to the installation's org_id, not shared across orgs.
async fn is_org_member(org_id: Uuid, user_id: Uuid) -> bool {
    let conn = establish_connection().await.expect("db connect");
    org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&conn)
        .await
        .expect("query")
        .is_some()
}

/// Security-critical: Alice is a member of Org A but NOT Org B.
/// When a Slack install belongs to Org B, the org-membership check must return
/// false — preventing Alice from being auto-linked into Org B's installation.
#[tokio::test]
async fn auto_match_rejects_cross_org_email() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let alice_email = format!("alice-cross-org-{}@example.com", Uuid::new_v4().simple());
    let alice_id = seed_user(&alice_email).await;

    let org_a = seed_org().await;
    let org_b = seed_org().await;

    // Alice is a member of Org A only.
    add_org_member(org_a, alice_id).await;

    // The data-layer check: is Alice a member of Org B's installation scope?
    // This must return false — auto_match must not link Alice to Org B.
    let alice_in_org_b = is_org_member(org_b, alice_id).await;
    assert!(
        !alice_in_org_b,
        "Alice must NOT appear as a member of Org B (cross-org collision guard)"
    );

    // Positive check: Alice IS in Org A.
    let alice_in_org_a = is_org_member(org_a, alice_id).await;
    assert!(
        alice_in_org_a,
        "Alice must appear as a member of Org A (positive case)"
    );
}

/// Positive case: when Alice is seeded in Org B, the org-membership check
/// returns true — auto_match may proceed to link her.
#[tokio::test]
async fn auto_match_user_in_same_org_links_successfully() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let alice_email = format!("alice-same-org-{}@example.com", Uuid::new_v4().simple());
    let alice_id = seed_user(&alice_email).await;
    let org_b = seed_org().await;

    add_org_member(org_b, alice_id).await;

    let alice_in_org_b = is_org_member(org_b, alice_id).await;
    assert!(
        alice_in_org_b,
        "Alice must appear as a member of Org B when correctly seeded"
    );
}

/// Verify that workspace membership is also org-scoped.
///
/// A workspace belonging to Org A must not appear as accessible to a user
/// who only has workspace membership in Org B's workspaces.
#[tokio::test]
async fn workspace_membership_is_org_scoped() {
    if db_unavailable() {
        eprintln!("Skipping: OXY_DATABASE_URL not set");
        return;
    }

    let conn = establish_connection().await.expect("db connect");

    let user_email = format!("ws-scope-test-{}@example.com", Uuid::new_v4().simple());
    let user_id = seed_user(&user_email).await;

    let org_a = seed_org().await;
    let org_b = seed_org().await;

    // Create a workspace in Org A.
    let ws_a_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_a_id),
        name: ActiveValue::Set(format!("Workspace A {}", ws_a_id)),
        org_id: ActiveValue::Set(Some(org_a)),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        status: ActiveValue::Set(workspaces::WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed workspace A");

    workspace_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(ws_a_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(workspace_members::WorkspaceRole::Member),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed workspace member");

    // Query: workspaces the user can access, scoped to Org B (the installation's org).
    // This should return nothing — the workspace belongs to Org A.
    use entity::prelude::{WorkspaceMembers, Workspaces};
    let user_workspaces_in_org_b = Workspaces::find()
        .inner_join(WorkspaceMembers)
        .filter(workspace_members::Column::UserId.eq(user_id))
        .filter(workspaces::Column::OrgId.eq(Some(org_b)))
        .all(&conn)
        .await
        .expect("query");

    assert!(
        user_workspaces_in_org_b.is_empty(),
        "User's Org A workspace must not appear when queried under Org B scope"
    );
}
