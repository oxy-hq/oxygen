//! Restricted apps disappear from the launcher for people who can't open them.
//!
//! The access GATE (`user_can_access_app`) and the DISCOVERY filter
//! (`published_app_summaries`) are two different code paths that must agree. Before
//! this filter existed, a restricted app rendered as a card for every org member and
//! 403'd on click — the worst of both worlds: the app's name leaked, and the person
//! who saw it couldn't use it.
//!
//! These tests drive the real reader against real seeded rows, through both grant
//! paths (a direct `app_members` row and an `org_teams` team), because a filter that
//! only understands one of them fails silently for the other.

use crate::common::test_db;
use entity::{
    app_members, app_team_grants, apps, org_members, org_members::OrgRole, org_team_members,
    org_teams, organizations, users, workspaces,
};
use oxy_app::server::api::workspace_custom_apps::{Viewer, published_app_summaries};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

// ── Seeding ─────────────────────────────────────────────────────────────────

async fn seed_user(conn: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("vis-{id}@example.com");
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Visibility Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed user");
    (id, email)
}

async fn seed_org(conn: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Visibility Org".into()),
        slug: ActiveValue::Set(format!("vis-org-{id}")),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");
    id
}

async fn seed_workspace(conn: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Visibility Workspace".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed workspace");
    id
}

async fn seed_member(conn: &DatabaseConnection, org_id: Uuid, user_id: Uuid, role: OrgRole) {
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org member");
}

/// A PUBLISHED app — the launcher only ever lists published ones, so an unpublished
/// app would pass these tests for the wrong reason.
async fn seed_app(
    conn: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    name: &str,
    restricted: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(format!("vis-app-{id}")),
        name: ActiveValue::Set(name.to_string()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("vis/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set(if restricted { "members" } else { "org" }.to_string()),
        published_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app");
    id
}

async fn seed_app_member(conn: &DatabaseConnection, app_id: Uuid, user_id: Uuid, role: &str) {
    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role.into()),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed app member");
}

async fn seed_team_with_grant(
    conn: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Uuid {
    let team_id = Uuid::new_v4();
    org_teams::ActiveModel {
        id: ActiveValue::Set(team_id),
        org_id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("vis-team-{team_id}")),
        description: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed team");
    org_team_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        team_id: ActiveValue::Set(team_id),
        user_id: ActiveValue::Set(user_id),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed team member");
    app_team_grants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        team_id: ActiveValue::Set(team_id),
        role: ActiveValue::Set(role.into()),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed team grant");
    team_id
}

/// The app names a viewer would see on their launcher.
async fn visible_to(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    who: (Uuid, &str),
) -> Vec<String> {
    let viewer = Viewer {
        id: who.0,
        email: who.1,
    };
    published_app_summaries(conn, workspace_id, Some(viewer))
        .await
        .expect("summaries")
        .into_iter()
        .map(|s| s.name)
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_restricted_app_is_hidden_from_a_plain_member_and_shown_to_a_grantee() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Open App", false).await;
    let locked = seed_app(&conn, org, ws, "Locked App", true).await;

    // A plain org member sees the open app, never the restricted one. Before the
    // filter this listed both, and clicking the second one 403'd.
    let (outsider, outsider_email) = seed_user(&conn).await;
    seed_member(&conn, org, outsider, OrgRole::Member).await;
    let seen = visible_to(&conn, ws, (outsider, &outsider_email)).await;
    assert!(
        seen.contains(&"Open App".to_string()),
        "an unrestricted app must stay visible to every org member: {seen:?}"
    );
    assert!(
        !seen.contains(&"Locked App".to_string()),
        "a restricted app must not appear for someone with no grant: {seen:?}"
    );

    // The same member, granted directly, now sees it.
    let (grantee, grantee_email) = seed_user(&conn).await;
    seed_member(&conn, org, grantee, OrgRole::Member).await;
    seed_app_member(&conn, locked, grantee, "member").await;
    let seen = visible_to(&conn, ws, (grantee, &grantee_email)).await;
    assert!(
        seen.contains(&"Locked App".to_string()),
        "a direct app_members grant must reveal the app: {seen:?}"
    );
}

#[tokio::test]
async fn a_team_grant_reveals_the_app_just_like_a_direct_one() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let locked = seed_app(&conn, org, ws, "Team App", true).await;

    let (user, email) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Member).await;
    assert!(
        !visible_to(&conn, ws, (user, &email))
            .await
            .contains(&"Team App".to_string()),
        "precondition: no grant yet, so the app must be hidden"
    );

    seed_team_with_grant(&conn, org, locked, user, "member").await;
    assert!(
        visible_to(&conn, ws, (user, &email))
            .await
            .contains(&"Team App".to_string()),
        "a team grant must reveal the app — the discovery filter has to understand \
         BOTH grant paths, not just direct rows"
    );
}

#[tokio::test]
async fn org_officers_keep_break_glass_visibility() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Officer App", true).await;

    // An org can't lock its own officers out of its apps — the same break-glass
    // term `Ring::AppAccess` grants, applied to discovery so the card is there too.
    for role in [OrgRole::Owner, OrgRole::Admin] {
        let label = format!("{role:?}");
        let (id, email) = seed_user(&conn).await;
        seed_member(&conn, org, id, role).await;
        let seen = visible_to(&conn, ws, (id, &email)).await;
        assert!(
            seen.contains(&"Officer App".to_string()),
            "an org {label} must still see a restricted app: {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_grant_in_another_org_reveals_nothing() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let locked = seed_app(&conn, org, ws, "Tenant App", true).await;

    // Someone granted the app but who is NOT a member of its org: the model ANDs the
    // grant with org membership, so this must reveal nothing. A grant narrows an org;
    // it is not a way into one.
    let (foreigner, foreign_email) = seed_user(&conn).await;
    let other_org = seed_org(&conn).await;
    seed_member(&conn, other_org, foreigner, OrgRole::Owner).await;
    seed_app_member(&conn, locked, foreigner, "admin").await;

    let seen = visible_to(&conn, ws, (foreigner, &foreign_email)).await;
    assert!(
        !seen.contains(&"Tenant App".to_string()),
        "a grant held by a non-member must not reveal the app — this is the gap where \
         the bundle used to load and every data query 403'd: {seen:?}"
    );
}

#[tokio::test]
async fn an_unauthenticated_viewer_sees_no_restricted_apps() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Public-ish App", false).await;
    seed_app(&conn, org, ws, "Secret App", true).await;

    // No viewer → fail closed. Discovery is not an access decision, so a wrong deny
    // costs nobody their app, while a wrong allow leaks the app's existence.
    let names: Vec<String> = published_app_summaries(&conn, ws, None)
        .await
        .expect("summaries")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.contains(&"Public-ish App".to_string()));
    assert!(
        !names.contains(&"Secret App".to_string()),
        "an absent viewer must not be shown restricted apps: {names:?}"
    );
}

#[tokio::test]
async fn an_all_open_workspace_needs_no_authz_facts_at_all() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Open One", false).await;
    seed_app(&conn, org, ws, "Open Two", false).await;

    // The launcher's fast path: with nothing restricted, the filter short-circuits
    // before it reads facts, so no principal-facts load happens at all. A user who
    // doesn't exist as an org member — or at all — still sees every open app, which
    // is only possible if the loader was never consulted.
    //
    // This pins the `oxy-customer-apps-perf` optimization behaviorally: if someone
    // reintroduces an unconditional `load_principal_facts_scoped`, this still passes,
    // but if they make the *filter* consult facts for unrestricted apps, it fails.
    let ghost = Uuid::new_v4();
    let names = visible_to(&conn, ws, (ghost, "ghost@example.com")).await;
    assert_eq!(
        names.len(),
        2,
        "unrestricted apps must list without any authz facts: {names:?}"
    );
}

#[tokio::test]
async fn one_restricted_app_does_not_hide_the_open_ones() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Open", false).await;
    seed_app(&conn, org, ws, "Locked", true).await;

    // The mixed workspace — the case where facts DO load. The restricted app is
    // filtered for a member with no grant, but the open one is unaffected, so
    // restricting one app can never quietly hide the rest.
    let (member, email) = seed_user(&conn).await;
    seed_member(&conn, org, member, OrgRole::Member).await;
    let names = visible_to(&conn, ws, (member, &email)).await;
    assert_eq!(names, vec!["Open".to_string()]);
}

#[tokio::test]
async fn each_summary_reports_its_own_visibility() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    seed_app(&conn, org, ws, "Open", false).await;
    seed_app(&conn, org, ws, "Locked", true).await;

    // An officer sees both, which is what makes this assertable in one pass — and
    // an officer is exactly who the field exists for. The launcher renders the
    // access control off `visibility`, so a summary that always said "org" would
    // show a restricted app as open and offer to "restrict" something already
    // restricted, with nothing on screen contradicting it.
    let (owner, email) = seed_user(&conn).await;
    seed_member(&conn, org, owner, OrgRole::Owner).await;
    let by_name: std::collections::HashMap<String, String> = published_app_summaries(
        &conn,
        ws,
        Some(Viewer {
            id: owner,
            email: &email,
        }),
    )
    .await
    .expect("summaries")
    .into_iter()
    .map(|s| (s.name, s.visibility))
    .collect();

    assert_eq!(by_name.get("Open").map(String::as_str), Some("org"));
    assert_eq!(by_name.get("Locked").map(String::as_str), Some("members"));
}
