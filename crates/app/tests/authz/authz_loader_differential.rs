//! Differential tests for the authz **fact loader**, against a real PostgreSQL.
//!
//! Skips automatically when `OXY_DATABASE_URL` is unset (i.e. pure unit-test builds).
//!
//! To run locally:
//!   OXY_DATABASE_URL=postgres://... cargo nextest run -p oxy-app --test authz -E 'test(authz_loader_differential)'
//!
//! ## Why this exists
//!
//! The unit differential tests (`server::authz::differential`) prove the MODEL
//! matches the shipped guards — but they hand-build the [`PrincipalFacts`], so they
//! test an *assumption* about what the loader returns, not the loader. A loader bug
//! (wrong column, missed partner condition, an over-broad set) would sail straight
//! through them and only surface as a live wrong answer.
//!
//! So: seed real rows, run the REAL `load_principal_facts` against them, and assert
//! the facts are what the policy tests assume. Together the two layers are an
//! end-to-end proof with no production traffic required — which matters, because a
//! shadow window can't validate anything while there are ~no users to generate the
//! traffic it samples.
//!
//! Each test seeds its own uniquely-keyed rows, so they're independent and re-runnable.

use entity::{
    app_admins, app_members, app_team_grants, apps, org_members, org_team_members, org_teams,
    organizations, partner_capabilities, partner_grants, partner_orgs, partner_role_bindings,
    users, workspace_members, workspaces,
};
use oxy::database::client::establish_connection;
use oxy_app::server::authz::loader::load_principal_facts;
use oxy_authz::{Action, PlatformRole, Resource, allows};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

async fn seed_user(conn: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("authz-diff-{id}@example.com");
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Authz Differential".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
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
        name: ActiveValue::Set(format!("Authz Diff Org {id}")),
        slug: ActiveValue::Set(format!("authz-diff-{id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(conn)
    .await
    .expect("seed org");
    id
}

async fn seed_membership(
    conn: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    role: org_members::OrgRole,
) -> Uuid {
    let id = Uuid::new_v4();
    org_members::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(conn)
    .await
    .expect("seed membership");
    id
}

async fn seed_app_admin(conn: &DatabaseConnection, email: &str) {
    seed_platform_grant(conn, email, PlatformRole::GlobalAdmin, None).await;
}

/// Seed a platform grant. `scope` of `None` is unbounded; `Some(orgs)` writes
/// `scope_all = false` plus the child rows.
async fn seed_platform_grant(
    conn: &DatabaseConnection,
    email: &str,
    role: PlatformRole,
    scope: Option<&[Uuid]>,
) {
    let id = Uuid::new_v4();
    app_admins::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email.to_string()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set(role.as_str().to_string()),
        scope_all: ActiveValue::Set(scope.is_none()),
        updated_at: ActiveValue::NotSet,
    }
    .insert(conn)
    .await
    .expect("seed app_admin");

    for org_id in scope.unwrap_or_default() {
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(id),
            org_id: ActiveValue::Set(*org_id),
            created_at: ActiveValue::NotSet,
            created_by: ActiveValue::Set(None),
        }
        .insert(conn)
        .await
        .expect("seed app_admin scope org");
    }
}

/// The **positive** Global-Admin grant — the direction `app_admins` membership actually
/// *elevates* a principal. `loader_derives_org_role_sets_from_real_rows` only asserts the
/// negative (a user not in `app_admins` is not global admin), which a loader that never
/// set the flag would also pass. Seed the row and prove the grant flows through the
/// `authz::globals::is_app_admin_email` read the loader depends on.
#[tokio::test]
async fn loader_grants_global_admin_to_an_app_admins_member() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    seed_app_admin(&conn, &email).await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader must resolve facts against a live seeded database");
    assert!(
        facts.is_global_admin(),
        "a user seeded into app_admins must load as Global Admin — the elevation path that \
         only the negative case was guarding"
    );
}

/// **The capability split, against real rows.** The unit tests hand-build
/// `PlatformStanding`, so they test an *assumption* about the loader. This is the one
/// that tests the loader: seed an `app_operator` grant and prove the role survives the
/// round trip, so the app rings open and org deletion does not.
///
/// A loader that ignored `role` and kept minting Global Admins would pass every unit
/// test in `oxy-authz` and fail here.
#[tokio::test]
async fn loader_reads_the_role_so_an_app_operator_cannot_delete_an_org() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let org_id = seed_org(&conn).await;
    seed_platform_grant(&conn, &email, PlatformRole::AppOperator, None).await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader must resolve facts against a live seeded database");

    assert!(
        allows(
            &facts,
            Action::AppAdmin,
            &Resource::app(Uuid::new_v4(), org_id)
        ),
        "an app operator must reach the app rings — the role has to be useful"
    );
    assert!(
        !allows(&facts, Action::OrgOwnerManage, &Resource::org(org_id)),
        "an app operator must NOT reach org deletion; if this fails the loader is \
         discarding `role` and every grant is a Global Admin again"
    );
    assert!(
        !allows(&facts, Action::MemberInvite, &Resource::org(org_id)),
        "nor org member management"
    );
}

/// Scope, against real rows: a bounded grant reaches its own orgs and no others.
///
/// Pins the child-table read AND the fail-closed direction — `scope_all = false` with
/// rows for org A must not resolve to "everything" for org B.
#[tokio::test]
async fn loader_reads_scope_rows_and_fences_tenant_reach() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let in_scope = seed_org(&conn).await;
    let out_of_scope = seed_org(&conn).await;
    seed_platform_grant(&conn, &email, PlatformRole::AppOperator, Some(&[in_scope])).await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader must resolve facts against a live seeded database");

    assert!(
        allows(
            &facts,
            Action::AppAdmin,
            &Resource::app(Uuid::new_v4(), in_scope)
        ),
        "the granted org must be reachable"
    );
    assert!(
        !allows(
            &facts,
            Action::AppAdmin,
            &Resource::app(Uuid::new_v4(), out_of_scope)
        ),
        "an org outside the grant's scope must not be reachable — scope is loaded from \
         app_admin_scope_orgs, and dropping that read reads as unbounded"
    );
}

/// The org-role sets are the backbone of every tenant ring: `owned ⊆ admin ⊆ member`.
/// If this derivation is wrong, every ring is wrong.
#[tokio::test]
async fn loader_derives_org_role_sets_from_real_rows() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    for (role, expect_owned, expect_admin) in [
        (org_members::OrgRole::Owner, true, true),
        (org_members::OrgRole::Admin, false, true),
        (org_members::OrgRole::Member, false, false),
    ] {
        let (user_id, email) = seed_user(&conn).await;
        let org_id = seed_org(&conn).await;
        seed_membership(&conn, org_id, user_id, role.clone()).await;

        let facts = load_principal_facts(&conn, user_id, &email).await.expect(
            "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
        );

        assert_eq!(
            facts.owned_orgs.contains(&org_id),
            expect_owned,
            "owned_orgs for {role:?}"
        );
        assert_eq!(
            facts.admin_orgs.contains(&org_id),
            expect_admin,
            "admin_orgs for {role:?}"
        );
        // Every role is a member — the containment the rings rely on.
        assert!(
            facts.member_orgs.contains(&org_id),
            "member_orgs for {role:?}"
        );
    }
}

/// A user must never pick up standing in an org they don't belong to. This is the
/// tenant-isolation property the whole model rests on.
#[tokio::test]
async fn loader_gives_no_standing_in_a_foreign_org() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let mine = seed_org(&conn).await;
    let theirs = seed_org(&conn).await;
    seed_membership(&conn, mine, user_id, org_members::OrgRole::Owner).await;

    let facts = load_principal_facts(&conn, user_id, &email).await.expect(
        "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
    );

    assert!(facts.member_orgs.contains(&mine));
    for set in [&facts.owned_orgs, &facts.admin_orgs, &facts.member_orgs] {
        assert!(!set.contains(&theirs), "leaked standing into a foreign org");
    }
    assert!(
        !facts
            .partners
            .iter()
            .any(|p| p.client_orgs.contains(&theirs)),
        "leaked partner standing into a foreign org"
    );
}

/// A plain member of a NON-partner org must get no partner sets — the short-circuit
/// the loader relies on for the common case.
#[tokio::test]
async fn loader_gives_no_partner_sets_to_an_ordinary_member() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let org_id = seed_org(&conn).await;
    seed_membership(&conn, org_id, user_id, org_members::OrgRole::Member).await;

    let facts = load_principal_facts(&conn, user_id, &email).await.expect(
        "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
    );
    assert!(facts.partners.is_empty());
    assert!(!facts.is_global_admin(), "not seeded into app_admins");
}

/// The partner path end-to-end, and the distinction that a mis-model would have leaked:
/// `managed_orgs` is the coarse "operates this client"; `develop_apps_orgs` is the
/// narrower data-plane capability. A managing partner WITHOUT develop_apps must get the
/// former and NOT the latter — otherwise it reads another tenant's app data.
#[tokio::test]
async fn loader_separates_managed_orgs_from_the_develop_apps_data_plane() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    for develop_apps in [false, true] {
        let (user_id, email) = seed_user(&conn).await;
        let partner_org = seed_org(&conn).await;
        let client_org = seed_org(&conn).await;

        // The operator is an ordinary member of the PARTNER org...
        let membership_id =
            seed_membership(&conn, partner_org, user_id, org_members::OrgRole::Member).await;

        // ...that org holds an ACTIVE partner grant...
        partner_grants::ActiveModel {
            org_id: ActiveValue::Set(partner_org),
            status: ActiveValue::Set("active".into()),
            created_by: ActiveValue::Set(None),
            created_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .expect("seed grant");

        // ...with a ceiling that may or may not include develop_apps...
        partner_capabilities::ActiveModel {
            org_id: ActiveValue::Set(partner_org),
            manage_members: ActiveValue::Set(false),
            manage_apps: ActiveValue::Set(true),
            develop_apps: ActiveValue::Set(develop_apps),
            view_audit: ActiveValue::Set(false),
            manage_billing: ActiveValue::Set(false),
            manage_secrets: ActiveValue::Set(false),
            create_orgs: ActiveValue::Set(false),
            manage_org_settings: ActiveValue::Set(false),
            updated_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .expect("seed capabilities");

        // ...the member holds partner access...
        partner_role_bindings::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_member_id: ActiveValue::Set(membership_id),
            created_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .expect("seed binding");

        // ...and the partner manages the client.
        partner_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            partner_org_id: ActiveValue::Set(partner_org),
            managed_org_id: ActiveValue::Set(client_org),
            created_by: ActiveValue::Set(None),
            created_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .expect("seed partner_orgs");

        let facts = load_principal_facts(&conn, user_id, &email).await.expect(
            "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
        );

        let standing = facts
            .partners
            .iter()
            .find(|p| p.partner_id == partner_org)
            .expect("an operator of an active grant has a standing in that partner");
        assert!(
            standing.client_orgs.contains(&client_org),
            "an operator of an active grant manages the client (develop_apps={develop_apps})"
        );
        assert!(
            standing.caps.contains(&oxy_authz::Cap::ManageApps),
            "the seeded ceiling grants manage_apps"
        );
        assert_eq!(
            standing.caps.contains(&oxy_authz::Cap::DevelopApps),
            develop_apps,
            "the data plane must follow the develop_apps ceiling ONLY (develop_apps={develop_apps})"
        );
        // The client's own org is not one the operator is a member of.
        assert!(!facts.member_orgs.contains(&client_org));
    }
}

/// A member of a partner org WITHOUT a partner-access binding is just an employee of
/// that org — they manage nothing.
#[tokio::test]
async fn loader_gives_no_managed_orgs_without_a_partner_access_binding() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let partner_org = seed_org(&conn).await;
    let client_org = seed_org(&conn).await;
    seed_membership(&conn, partner_org, user_id, org_members::OrgRole::Member).await;

    partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner_org),
        status: ActiveValue::Set("active".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed grant");
    partner_orgs::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        partner_org_id: ActiveValue::Set(partner_org),
        managed_org_id: ActiveValue::Set(client_org),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed partner_orgs");
    // NOTE: no partner_role_bindings row.

    let facts = load_principal_facts(&conn, user_id, &email).await.expect(
        "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
    );
    assert!(
        facts.partners.is_empty(),
        "a member without partner access manages nothing"
    );
}

/// A SUSPENDED grant confers nothing, however complete the rest of the setup is.
#[tokio::test]
async fn loader_ignores_a_suspended_partner_grant() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    let (user_id, email) = seed_user(&conn).await;
    let partner_org = seed_org(&conn).await;
    let client_org = seed_org(&conn).await;
    let membership_id =
        seed_membership(&conn, partner_org, user_id, org_members::OrgRole::Member).await;

    partner_grants::ActiveModel {
        org_id: ActiveValue::Set(partner_org),
        status: ActiveValue::Set("suspended".into()),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed grant");
    partner_role_bindings::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_member_id: ActiveValue::Set(membership_id),
        created_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed binding");
    partner_orgs::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        partner_org_id: ActiveValue::Set(partner_org),
        managed_org_id: ActiveValue::Set(client_org),
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
    }
    .insert(&conn)
    .await
    .expect("seed partner_orgs");

    let facts = load_principal_facts(&conn, user_id, &email).await.expect(
        "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
    );
    assert!(
        facts.partners.is_empty(),
        "a suspended grant must confer nothing"
    );
}

/// `ws_admin_override` must carry ONLY the exceptional elevations — a Viewer/Member
/// override must not appear, or a plain member would gain workspace-admin.
#[tokio::test]
async fn loader_loads_only_elevating_workspace_overrides() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL unset");
        return;
    }
    let conn = establish_connection().await.expect("db connect");

    for (role, expect) in [
        (workspace_members::WorkspaceRole::Owner, true),
        (workspace_members::WorkspaceRole::Admin, true),
        (workspace_members::WorkspaceRole::Member, false),
        (workspace_members::WorkspaceRole::Viewer, false),
    ] {
        let (user_id, email) = seed_user(&conn).await;
        let org_id = seed_org(&conn).await;
        seed_membership(&conn, org_id, user_id, org_members::OrgRole::Member).await;

        let workspace_id = Uuid::new_v4();
        workspaces::ActiveModel {
            id: ActiveValue::Set(workspace_id),
            name: ActiveValue::Set(format!("authz-diff-ws-{workspace_id}")),
            org_id: ActiveValue::Set(Some(org_id)),
            created_by: ActiveValue::Set(Some(user_id)),
            status: ActiveValue::Set(workspaces::WorkspaceStatus::Ready),
            ..Default::default()
        }
        .insert(&conn)
        .await
        .expect("seed workspace");

        workspace_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            workspace_id: ActiveValue::Set(workspace_id),
            user_id: ActiveValue::Set(user_id),
            role: ActiveValue::Set(role.clone()),
            created_at: ActiveValue::NotSet,
            updated_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .expect("seed workspace member");

        let facts = load_principal_facts(&conn, user_id, &email).await.expect(
            "the loader must resolve facts against a live seeded database; None means a lookup \
         errored, which every caller reads as unknown-not-absent",
        );
        assert_eq!(
            facts.ws_admin_override.contains(&workspace_id),
            expect,
            "ws_admin_override must carry only Admin/Owner elevations (role={role:?})"
        );
    }
}

// ── Per-app membership facts ──────────────────────────────────────────────────

/// Seed a custom app. `project_id` is a workspace id but carries no FK, so a
/// bare uuid is enough here — we're testing membership, not the workspace join.
async fn seed_app(conn: &DatabaseConnection, org_id: Uuid, restricted: bool) -> Uuid {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(format!("authz-diff-app-{id}")),
        name: ActiveValue::Set("Authz Differential App".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(Uuid::new_v4()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("authz/diff".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set(if restricted { "members" } else { "org" }.to_string()),
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
        role: ActiveValue::Set(role.to_string()),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed app member");
}

/// The facts half of the per-app work: `app_members` rows must reach
/// `PrincipalFacts`, with `admin ⊆ all` the way the org sets nest. If this
/// regresses, `Ring::AppAccess` denies a restricted app to its own members and
/// `Ring::AppAdmin` denies the app's admin — both silent, both fail-closed, and
/// neither visible to the hand-built unit differential.
#[tokio::test]
async fn loader_derives_app_membership_sets_from_real_rows() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let conn = establish_connection().await.expect("connect");
    let org_id = seed_org(&conn).await;

    // A plain app member: in `app_memberships`, NOT in the admin set.
    let (member_id, member_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, member_id, org_members::OrgRole::Member).await;
    let app_id = seed_app(&conn, org_id, true).await;
    seed_app_member(&conn, app_id, member_id, "member").await;

    let facts = load_principal_facts(&conn, member_id, &member_email)
        .await
        .expect("loader must resolve facts against a live seeded database");
    assert!(
        facts.app_memberships.contains(&app_id),
        "an app_members row must load into app_memberships"
    );
    assert!(
        !facts.app_admin_memberships.contains(&app_id),
        "role='member' must NOT load as an app admin — that would hand the app's \
         privileged surface to every member"
    );

    // An app admin: in BOTH sets (admin implies membership, like owner→member).
    let (admin_id, admin_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, admin_id, org_members::OrgRole::Member).await;
    seed_app_member(&conn, app_id, admin_id, "admin").await;
    let facts = load_principal_facts(&conn, admin_id, &admin_email)
        .await
        .expect("loader facts");
    assert!(facts.app_memberships.contains(&app_id));
    assert!(
        facts.app_admin_memberships.contains(&app_id),
        "role='admin' must load into app_admin_memberships"
    );

    // A user with no app_members row holds neither, even as an org member.
    let (outsider_id, outsider_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, outsider_id, org_members::OrgRole::Member).await;
    let facts = load_principal_facts(&conn, outsider_id, &outsider_email)
        .await
        .expect("loader facts");
    assert!(
        !facts.app_memberships.contains(&app_id) && !facts.app_admin_memberships.contains(&app_id),
        "plain org membership must not synthesize per-app membership — that would \
         make restricted apps meaningless"
    );
}

/// Membership must not bleed across apps: a row for app A says nothing about B.
#[tokio::test]
async fn loader_scopes_app_membership_to_its_own_app() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let conn = establish_connection().await.expect("connect");
    let org_id = seed_org(&conn).await;
    let (user_id, email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, user_id, org_members::OrgRole::Member).await;

    let mine = seed_app(&conn, org_id, true).await;
    let theirs = seed_app(&conn, org_id, true).await;
    seed_app_member(&conn, mine, user_id, "admin").await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader facts");
    assert!(facts.app_admin_memberships.contains(&mine));
    assert!(
        !facts.app_memberships.contains(&theirs) && !facts.app_admin_memberships.contains(&theirs),
        "admin of one app must not confer standing on another app in the same org"
    );
}

// ── Team-reached grants ─────────────────────────────────────────────────────
//
// Teams are the control surface an org admin actually uses, and they reach
// `PrincipalFacts` through a SECOND path (`org_team_members` → `app_team_grants`)
// that the hand-built unit differential cannot see. If this union regresses, every
// team-granted user silently loses the app while direct `app_members` rows keep
// working — the failure would look like "teams don't do anything."

async fn seed_team(conn: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    org_teams::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("authz-diff-team-{id}")),
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

async fn seed_team_member(conn: &DatabaseConnection, team_id: Uuid, user_id: Uuid) {
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
}

async fn seed_team_grant(conn: &DatabaseConnection, app_id: Uuid, team_id: Uuid, role: &str) {
    app_team_grants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        team_id: ActiveValue::Set(team_id),
        role: ActiveValue::Set(role.to_string()),
        created_at: ActiveValue::NotSet,
        created_by: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed team grant");
}

/// A grant reached through a team must land in the same fact vectors a direct
/// `app_members` row does — that union is why no `oxy-authz` ring mentions teams.
#[tokio::test]
async fn loader_unions_team_reached_grants_into_app_membership_sets() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let conn = establish_connection().await.expect("connect");
    let org_id = seed_org(&conn).await;
    let app_id = seed_app(&conn, org_id, true).await;

    // Plain member grant through a team → app_memberships only.
    let (member_id, member_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, member_id, org_members::OrgRole::Member).await;
    let team = seed_team(&conn, org_id).await;
    seed_team_member(&conn, team, member_id).await;
    seed_team_grant(&conn, app_id, team, "member").await;

    let facts = load_principal_facts(&conn, member_id, &member_email)
        .await
        .expect("loader facts");
    assert!(
        facts.app_memberships.contains(&app_id),
        "a team grant must reach app_memberships, or teams grant nothing at all"
    );
    assert!(
        !facts.app_admin_memberships.contains(&app_id),
        "a role='member' team grant must not confer the app's privileged surface"
    );

    // Admin grant through a team → BOTH sets, exactly like a direct admin row.
    let (admin_id, admin_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, admin_id, org_members::OrgRole::Member).await;
    let admin_team = seed_team(&conn, org_id).await;
    seed_team_member(&conn, admin_team, admin_id).await;
    seed_team_grant(&conn, app_id, admin_team, "admin").await;

    let facts = load_principal_facts(&conn, admin_id, &admin_email)
        .await
        .expect("loader facts");
    assert!(
        facts.app_memberships.contains(&app_id) && facts.app_admin_memberships.contains(&app_id),
        "an admin team grant must nest admin ⊆ all, like a direct admin row"
    );
}

/// Being in a team that holds NO grant confers nothing. The team is the audience,
/// not the authority — this is the check that stops "add them to Finance" from
/// quietly meaning "give them every app".
#[tokio::test]
async fn loader_ignores_team_membership_without_a_grant() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let conn = establish_connection().await.expect("connect");
    let org_id = seed_org(&conn).await;
    let granted = seed_app(&conn, org_id, true).await;
    let ungranted = seed_app(&conn, org_id, true).await;

    let (user_id, email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, user_id, org_members::OrgRole::Member).await;
    let team = seed_team(&conn, org_id).await;
    seed_team_member(&conn, team, user_id).await;
    seed_team_grant(&conn, granted, team, "member").await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader facts");
    assert!(facts.app_memberships.contains(&granted));
    assert!(
        !facts.app_memberships.contains(&ungranted),
        "a team grant on one app must not reach another app in the same org"
    );

    // And someone in NO team holds nothing, even with the grant in place.
    let (outsider_id, outsider_email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, outsider_id, org_members::OrgRole::Member).await;
    let facts = load_principal_facts(&conn, outsider_id, &outsider_email)
        .await
        .expect("loader facts");
    assert!(
        !facts.app_memberships.contains(&granted),
        "a grant to a team the user is not in must not reach them"
    );
}

/// The strongest grant wins when both paths name the same app. A direct `member`
/// row plus an `admin` team grant must read as admin — the same way two org
/// memberships resolve — or the two sources would fight and the answer would depend
/// on load order.
#[tokio::test]
async fn loader_takes_the_strongest_grant_when_both_paths_apply() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let conn = establish_connection().await.expect("connect");
    let org_id = seed_org(&conn).await;
    let app_id = seed_app(&conn, org_id, true).await;
    let (user_id, email) = seed_user(&conn).await;
    seed_membership(&conn, org_id, user_id, org_members::OrgRole::Member).await;

    seed_app_member(&conn, app_id, user_id, "member").await;
    let team = seed_team(&conn, org_id).await;
    seed_team_member(&conn, team, user_id).await;
    seed_team_grant(&conn, app_id, team, "admin").await;

    let facts = load_principal_facts(&conn, user_id, &email)
        .await
        .expect("loader facts");
    assert!(
        facts.app_admin_memberships.contains(&app_id),
        "an admin team grant must win over a weaker direct row"
    );
    // And the app must appear exactly once — the union de-duplicates, or a ring
    // reading `.len()` would see phantom grants.
    assert_eq!(
        facts
            .app_memberships
            .iter()
            .filter(|a| **a == app_id)
            .count(),
        1,
        "the two grant paths must union, not append"
    );
}
