//! The custom-app gate's frontline term, against a real database.
//!
//! This is the test two rounds of review asked for and two commit messages
//! promised. Both frontline defects on this branch lived in exactly this
//! decision and neither was reachable from a unit test:
//!
//!   * the first version admitted any active worker and trusted `Ring::AppAccess`
//!     to narrow it — but the ring is handed a WORKSPACE id where it expects an
//!     app id, so it never narrowed anything, and the term was live only while
//!     the fact loader was erroring;
//!   * the second required a grant but was then zeroed by `enforce`'s
//!     conjunction, so a granted worker got 403 on every healthy request.
//!
//! What makes those catchable here and not above is the join. The decision is
//! "active standing AND a grant on an app in THIS workspace", and every way of
//! getting it wrong — dropping the standing check, dropping the grant check,
//! scoping the grant to the org instead of the workspace — still passes a happy
//! path with one worker, one app and one grant. So each case below removes
//! exactly one of those and asserts the answer flips.

use entity::{app_members, apps, org_frontline_members, org_members, organizations, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_app::server::api::custom_apps_functions::host::org_directory;
use oxy_app::server::api::custom_apps_gates::frontline_worker_with_app_grant;

/// One org, two workspaces, one app in each.
struct Fx {
    org: Uuid,
    workspace_a: Uuid,
    workspace_b: Uuid,
    app_a: Uuid,
    app_b: Uuid,
}

async fn user(db: &DatabaseConnection, name: &str, email: Option<String>) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        // `None` for a worker: a frontline user is deliberately mailbox-less,
        // and that NULL is what keeps them out of every email-keyed path.
        email: ActiveValue::Set(email),
        name: ActiveValue::Set(name.into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn seed(db: &DatabaseConnection) -> Fx {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Poke".into()),
        slug: ActiveValue::Set(format!("poke-{}", &org.simple().to_string()[..8])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    let mut ws = Vec::new();
    for label in ["a", "b"] {
        let id = Uuid::new_v4();
        entity::workspaces::ActiveModel {
            id: ActiveValue::Set(id),
            org_id: ActiveValue::Set(Some(org)),
            name: ActiveValue::Set(format!("workspace {label}")),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("seed workspace");
        ws.push(id);
    }

    let mut app = Vec::new();
    for (i, project) in ws.iter().enumerate() {
        let id = Uuid::new_v4();
        apps::ActiveModel {
            id: ActiveValue::Set(id),
            org_id: ActiveValue::Set(org),
            project_id: ActiveValue::Set(*project),
            slug: ActiveValue::Set(format!("app-{i}")),
            name: ActiveValue::Set(format!("App {i}")),
            // Every NOT NULL column the table actually has. Spelled out rather
            // than defaulted, because a fixture that cannot insert is a test
            // that proves nothing about the logic it was written for.
            branch: ActiveValue::Set("main".into()),
            source_repo: ActiveValue::Set("git@example.com:poke/app.git".into()),
            status: ActiveValue::Set("active".into()),
            source_type: ActiveValue::Set("git".into()),
            source_config: ActiveValue::Set(serde_json::json!({})),
            visibility: ActiveValue::Set("org".into()),
            // Published: `user_can_access_app` refuses every customer a draft,
            // and the third test below drives that gate too.
            published_at: ActiveValue::Set(Some(chrono::Utc::now().fixed_offset())),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("seed app");
        app.push(id);
    }

    Fx {
        org,
        workspace_a: ws[0],
        workspace_b: ws[1],
        app_a: app[0],
        app_b: app[1],
    }
}

async fn enrol(db: &DatabaseConnection, org: Uuid, user_id: Uuid, status: &str) {
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(user_id),
        status: ActiveValue::Set(status.into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("enrol worker");
}

async fn grant(db: &DatabaseConnection, app_id: Uuid, user_id: Uuid) {
    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set("member".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("grant app membership");
}

/// The decision, one removed ingredient at a time.
#[tokio::test]
async fn a_worker_reaches_only_the_workspace_they_hold_a_grant_in() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    // ── The happy path: active standing + a grant on an app in workspace A.
    let granted = user(&db, "Maria S.", None).await;
    enrol(&db, fx.org, granted, "active").await;
    grant(&db, fx.app_a, granted).await;

    assert!(
        frontline_worker_with_app_grant(&db, fx.org, granted, fx.workspace_a).await,
        "an enrolled worker holding a grant on this workspace's app must reach it — \
         this is the case that returned 403 on every healthy request"
    );

    // ── Same worker, OTHER workspace. The join is the only thing that denies
    // this, and without it the term reads "has a grant anywhere in the org" —
    // exactly the org-wide reach a worker must not have.
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, granted, fx.workspace_b).await,
        "a grant on one workspace's app must not open another's"
    );

    // ── Standing, no grant. This is the fail-open the first version shipped:
    // it admitted every active worker and trusted a ring that could not see the
    // grant to narrow it.
    let ungranted = user(&db, "Sam T.", None).await;
    enrol(&db, fx.org, ungranted, "active").await;
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, ungranted, fx.workspace_a).await,
        "standing alone must not be access"
    );

    // ── Grant, no standing. A plain user with an app grant is not a frontline
    // worker, and this term must not be the thing that lets them in — their
    // access comes from org membership through the ring, not from here.
    let outsider = user(&db, "Nia O.", Some("nia@example.com".into())).await;
    grant(&db, fx.app_a, outsider).await;
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, outsider, fx.workspace_a).await,
        "a grant without frontline standing is not this term's business"
    );

    // ── Suspended standing. `is_active_frontline` reads `status == "active"`,
    // and a worker who has left must lose access without their rows being
    // deleted — the submissions they filed still have to attribute to somebody.
    let suspended = user(&db, "Gone A.", None).await;
    enrol(&db, fx.org, suspended, "suspended").await;
    grant(&db, fx.app_a, suspended).await;
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, suspended, fx.workspace_a).await,
        "suspended standing must deny even with a live grant"
    );

    // ── The fixture must not be proving this by accident: no worker here holds
    // an org_members row, which is what would make every assertion above pass
    // for the wrong reason on a term that checked membership instead.
    for who in [granted, ungranted, suspended] {
        assert!(
            org_members::Entity::find()
                // By USER id, not `find_by_id`. `org_members` has a surrogate
                // primary key, so `find_by_id(user)` searches the membership-row
                // id space with a `users.id` — two spaces that never collide, so
                // it returned None unconditionally. The assertion written to
                // stop this test passing for the wrong reason was itself passing
                // for the wrong reason, and would have held with all three
                // workers holding memberships.
                .filter(org_members::Column::UserId.eq(who))
                .one(&db)
                .await
                .expect("query")
                .is_none(),
            "a seeded worker has an org_members row, so this test proves nothing"
        );
    }
}

/// The directory names people who can reach THIS app, and nobody else.
///
/// One rule covering two kinds of principal, so the cases that matter are where
/// the halves disagree: a worker granted here is in, the SAME worker is out of
/// the app next door, and suspending them takes them out of both. A plain union
/// of the two tables passes the first and fails the other two, which is the
/// version this replaces.
#[tokio::test]
async fn the_directory_names_whoever_can_reach_this_app() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    let office = user(&db, "Nia O.", Some("nia@example.com".into())).await;
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(fx.org),
        user_id: ActiveValue::Set(office),
        role: ActiveValue::Set(entity::org_members::OrgRole::Member),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed member");

    let crew = user(&db, "Maria S.", None).await;
    enrol(&db, fx.org, crew, "active").await;
    grant(&db, fx.app_a, crew).await;

    let ungranted = user(&db, "Sam T.", None).await;
    enrol(&db, fx.org, ungranted, "active").await;

    async fn names(db: &DatabaseConnection, org: Uuid, app: Uuid) -> Vec<String> {
        org_directory(db, org, app).await.expect("directory")["people"]
            .as_array()
            .expect("people array")
            .iter()
            .map(|p| p["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    let here = names(&db, fx.org, fx.app_a).await;
    assert!(
        here.contains(&"Nia O.".to_string()),
        "an org member must be in it"
    );
    assert!(
        here.contains(&"Maria S.".to_string()),
        "a worker granted on THIS app must be nameable — an app that cannot name most \
         of its users grows its own people model, which is what this exists to stop"
    );
    assert!(
        !here.contains(&"Sam T.".to_string()),
        "a worker with no grant cannot reach this app, so it must not name them"
    );

    // The app next door: same org, same worker, no grant. This is the case a
    // plain union gets wrong.
    let next_door = names(&db, fx.org, fx.app_b).await;
    assert!(
        next_door.contains(&"Nia O.".to_string()),
        "org membership is not per-app"
    );
    assert!(
        !next_door.contains(&"Maria S.".to_string()),
        "a grant is per-app, so the directory's frontline half must be too"
    );

    // Suspension removes them here as well as from the login and the kiosk —
    // one column, and every surface that reads it agrees.
    oxy_auth::frontline::set_worker_standing(&db, fx.org, crew, false)
        .await
        .expect("suspend");
    assert!(
        !names(&db, fx.org, fx.app_a)
            .await
            .contains(&"Maria S.".to_string()),
        "a suspended worker must leave the directory too"
    );
}

/// The model, fact-loaded from the same rows, agrees with the gate's oracle —
/// and with the app-keyed gate every function invoke goes through.
///
/// This is the differential the data-plane exemption never had. `enforce` is
/// `existing_allow && allows(..)`, so a ring that cannot see a worker's grant
/// does not merely fail to narrow — it DENIES, on every healthy request, and the
/// only way the gate could admit the crew was to skip the model for them.
/// `Ring::WorkspaceData` reads the fact the loader now derives, and this holds
/// the two sides to each other for every shape the first test enumerates.
///
/// The last assertion in each case is `user_can_access_app`, the gate behind the
/// app shell and every function invoke. It had NO frontline term: the data plane
/// admitted a worker and the model admitted a worker, and the one gate the
/// kiosk actually hits answered 403 to every function in the app they were
/// enrolled to use.
#[tokio::test]
async fn the_model_and_both_gates_agree_about_a_worker() {
    use oxy_app::server::api::custom_apps_auth::user_can_access_app;
    use oxy_app::server::authz::{Action, Resource, allows, enforce, loader};

    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;

    let granted = user(&db, "Maria S.", None).await;
    enrol(&db, fx.org, granted, "active").await;
    grant(&db, fx.app_a, granted).await;
    let ungranted = user(&db, "Sam T.", None).await;
    enrol(&db, fx.org, ungranted, "active").await;
    let suspended = user(&db, "Gone A.", None).await;
    enrol(&db, fx.org, suspended, "suspended").await;
    grant(&db, fx.app_a, suspended).await;

    let cases = [
        (
            "granted, own workspace",
            granted,
            fx.workspace_a,
            fx.app_a,
            true,
        ),
        (
            "granted, workspace next door",
            granted,
            fx.workspace_b,
            fx.app_b,
            false,
        ),
        (
            "standing, no grant",
            ungranted,
            fx.workspace_a,
            fx.app_a,
            false,
        ),
        (
            "grant, suspended",
            suspended,
            fx.workspace_a,
            fx.app_a,
            false,
        ),
    ];
    for (label, who, ws, app, want) in cases {
        let oracle = frontline_worker_with_app_grant(&db, fx.org, who, ws).await;
        assert_eq!(oracle, want, "{label}: the gate's own term");

        // "" is what the gate passes for a worker: they have no address.
        let facts = loader::load_principal_facts_scoped(
            &db,
            &oxy_app::server::authz::Caller::without_credential(who, ""),
            false,
        )
        .await
        .expect("facts must load for a worker — unknown facts would skip the ring");
        let resource = Resource::workspace(ws, fx.org);
        assert_eq!(
            allows(&facts, Action::WorkspaceDataAccess, &resource),
            want,
            "{label}: the model, from the loader's facts"
        );
        // The conjunction the gate actually ships. Before this ring it was
        // `true && false` for the granted case, and the gate skipped it.
        assert_eq!(
            enforce(
                "gate.custom_app",
                &facts,
                Action::WorkspaceDataAccess,
                &resource,
                oracle
            ),
            want,
            "{label}: enforce"
        );

        let app_row = apps::Entity::find_by_id(app)
            .one(&db)
            .await
            .expect("query")
            .expect("seeded app");
        assert_eq!(
            user_can_access_app(
                &db,
                &oxy_app::server::authz::Caller::without_credential(who, ""),
                &app_row
            )
            .await
            .expect("access check"),
            want,
            "{label}: user_can_access_app — the shell and every function invoke"
        );
    }

    // A grant on a DRAFT. `user_can_access_app` refuses every customer an
    // unpublished app; the two frontline joins must draw the same line, or a
    // worker's grant opens the workspace's data plane while the app's own
    // shell still refuses to load — access to the data behind a door that is
    // shut. Unpublish the app the granted worker holds and every answer flips.
    apps::ActiveModel {
        id: ActiveValue::Set(fx.app_a),
        published_at: ActiveValue::Set(None),
        ..Default::default()
    }
    .update(&db)
    .await
    .expect("unpublish");
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, granted, fx.workspace_a).await,
        "a grant on a draft app is not reach into its workspace (gate)"
    );
    let facts = loader::load_principal_facts_scoped(
        &db,
        &oxy_app::server::authz::Caller::without_credential(granted, ""),
        false,
    )
    .await
    .expect("facts");
    assert!(
        !allows(
            &facts,
            Action::WorkspaceDataAccess,
            &Resource::workspace(fx.workspace_a, fx.org)
        ),
        "a grant on a draft app is not reach into its workspace (model)"
    );
    // The app-keyed gate hits the same branch its public counterpart does:
    // `user_can_access_app` refuses a draft to every customer, workers included.
    // (`custom_apps_auth` caches a verdict per (user, app) for 60s, so the
    // cache is cleared first — this is a fresh database but one process.)
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    let draft = apps::Entity::find_by_id(fx.app_a)
        .one(&db)
        .await
        .expect("query")
        .expect("app");
    assert!(
        !user_can_access_app(
            &db,
            &oxy_app::server::authz::Caller::without_credential(granted, ""),
            &draft
        )
        .await
        .expect("access check"),
        "a grant on a draft app does not open the app itself"
    );
}

/// The one row the whole frontline model keys on has a writer a worker can pass.
///
/// `write_access` — the app's access settings — validated every grantee as an org
/// member, which a worker never is. So the grant `Ring::AppAccess` and
/// `user_can_access_app` both require could be created only by hand. Now an
/// ACTIVE worker of the org is a valid grantee; an outsider and a suspended
/// worker are still refused, because a grant narrows within an org and a worker
/// who has left has left.
#[tokio::test]
async fn the_access_settings_accept_an_active_worker_and_nobody_else() {
    use axum::http::StatusCode;
    use oxy_app::server::api::org_teams::dto::{GranteeRef, SetAppAccessRequest};
    use oxy_app::server::api::org_teams::service::write_access;

    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let app = apps::Entity::find_by_id(fx.app_a)
        .one(&db)
        .await
        .expect("query")
        .expect("app");
    let actor = user(&db, "Admin", Some("admin@example.com".into())).await;

    let crew = user(&db, "Maria S.", None).await;
    enrol(&db, fx.org, crew, "active").await;
    let gone = user(&db, "Gone A.", None).await;
    enrol(&db, fx.org, gone, "suspended").await;
    let outsider = user(&db, "Nia O.", Some("nia@example.com".into())).await;

    let req = |id: Uuid| SetAppAccessRequest {
        visibility: "members".into(),
        grants: vec![GranteeRef::User {
            id,
            role: "member".into(),
        }],
    };

    let written = write_access(&db, &app, actor, &req(crew))
        .await
        .expect("an active worker is a valid grantee");
    assert!(
        written.grants.iter().any(|g| g.id == crew),
        "the grant reads back: {written:?}"
    );
    // And it is the grant the gate reads.
    assert!(frontline_worker_with_app_grant(&db, fx.org, crew, fx.workspace_a).await);

    assert_eq!(
        write_access(&db, &app, actor, &req(gone)).await.err(),
        Some(StatusCode::BAD_REQUEST),
        "a suspended worker has left; they are not a grantee an admin may ADD"
    );
    assert_eq!(
        write_access(&db, &app, actor, &req(outsider)).await.err(),
        Some(StatusCode::BAD_REQUEST),
        "no standing of either kind is still refused"
    );

    // Suspending a worker who already holds the grant must not make the app's
    // access un-savable: the dialog seeds its full-replace payload from what is
    // there, and suspension deletes nothing. The existing row passes through;
    // it is inert while they are suspended (the loader reads active rows only)
    // and live again when they are reinstated.
    oxy_auth::frontline::set_worker_standing(&db, fx.org, crew, false)
        .await
        .expect("suspend");
    write_access(&db, &app, actor, &req(crew))
        .await
        .expect("re-saving a list that still names a suspended worker is fine");
    assert!(
        !frontline_worker_with_app_grant(&db, fx.org, crew, fx.workspace_a).await,
        "and the surviving grant opens nothing while they are suspended"
    );
}

/// Granting at enrolment is deciding an app's audience, and is held to the
/// ring the access settings enforce — not to the org-admin guard the route
/// already had. An org officer may; a plain member may not.
#[tokio::test]
async fn granting_at_enrolment_needs_the_standing_to_manage_app_access() {
    use oxy_app::server::api::frontline_grants::may_grant_apps;

    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let mut standing = Vec::new();
    for (name, role) in [
        ("Owner", entity::org_members::OrgRole::Admin),
        ("Member", entity::org_members::OrgRole::Member),
    ] {
        let id = user(
            &db,
            name,
            Some(format!("{}@example.com", name.to_lowercase())),
        )
        .await;
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(fx.org),
            user_id: ActiveValue::Set(id),
            role: ActiveValue::Set(role),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed membership");
        standing.push((name, id));
    }
    let (_, admin) = standing[0];
    let (_, member) = standing[1];
    let as_session = oxy_server_authz::Caller::without_credential;
    assert!(may_grant_apps(&db, &as_session(admin, "owner@example.com"), fx.org).await);
    assert!(
        !may_grant_apps(&db, &as_session(member, "member@example.com"), fx.org).await,
        "a plain member cannot decide an app's audience through enrolment either"
    );
}

/// Enrolment can grant the apps the worker will use — one call for the manager
/// — and a request naming another org's app is refused before the worker exists.
#[tokio::test]
async fn enrolment_grants_only_this_orgs_apps_and_is_idempotent() {
    use oxy_app::server::api::frontline_grants::{
        GrantError, grant_apps_to_worker, validate_apps_in_org,
    };

    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let other = seed(&db).await; // another org, its own workspaces and apps
    let crew = user(&db, "Maria S.", None).await;
    enrol(&db, fx.org, crew, "active").await;

    // Another org's app, or one that does not exist: refused, naming the ids.
    let stranger = Uuid::new_v4();
    match validate_apps_in_org(&db, fx.org, &[fx.app_a, other.app_a, stranger]).await {
        Err(GrantError::NotThisOrg(ids)) => {
            assert!(ids.contains(&other.app_a) && ids.contains(&stranger));
            assert!(!ids.contains(&fx.app_a));
        }
        other => panic!("expected NotThisOrg, got {other:?}"),
    }
    // Nothing was written by asking.
    assert!(!frontline_worker_with_app_grant(&db, fx.org, crew, fx.workspace_a).await);

    // Both of this org's apps, then the same call again: two rows, not four,
    // and the gate opens both workspaces.
    let granted = grant_apps_to_worker(&db, fx.org, crew, &[fx.app_a, fx.app_b], None)
        .await
        .expect("grant");
    assert_eq!(granted.len(), 2);
    assert!(
        granted.iter().all(|a| a.org_id == fx.org),
        "the rows come back named"
    );
    grant_apps_to_worker(&db, fx.org, crew, &[fx.app_a, fx.app_b], None)
        .await
        .expect("granting again is a no-op");
    let rows = app_members::Entity::find()
        .filter(app_members::Column::UserId.eq(crew))
        .all(&db)
        .await
        .expect("query");
    assert_eq!(rows.len(), 2, "idempotent: {rows:?}");
    assert!(frontline_worker_with_app_grant(&db, fx.org, crew, fx.workspace_a).await);
    assert!(frontline_worker_with_app_grant(&db, fx.org, crew, fx.workspace_b).await);
}

/// The manager's surface: the crew as a list, and a worker's apps made exactly
/// a given set — additions granted, the rest revoked, foreign apps refused,
/// unknown workers refused.
#[tokio::test]
async fn an_admin_lists_the_crew_and_replaces_a_workers_apps() {
    use oxy_app::server::api::frontline_admin::{ReplaceError, replace_worker_apps, workers_of};
    use oxy_app::server::api::frontline_grants::GrantError;
    use oxy_auth::frontline::{PinPolicy, enroll_worker};

    let (db, _url) = fresh_db(Schema::Central).await;
    let fx = seed(&db).await;
    let other = seed(&db).await;
    assert!(
        workers_of(&db, fx.org)
            .await
            .expect("empty crew")
            .is_empty()
    );

    // A real enrolment, so the row carries an identifier.
    let maria = enroll_worker(
        &db,
        fx.org,
        "Maria S.",
        "maria.s",
        "4821",
        PinPolicy::default(),
    )
    .await
    .expect("enrol");
    grant(&db, fx.app_a, maria).await;

    let crew = workers_of(&db, fx.org).await.expect("crew");
    assert_eq!(crew.len(), 1);
    assert_eq!(crew[0].identifier, "maria.s");
    assert_eq!(crew[0].status, "active");
    assert_eq!(crew[0].apps, vec![fx.app_a]);
    assert!(crew[0].locked_until.is_none());
    assert!(
        workers_of(&db, other.org)
            .await
            .expect("other org")
            .is_empty(),
        "the crew is the org's, not the tenant's neighbours'"
    );

    // Replace: app_a out, app_b in — one grant added, one revoked, the gate agrees.
    let changed = replace_worker_apps(&db, fx.org, maria, &[fx.app_b], None)
        .await
        .expect("replace");
    assert_eq!(
        changed.added.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![fx.app_b]
    );
    assert_eq!(
        changed.removed.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![fx.app_a]
    );
    assert_eq!(changed.apps, vec![fx.app_b]);
    assert!(!frontline_worker_with_app_grant(&db, fx.org, maria, fx.workspace_a).await);
    assert!(frontline_worker_with_app_grant(&db, fx.org, maria, fx.workspace_b).await);
    assert_eq!(
        workers_of(&db, fx.org).await.expect("crew")[0].apps,
        vec![fx.app_b]
    );

    // The same list again moves nothing; an empty list revokes everything.
    let again = replace_worker_apps(&db, fx.org, maria, &[fx.app_b], None)
        .await
        .expect("replace again");
    assert!(again.added.is_empty() && again.removed.is_empty());
    let none = replace_worker_apps(&db, fx.org, maria, &[], None)
        .await
        .expect("revoke all");
    assert_eq!(none.removed.len(), 1);
    assert!(none.apps.is_empty());

    // Another org's app: refused by id, nothing written. Unknown worker: 404.
    assert!(matches!(
        replace_worker_apps(&db, fx.org, maria, &[other.app_a], None).await,
        Err(ReplaceError::Grant(GrantError::NotThisOrg(_)))
    ));
    assert!(matches!(
        replace_worker_apps(&db, fx.org, Uuid::new_v4(), &[fx.app_a], None).await,
        Err(ReplaceError::NotFound)
    ));
}
