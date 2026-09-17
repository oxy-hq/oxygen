//! The app-access **control plane**, end to end against a real PostgreSQL.
//!
//! `custom_app_visibility.rs` covers discovery (what the launcher shows).
//! `authz_loader_differential.rs` covers the fact loader. This file covers the part
//! in between, which is where the security-relevant decisions actually get written:
//!
//! - **Write validation** — a grantee must be a member of the app's org, a team must
//!   belong to it, roles and visibility come from a closed set, and the list is
//!   capped. Every one of these is a boundary someone could otherwise cross by
//!   hand-rolling a request; the UI can't be the enforcement.
//! - **Tenant isolation** — an app id or team id from another org must 404, never
//!   resolve. The org filter is the boundary, not the id's unguessability.
//! - **Replace semantics** — a save is a full replace, so a revoke actually revokes
//!   instead of merging.
//! - **The grant union on the shipped-gate side** — `has_app_grant` is the mirror of
//!   the loader's union. If the two disagree, the model and the gate disagree, which
//!   is exactly what `oxy-authz` exists to prevent.
//! - **Cascade** — deleting a team must revoke what it granted.
//!
//! These drive the real service functions, not HTTP, because the handlers are thin
//! (parse → gate → call service) and the gate is already pinned by the differential
//! suites. What is worth testing here is the behavior underneath.

use crate::common::test_db;
use entity::{
    apps, org_members, org_members::OrgRole, org_team_members, org_teams, organizations, users,
    workspaces,
};
use oxy_app::server::api::custom_apps_auth::{
    has_app_grant, resolve_app_role, resolve_org_role, resolve_org_standing, user_can_access_app,
};
use oxy_app::server::api::org_teams::dto::{GranteeRef, SetAppAccessRequest};
use oxy_app::server::api::org_teams::service;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

// ── Seeding ─────────────────────────────────────────────────────────────────

async fn seed_user(conn: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("acl-{id}@example.com");
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Access Control Test".into()),
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
        name: ActiveValue::Set("ACL Org".into()),
        slug: ActiveValue::Set(format!("acl-org-{id}")),
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
        name: ActiveValue::Set("ACL Workspace".into()),
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

async fn seed_app(conn: &DatabaseConnection, org_id: Uuid, workspace_id: Uuid) -> apps::Model {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(format!("acl-app-{id}")),
        name: ActiveValue::Set("ACL App".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("acl/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        published_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app")
}

async fn seed_team(conn: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    org_teams::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("acl-team-{id}")),
        description: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed team");
    id
}

async fn join_team(conn: &DatabaseConnection, team_id: Uuid, user_id: Uuid) {
    org_team_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        team_id: ActiveValue::Set(team_id),
        user_id: ActiveValue::Set(user_id),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("join team");
}

fn team_grant(id: Uuid, role: &str) -> GranteeRef {
    GranteeRef::Team {
        id,
        role: role.into(),
    }
}

fn user_grant(id: Uuid, role: &str) -> GranteeRef {
    GranteeRef::User {
        id,
        role: role.into(),
    }
}

fn restrict(grants: Vec<GranteeRef>) -> SetAppAccessRequest {
    SetAppAccessRequest {
        visibility: "members".into(),
        grants,
    }
}

/// Re-read the app row so assertions see the persisted `visibility`, not the
/// pre-write copy.
async fn reload(conn: &DatabaseConnection, app_id: Uuid) -> apps::Model {
    entity::prelude::Apps::find_by_id(app_id)
        .one(conn)
        .await
        .expect("reload app")
        .expect("app exists")
}

// ── Write validation: the boundaries the UI must not be the only guard for ──

#[tokio::test]
async fn a_grantee_who_is_not_an_org_member_is_rejected() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;

    // Belongs to a DIFFERENT org. `Ring::AppAccess` would deny them anyway, but
    // writing the row would create a grant that silently grants nothing — and the
    // admin would have no way to tell.
    let (outsider, _) = seed_user(&conn).await;
    let other_org = seed_org(&conn).await;
    seed_member(&conn, other_org, outsider, OrgRole::Owner).await;

    let err = service::write_access(
        &conn,
        &app,
        outsider,
        &restrict(vec![user_grant(outsider, "member")]),
    )
    .await
    .expect_err("a non-member grantee must be rejected");
    assert_eq!(err, axum::http::StatusCode::BAD_REQUEST);

    // And nothing was written — validation runs BEFORE the transaction.
    assert_eq!(reload(&conn, app.id).await.visibility, "org");
    assert!(
        service::read_access(&conn, &app)
            .await
            .unwrap()
            .grants
            .is_empty(),
        "a rejected write must not leave a partial grant list"
    );
}

#[tokio::test]
async fn a_team_from_another_org_is_rejected() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    // A team id is a UUID like any other; the org filter is what stops it crossing
    // the tenant boundary, not its unguessability.
    let foreign_org = seed_org(&conn).await;
    let foreign_team = seed_team(&conn, foreign_org).await;

    let err = service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(foreign_team, "member")]),
    )
    .await
    .expect_err("a team outside the app's org must be rejected");
    assert_eq!(err, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn role_and_visibility_come_from_a_closed_set() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let team = seed_team(&conn, org).await;

    // A role outside {admin, member} would trip the DB CHECK as a 500; rejecting at
    // the edge makes it the 400 it actually is.
    for bad_role in ["owner", "superuser", "ADMIN", ""] {
        let err = service::write_access(
            &conn,
            &app,
            actor,
            &restrict(vec![team_grant(team, bad_role)]),
        )
        .await
        .expect_err("role must be admin or member");
        assert_eq!(
            err,
            axum::http::StatusCode::BAD_REQUEST,
            "role {bad_role:?}"
        );
    }

    for bad_visibility in ["public", "private", "MEMBERS", ""] {
        let req = SetAppAccessRequest {
            visibility: bad_visibility.into(),
            grants: vec![],
        };
        let err = service::write_access(&conn, &app, actor, &req)
            .await
            .expect_err("visibility must be org or members");
        assert_eq!(
            err,
            axum::http::StatusCode::BAD_REQUEST,
            "visibility {bad_visibility:?}"
        );
    }
}

#[tokio::test]
async fn the_grant_list_is_capped() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    // 201 distinct team ids — over the 200 cap. Rejected on length before any of
    // them is looked up, so a runaway client can't turn this into 201 queries.
    let grants: Vec<GranteeRef> = (0..201)
        .map(|_| team_grant(Uuid::new_v4(), "member"))
        .collect();
    let err = service::write_access(&conn, &app, actor, &restrict(grants))
        .await
        .expect_err("an oversized grant list must be rejected");
    assert_eq!(err, axum::http::StatusCode::BAD_REQUEST);
}

// ── Tenant isolation ────────────────────────────────────────────────────────

#[tokio::test]
async fn an_app_id_from_another_org_does_not_resolve() {
    let conn = test_db().await;
    let mine = seed_org(&conn).await;
    let theirs = seed_org(&conn).await;
    let their_ws = seed_workspace(&conn, theirs).await;
    let their_app = seed_app(&conn, theirs, their_ws).await;

    // This is the check that makes the admin console's app-keyed routes safe: the
    // org is derived and then re-asserted, so a mismatched pair 404s.
    let err = service::load_app_in_org(&conn, mine, their_app.id)
        .await
        .expect_err("an app in another org must 404, not resolve");
    assert_eq!(err, axum::http::StatusCode::NOT_FOUND);

    // Sanity: it does resolve for its real owner.
    service::load_app_in_org(&conn, theirs, their_app.id)
        .await
        .expect("the owning org resolves its own app");
}

#[tokio::test]
async fn listing_teams_and_apps_is_scoped_to_one_org() {
    let conn = test_db().await;
    let mine = seed_org(&conn).await;
    let theirs = seed_org(&conn).await;
    let my_ws = seed_workspace(&conn, mine).await;
    let their_ws = seed_workspace(&conn, theirs).await;
    seed_app(&conn, mine, my_ws).await;
    seed_app(&conn, theirs, their_ws).await;
    let my_team = seed_team(&conn, mine).await;
    let their_team = seed_team(&conn, theirs).await;

    let teams = service::list_org_teams(&conn, mine).await.expect("teams");
    assert!(teams.iter().any(|t| t.id == my_team));
    assert!(
        !teams.iter().any(|t| t.id == their_team),
        "another org's team leaked into the grant picker"
    );

    let apps = service::list_org_apps_with_access(&conn, mine)
        .await
        .expect("apps");
    assert_eq!(apps.len(), 1, "the app list must be scoped to one org");
}

// ── Replace semantics ───────────────────────────────────────────────────────

#[tokio::test]
async fn saving_replaces_the_list_rather_than_merging_it() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let (alice, _) = seed_user(&conn).await;
    seed_member(&conn, org, alice, OrgRole::Member).await;
    let team_a = seed_team(&conn, org).await;
    let team_b = seed_team(&conn, org).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![
            team_grant(team_a, "admin"),
            user_grant(alice, "member"),
        ]),
    )
    .await
    .expect("first save");
    assert_eq!(
        service::read_access(&conn, &app)
            .await
            .unwrap()
            .grants
            .len(),
        2
    );

    // Second save names only team_b. If this merged instead of replacing, the
    // revoked grants would survive — a revoke that doesn't revoke.
    let after = service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team_b, "member")]),
    )
    .await
    .expect("second save");
    assert_eq!(after.grants.len(), 1);
    assert_eq!(after.grants[0].id, team_b);
    assert!(
        !after.grants.iter().any(|g| g.id == team_a || g.id == alice),
        "a replace must drop the grants it no longer names"
    );
}

#[tokio::test]
async fn reopening_an_app_to_the_org_keeps_the_list_for_later() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let team = seed_team(&conn, org).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("restrict");

    // Flipping back to 'org' with the same list keeps it — so an admin who
    // un-restricts and re-restricts doesn't have to rebuild the audience.
    //
    // A `member` grant is dormant while visibility is 'org' (everyone can open it
    // anyway) and applies again on re-restricting. An `admin` grant is NOT — it
    // stays live and keeps handing out the app's privileged surface, which is what
    // `opening_an_app_to_the_org_keeps_grants_and_their_admin_power` below pins and
    // why the dialog keeps showing the list on the open branch.
    let req = SetAppAccessRequest {
        visibility: "org".into(),
        grants: vec![team_grant(team, "member")],
    };
    let after = service::write_access(&conn, &app, actor, &req)
        .await
        .expect("reopen");
    assert_eq!(after.visibility, "org");
    assert_eq!(after.grants.len(), 1);
}

// ── The grant union, shipped-gate side ──────────────────────────────────────

#[tokio::test]
async fn has_app_grant_sees_both_paths() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    // No grant at all.
    let (nobody, _) = seed_user(&conn).await;
    seed_member(&conn, org, nobody, OrgRole::Member).await;

    // Direct row only.
    let (direct, _) = seed_user(&conn).await;
    seed_member(&conn, org, direct, OrgRole::Member).await;

    // Team only — the path a `has_app_grant` that read just `app_members` would
    // miss, silently reporting a team-granted user as having no grant.
    let (via_team, _) = seed_user(&conn).await;
    seed_member(&conn, org, via_team, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, via_team).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![
            team_grant(team, "admin"),
            user_grant(direct, "member"),
        ]),
    )
    .await
    .expect("save");

    assert!(!has_app_grant(&conn, nobody, app.id).await.unwrap());
    assert!(has_app_grant(&conn, direct, app.id).await.unwrap());
    assert!(
        has_app_grant(&conn, via_team, app.id).await.unwrap(),
        "a team-reached grant must count — the union is the whole point"
    );

    // A grant on a DIFFERENT app says nothing about this one.
    let other = seed_app(&conn, org, ws).await;
    assert!(!has_app_grant(&conn, via_team, other.id).await.unwrap());
}

#[tokio::test]
async fn ctx_user_app_role_sees_a_team_granted_member() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    let (plain, plain_email) = seed_user(&conn).await;
    seed_member(&conn, org, plain, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, plain).await;

    // A plain org member with no grant is neither.
    let (bystander, bystander_email) = seed_user(&conn).await;
    seed_member(&conn, org, bystander, OrgRole::Member).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("save");
    let saved = reload(&conn, app.id).await;

    // `ctx.user.appRole` is what an app gates its own admin surface on. A
    // team-granted user reporting `None` here would make team grants invisible to
    // every function that asks.
    assert_eq!(
        resolve_app_role(&conn, plain, &plain_email, &saved)
            .await
            .unwrap(),
        Some("member")
    );
    assert_eq!(
        resolve_app_role(&conn, bystander, &bystander_email, &saved)
            .await
            .unwrap(),
        None
    );
}

// ── The gate, end to end ────────────────────────────────────────────────────

#[tokio::test]
async fn the_access_gate_agrees_with_the_grant_list() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    let (granted, granted_email) = seed_user(&conn).await;
    seed_member(&conn, org, granted, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, granted).await;

    let (ungranted, ungranted_email) = seed_user(&conn).await;
    seed_member(&conn, org, ungranted, OrgRole::Member).await;

    let (officer, officer_email) = seed_user(&conn).await;
    seed_member(&conn, org, officer, OrgRole::Admin).await;

    // While open, every member gets in.
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    for (id, email) in [(&granted, &granted_email), (&ungranted, &ungranted_email)] {
        assert!(
            user_can_access_app(&conn, *id, email, &app).await.unwrap(),
            "an unrestricted app must admit every org member"
        );
    }

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("restrict");
    let saved = reload(&conn, app.id).await;

    assert!(
        user_can_access_app(&conn, granted, &granted_email, &saved)
            .await
            .unwrap(),
        "the team-granted member must still get in"
    );
    assert!(
        !user_can_access_app(&conn, ungranted, &ungranted_email, &saved)
            .await
            .unwrap(),
        "a member with no grant must be shut out — the whole point of restricting"
    );
    assert!(
        user_can_access_app(&conn, officer, &officer_email, &saved)
            .await
            .unwrap(),
        "org officers keep break-glass so an org can't lock itself out"
    );
}

#[tokio::test]
async fn deleting_a_team_revokes_what_it_granted() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    let (member, member_email) = seed_user(&conn).await;
    seed_member(&conn, org, member, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, member).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("grant");
    let saved = reload(&conn, app.id).await;
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    assert!(
        user_can_access_app(&conn, member, &member_email, &saved)
            .await
            .unwrap()
    );

    // `app_team_grants.team_id` cascades. Someone who could ONLY reach the app
    // through this team loses it — that is the intent, and it is why the delete
    // handler drops the access cache.
    org_teams::Entity::delete_by_id(team)
        .exec(&conn)
        .await
        .expect("delete team");
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();

    assert!(
        !has_app_grant(&conn, member, app.id).await.unwrap(),
        "deleting a team must revoke the grants it carried"
    );
    assert!(
        !user_can_access_app(&conn, member, &member_email, &saved)
            .await
            .unwrap(),
        "access must follow the revoked grant"
    );
}

#[tokio::test]
async fn leaving_a_team_revokes_access_without_touching_the_grant() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    let (member, member_email) = seed_user(&conn).await;
    seed_member(&conn, org, member, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, member).await;
    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("grant");
    let saved = reload(&conn, app.id).await;

    // Removing one person from the team must not disturb the app's grant — this is
    // the whole reason teams exist as an indirection.
    org_team_members::Entity::delete_many()
        .filter(org_team_members::Column::UserId.eq(member))
        .exec(&conn)
        .await
        .expect("leave team");
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();

    assert!(!has_app_grant(&conn, member, app.id).await.unwrap());
    assert!(
        !user_can_access_app(&conn, member, &member_email, &saved)
            .await
            .unwrap()
    );
    assert_eq!(
        service::read_access(&conn, &saved)
            .await
            .unwrap()
            .grants
            .len(),
        1,
        "the app's grant list is unchanged — only the team's roster moved"
    );
}

// ── Summary counts drive the settings list's badge ──────────────────────────

#[tokio::test]
async fn the_org_app_list_counts_both_grant_kinds() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let (alice, _) = seed_user(&conn).await;
    seed_member(&conn, org, alice, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;

    let before = service::list_org_apps_with_access(&conn, org)
        .await
        .unwrap();
    assert_eq!(before[0].visibility, "org");
    assert_eq!(before[0].grant_count, 0);

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member"), user_grant(alice, "admin")]),
    )
    .await
    .expect("save");

    let after = service::list_org_apps_with_access(&conn, org)
        .await
        .unwrap();
    assert_eq!(after[0].visibility, "members");
    assert_eq!(
        after[0].grant_count, 2,
        "the badge counts users AND teams — counting only one kind would read as \
         'restricted, nobody granted' while people could still get in"
    );
}

// ── Regression: a repeated grantee is a request we understand ───────────────

#[tokio::test]
async fn a_repeated_grantee_is_collapsed_rather_than_500ing() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let (alice, _) = seed_user(&conn).await;
    seed_member(&conn, org, alice, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;

    // Naming the same grantee twice used to trip `app_members_app_user_unique`
    // mid-transaction and surface as a 500. The UI can't produce this, but a script
    // or a retried request can — and it is a well-formed request with a redundant
    // entry, not a server fault.
    let saved = service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![
            user_grant(alice, "member"),
            team_grant(team, "member"),
            user_grant(alice, "admin"),
            team_grant(team, "admin"),
        ]),
    )
    .await
    .expect("a repeated grantee must not be a server error");

    assert_eq!(saved.grants.len(), 2, "repeats collapse to one row each");
    // Last mention wins — it matches the endpoint's full-replace semantics.
    for grant in &saved.grants {
        assert_eq!(
            grant.role, "admin",
            "the last mention of a grantee decides its role"
        );
    }
}

#[tokio::test]
async fn a_user_and_a_team_sharing_an_id_are_both_kept() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    // The two kinds live in different tables, so a shared UUID is legal. Collapsing
    // on id alone would silently drop one of them.
    let (alice, _) = seed_user(&conn).await;
    seed_member(&conn, org, alice, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;

    let saved = service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![
            user_grant(alice, "member"),
            team_grant(team, "member"),
        ]),
    )
    .await
    .expect("save");
    assert_eq!(saved.grants.len(), 2);
    assert!(saved.grants.iter().any(|g| g.kind == "user"));
    assert!(saved.grants.iter().any(|g| g.kind == "team"));
}

/// Opening an app up to the whole org KEEPS its grants, and an `admin` grant stays
/// live — so the UI must keep showing them.
///
/// This is deliberate, not a leak: `Ring::AppAdmin` has no visibility term, so an
/// admin grant on an unrestricted app is how a non-officer becomes that app's
/// administrator. But it means "Everyone in the organization" does NOT revoke
/// anything, which is the opposite of what the phrase suggests — the dialog and the
/// badge have to surface the surviving roles or the privilege outlives the change an
/// admin reads as removing it.
#[tokio::test]
async fn opening_an_app_to_the_org_keeps_grants_and_their_admin_power() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;

    // A plain member who is an app ADMIN through a team.
    let (member, member_email) = seed_user(&conn).await;
    seed_member(&conn, org, member, OrgRole::Member).await;
    let team = seed_team(&conn, org).await;
    join_team(&conn, team, member).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "admin")]),
    )
    .await
    .expect("restrict");
    let restricted_app = reload(&conn, app.id).await;
    assert_eq!(
        resolve_app_role(&conn, member, &member_email, &restricted_app)
            .await
            .unwrap(),
        Some("admin")
    );

    // Now open it to the whole org, sending the grant list back unchanged — exactly
    // what the dialog does, since its state isn't cleared by the visibility switch.
    let reopened = service::write_access(
        &conn,
        &app,
        actor,
        &SetAppAccessRequest {
            visibility: "org".into(),
            grants: vec![team_grant(team, "admin")],
        },
    )
    .await
    .expect("reopen");

    assert_eq!(reopened.visibility, "org");
    assert_eq!(
        reopened.grants.len(),
        1,
        "grants must survive the switch — the server keeps them on both branches"
    );

    // And the admin power is STILL live. If this ever starts returning None, the
    // dialog's "Roles" section on the open branch has become dead UI and should go.
    let open_app = reload(&conn, app.id).await;
    assert_eq!(
        resolve_app_role(&conn, member, &member_email, &open_app)
            .await
            .unwrap(),
        Some("admin"),
        "an admin grant outlives the switch to org-wide visibility — which is why \
         the UI must keep showing it"
    );

    // Removing it is a normal save, no need to flip back to restricted first.
    let cleared = service::write_access(
        &conn,
        &app,
        actor,
        &SetAppAccessRequest {
            visibility: "org".into(),
            grants: vec![],
        },
    )
    .await
    .expect("clear");
    assert!(cleared.grants.is_empty());
    assert_eq!(
        resolve_app_role(&conn, member, &member_email, &reload(&conn, app.id).await)
            .await
            .unwrap(),
        None
    );
}

/// The partner console's client-app list carries visibility and grant count.
///
/// `GET /partners/{id}/orgs/{org_id}/apps` returns `AppAccessSummaryDto`, not the
/// narrow `PartnerAppDto` the publish handlers use. Nothing else asserts that: the
/// handler is thin (gate → this service call), and if the route narrowed back only
/// the frontend would notice, at runtime. Pinning the serialized field set here
/// catches a rename or a removal at `cargo test` time.
#[tokio::test]
async fn the_partner_app_list_shape_carries_access_state() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let team = seed_team(&conn, org).await;
    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "admin")]),
    )
    .await
    .expect("restrict");

    let rows = service::list_org_apps_with_access(&conn, org)
        .await
        .expect("list");
    let json = serde_json::to_value(&rows).expect("serialize");
    let row = json
        .as_array()
        .and_then(|a| a.first())
        .expect("one app")
        .as_object()
        .expect("object");

    // Exactly the keys both partner panels and the org settings list consume. A
    // missing one is a silently-blank badge rather than an error, which is why this
    // asserts presence rather than trusting the type.
    for key in [
        "id",
        "name",
        "slug",
        "visibility",
        "grant_count",
        "published",
    ] {
        assert!(
            row.contains_key(key),
            "the app list DTO dropped `{key}`: {row:?}"
        );
    }
    assert_eq!(row["visibility"], "members");
    assert_eq!(row["grant_count"], 1);
}

/// A save that changes nothing must not age `apps.updated_at`.
///
/// Save is enabled whenever the dialog is seeded, so open-and-Save with no edits is
/// a normal thing to do — and `/admin/apps` sorts `updated_at DESC`, so aging the
/// row there reorders an operator's list for a change that didn't happen.
#[tokio::test]
async fn a_no_op_save_does_not_age_the_app_row() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let team = seed_team(&conn, org).await;

    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("restrict");
    let after_change = reload(&conn, app.id).await;

    // Same visibility, same grants — the no-op. The skip is decided by Postgres
    // (`WHERE visibility <> ?`), not by comparing against the row passed in, so what
    // this pins is that a save changing nothing writes nothing. Which row is handed
    // over no longer affects the outcome; the case that IS sensitive to staleness is
    // `a_stale_row_cannot_swallow_a_visibility_change`.
    service::write_access(
        &conn,
        &after_change,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("no-op save");
    assert_eq!(
        reload(&conn, app.id).await.updated_at,
        after_change.updated_at,
        "a save that changes nothing must leave updated_at alone"
    );

    // But a real visibility change still ages it.
    service::write_access(
        &conn,
        &reload(&conn, app.id).await,
        actor,
        &SetAppAccessRequest {
            visibility: "org".into(),
            grants: vec![team_grant(team, "member")],
        },
    )
    .await
    .expect("reopen");
    assert!(
        reload(&conn, app.id).await.updated_at > after_change.updated_at,
        "changing visibility IS a change to the app row"
    );
}

/// A save decided from a STALE row must not silently drop a visibility change.
///
/// This is the interleaving the `WHERE visibility <> ?` filter exists for. The
/// callers all load the app before `BEGIN`, so deciding the write in Rust against
/// that snapshot loses a race:
///
///   A and B both load the app at `org`.
///   A saves `members` — its snapshot says `org`, so it writes. Committed: `members`.
///   B saves `org`     — its snapshot ALSO says `org`, so a Rust-side comparison
///                       skips the statement, and the row stays `members` while
///                       B's grants replace A's.
///
/// B asked to open the app to everyone and got the opposite, with no error — the
/// one axis where losing the race TIGHTENS access. Passing a deliberately stale row
/// here reproduces B exactly: the fix must still write, because the committed value
/// differs even though the caller's copy doesn't.
#[tokio::test]
async fn a_stale_row_cannot_swallow_a_visibility_change() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let ws = seed_workspace(&conn, org).await;
    let app = seed_app(&conn, org, ws).await;
    let (actor, _) = seed_user(&conn).await;
    seed_member(&conn, org, actor, OrgRole::Owner).await;
    let team = seed_team(&conn, org).await;

    // B's snapshot: taken while the app is still `org`.
    let stale = app.clone();
    assert_eq!(stale.visibility, "org");

    // A restricts it. Committed value is now `members`.
    service::write_access(
        &conn,
        &app,
        actor,
        &restrict(vec![team_grant(team, "member")]),
    )
    .await
    .expect("A restricts");
    assert_eq!(reload(&conn, app.id).await.visibility, "members");

    // B saves `org` from its stale snapshot. A Rust-side `stale.visibility ==
    // "org"` comparison would skip the write here and leave the app restricted.
    let returned = service::write_access(
        &conn,
        &stale,
        actor,
        &SetAppAccessRequest {
            visibility: "org".into(),
            grants: vec![team_grant(team, "member")],
        },
    )
    .await
    .expect("B reopens");

    assert_eq!(
        reload(&conn, app.id).await.visibility,
        "org",
        "a visibility change decided from a stale snapshot must still reach the row \
         — otherwise B gets A's visibility with B's grants, the halfway merge the \
         full-replace contract rules out"
    );
    // The response agrees with the row. Note this assertion CANNOT tell a read from
    // an assertion — the row says `org` either way, so it passed before the response
    // was re-read too. Distinguishing them needs a write landing between `commit()`
    // and the re-read, which isn't reachable without a hook; what's pinned here is
    // that the two agree, not the mechanism.
    assert_eq!(returned.visibility, "org");
}

// ── ctx.user identity facts ─────────────────────────────────────────────────
//
// `orgRole` and `teams` decide nothing — they are descriptive fields a function
// uses to explain, label, or lay out. What still needs pinning is that they are
// *honest*: a role that over-reports invites an app to gate on it, and a team list
// that ignores the org filter leaks one tenant's vocabulary into another's app.

/// A team with a chosen name, so ordering can be asserted (`seed_team` names are
/// uuid-suffixed and therefore unsorted-by-construction).
async fn seed_named_team(conn: &DatabaseConnection, org_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    org_teams::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(name.to_string()),
        description: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed named team");
    id
}

/// `ctx.user.orgRole` reports the `org_members` row verbatim — including its
/// absence.
///
/// The absence is the case worth pinning: Oxy staff and partners reach a tenant's
/// app through break-glass without holding a membership row, so an app that treats
/// "no orgRole" as "not allowed" would lock out exactly the people brought in to
/// help. `appRole` is the gate; this one is allowed to be silent.
#[tokio::test]
async fn org_role_mirrors_the_membership_row_including_its_absence() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;

    for (role, expected) in [
        (OrgRole::Owner, "owner"),
        (OrgRole::Admin, "admin"),
        (OrgRole::Member, "member"),
    ] {
        let (user, _) = seed_user(&conn).await;
        seed_member(&conn, org, user, role).await;
        assert_eq!(
            resolve_org_role(&conn, user, org).await.unwrap(),
            Some(expected)
        );
    }

    let (outsider, _) = seed_user(&conn).await;
    assert_eq!(resolve_org_role(&conn, outsider, org).await.unwrap(), None);
}

/// Membership in *another* org is not this org's business.
///
/// `org_team_members` carries no org column, so the obvious "every team this user
/// is in" query is wrong in a way nothing would surface in a single-tenant test: a
/// consultant in six orgs would have all six orgs' team names handed to whichever
/// app asked first. The org filter on `org_teams` is the boundary.
#[tokio::test]
async fn org_teams_never_cross_the_tenant_boundary() {
    let conn = test_db().await;
    let (consultant, _) = seed_user(&conn).await;

    let acme = seed_org(&conn).await;
    seed_member(&conn, acme, consultant, OrgRole::Member).await;
    let acme_team = seed_named_team(&conn, acme, "Finance").await;
    join_team(&conn, acme_team, consultant).await;

    let initech = seed_org(&conn).await;
    seed_member(&conn, initech, consultant, OrgRole::Member).await;
    let initech_team = seed_named_team(&conn, initech, "Project Bluebird").await;
    join_team(&conn, initech_team, consultant).await;

    let seen = resolve_org_standing(&conn, consultant, acme)
        .await
        .unwrap()
        .1;
    let names: Vec<&str> = seen.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Finance"],
        "an app in one org must not learn another org's team names"
    );
    assert_eq!(seen[0].id, acme_team);
    assert!(
        !seen.iter().any(|t| t.id == initech_team),
        "the other tenant's team id leaked"
    );
}

/// Name-sorted, so a rendered team list doesn't reshuffle between invocations on
/// whatever order Postgres felt like returning — and sorted the way a **reader**
/// expects, not the way bytes do.
///
/// `"aardvarks"` is the case that matters: a byte-order `cmp` puts every lowercase
/// name after every capitalised one, so a team someone named in lowercase lands at
/// the bottom of the list and the whole thing reads as unsorted. Team names are
/// user-authored free text rendered in an app's UI, so this is a real rendering
/// bug, not a test detail.
#[tokio::test]
async fn org_teams_come_back_name_sorted_and_empty_means_empty() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let (user, _) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Member).await;

    assert!(
        resolve_org_standing(&conn, user, org)
            .await
            .unwrap()
            .1
            .is_empty(),
        "belonging to no team is an empty list, not an error"
    );

    for name in ["Store Managers", "Finance", "aardvarks"] {
        let team = seed_named_team(&conn, org, name).await;
        join_team(&conn, team, user).await;
    }

    let names: Vec<String> = resolve_org_standing(&conn, user, org)
        .await
        .unwrap()
        .1
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["aardvarks", "Finance", "Store Managers"]);
}

/// A stale team row must not outlive the membership that justified it.
///
/// `remove_member` deletes the `org_members` row and the workspace overrides but
/// leaves `org_team_members` behind — the comment there about re-invites "not
/// silently reactivating stale overrides" covers workspaces and misses teams. So
/// "is in a team" is genuinely not evidence of "is in the org".
///
/// Reporting the leftovers would make `ctx.user` self-contradictory: `orgRole`
/// absent while `teams` names the org's internal groups, with an app free to
/// render a team-shaped view for someone the org has ejected. Both fields now
/// derive from the same membership fact, so the contradiction is unrepresentable.
#[tokio::test]
async fn a_stale_team_row_reports_nothing_once_org_membership_is_gone() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let (user, _) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Member).await;
    let team = seed_named_team(&conn, org, "Finance").await;
    join_team(&conn, team, user).await;

    assert_eq!(
        resolve_org_standing(&conn, user, org)
            .await
            .unwrap()
            .1
            .len(),
        1,
        "sanity: a current member sees their team"
    );

    // Exactly what `remove_member` leaves behind — the membership goes, the team
    // row stays.
    org_members::Entity::delete_many()
        .filter(org_members::Column::OrgId.eq(org))
        .filter(org_members::Column::UserId.eq(user))
        .exec(&conn)
        .await
        .expect("remove membership");

    assert_eq!(
        resolve_org_role(&conn, user, org).await.unwrap(),
        None,
        "sanity: the membership really is gone"
    );
    assert!(
        resolve_org_standing(&conn, user, org)
            .await
            .unwrap()
            .1
            .is_empty(),
        "teams must follow membership — an ex-member is not on the team roster"
    );
}
