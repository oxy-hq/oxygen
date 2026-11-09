//! The frontline app grants and the crew roster, against a real database:
//! granting at enrolment (`frontline_grants`) and a manager replacing a
//! worker's apps (`frontline_admin`). Moved from `oxy-app`'s
//! `tests/platform/frontline_app_grant.rs` with the two modules; the gate's
//! own cases stayed there. The fixtures are that file's, copied.

use entity::{app_members, apps, org_frontline_members, org_members, organizations, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_custom_apps::gate::frontline_worker_with_app_grant;

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
            // Published: `user_can_access_app` refuses every customer a draft.
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

/// Granting at enrolment is deciding an app's audience, and is held to the
/// ring the access settings enforce — not to the org-admin guard the route
/// already had. An org officer may; a plain member may not.
#[tokio::test]
async fn granting_at_enrolment_needs_the_standing_to_manage_app_access() {
    use oxy_api_frontline::frontline_grants::may_grant_apps;

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
    use oxy_api_frontline::frontline_grants::{
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
    use oxy_api_frontline::frontline_admin::{ReplaceError, replace_worker_apps, workers_of};
    use oxy_api_frontline::frontline_grants::GrantError;
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
