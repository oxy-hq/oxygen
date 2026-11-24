//! An org with a custom app whose live build declares the brief's catalogue,
//! a member, a second member, someone outside the org, and the three people
//! the notification catalogue segments by: an owner, an admin, and a plain
//! member an operator granted the Factory in the workspace.

use chrono::{Duration, Timelike, Utc};
use entity::{
    app_builds, apps, notifications, org_members, users, workspace_factory_grants, workspaces,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) const APP_SLUG: &str = "store-ops";

pub(super) struct World {
    pub workspace: Uuid,
    pub app: apps::Model,
    pub member: AuthenticatedUser,
    pub other: AuthenticatedUser,
    pub stranger: AuthenticatedUser,
    /// An owner of the org: every audience admits them.
    pub owner: AuthenticatedUser,
    /// An admin: an operator and an officer, but not an owner, so no Billing.
    pub admin: AuthenticatedUser,
    /// A plain org member holding a Factory grant on `workspace`: an operator
    /// with no standing in the org at all.
    pub grantee: AuthenticatedUser,
}

/// `operations` → `operations/food-safety`, `operations/receiving`; a
/// time-sensitive kind with an open action, a critical one whose action runs
/// `ack-alarm`, a passive one, one a level down, and `shift_note`, which a
/// function sends and the catalogue does not place.
pub(super) fn manifest() -> Value {
    json!({
        "functions": {
            "lists": { "notify": { "kinds": {
                "list_overdue": {}, "temp_alarm": {}, "stock_low": {}, "delivery_late": {},
                "shift_note": {} } } },
            "ack-alarm": { "route": true }
        },
        "notifications": {
            "categories": {
                "operations": { "label": "Store operations" },
                "operations/food-safety": { "label": "Food safety", "description": "Temperature logs and checks" },
                "operations/receiving": {}
            },
            "kinds": {
                "list_overdue": { "category": "operations", "label": "Overdue lists", "level": "time_sensitive",
                                  "actions": [{ "id": "open", "label": "Open list" }] },
                "temp_alarm": { "category": "operations/food-safety", "label": "Temperature alarm", "level": "critical",
                                "actions": [{ "id": "ack", "label": "Acknowledge", "function": "ack-alarm" }] },
                "stock_low": { "category": "operations", "level": "passive" },
                "delivery_late": { "category": "operations/receiving" }
            }
        }
    })
}

pub(super) fn signed_in(id: Uuid) -> AuthenticatedUser {
    AuthenticatedUser {
        id,
        email: None,
        name: "Person".into(),
        picture: None,
        status: users::UserStatus::Active,
        credential: None,
    }
}

pub(super) async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("p-{id}@example.com"))),
        name: Set("Person".into()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

pub(super) async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid) {
    join_as(db, org, user, org_members::OrgRole::Member).await;
}

pub(super) async fn join_as(
    db: &DatabaseConnection,
    org: Uuid,
    user: Uuid,
    role: org_members::OrgRole,
) {
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        role: Set(role),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
}

/// The row that IS a Factory grant — what `Ring::Factory` reads.
pub(super) async fn grant_factory(db: &DatabaseConnection, workspace: Uuid, user: Uuid) {
    workspace_factory_grants::ActiveModel {
        workspace_id: Set(workspace),
        user_id: Set(user),
        granted_by: Set(None),
        granted_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("grant the factory");
}

/// An app in `org` whose live (published) build declares `manifest`.
pub(super) async fn seed_app(
    db: &DatabaseConnection,
    org: Uuid,
    workspace: Uuid,
    slug: &str,
    manifest: Option<Value>,
) -> apps::Model {
    let app = seed_bare_app(db, org, workspace, slug).await;
    publish(db, &app, manifest).await
}

/// An app in `org` with no build at all.
pub(super) async fn seed_bare_app(
    db: &DatabaseConnection,
    org: Uuid,
    workspace: Uuid,
    slug: &str,
) -> apps::Model {
    apps::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        project_id: Set(workspace),
        slug: Set(slug.into()),
        name: Set(format!("App {slug}")),
        branch: Set("main".into()),
        source_repo: Set("git@example.com:acme/app.git".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app")
}

/// A new build of `app` declaring `manifest`, made live.
pub(super) async fn publish(
    db: &DatabaseConnection,
    app: &apps::Model,
    manifest: Option<Value>,
) -> apps::Model {
    let build = seed_build(db, app, manifest).await;
    point(db, app, AppEnvironment::Production, build).await;
    app.clone()
}

/// A new build of `app` declaring `manifest`, as its draft only — what a
/// never-promoted app's production runs.
pub(super) async fn draft(
    db: &DatabaseConnection,
    app: &apps::Model,
    manifest: Option<Value>,
) -> apps::Model {
    let build = seed_build(db, app, manifest).await;
    point(db, app, AppEnvironment::Staging, build).await;
    app.clone()
}

/// Point `environment` of `app` at `build`, through the one pointer writer.
async fn point(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: AppEnvironment,
    build: Uuid,
) {
    oxy_custom_apps::environments::record_move(
        db,
        app.id,
        &environment,
        Some(build),
        oxy_custom_apps::environments::EnvAction::Publish,
        None,
    )
    .await
    .expect("move the pointer");
}

async fn seed_build(db: &DatabaseConnection, app: &apps::Model, manifest: Option<Value>) -> Uuid {
    let build = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(build),
        app_id: Set(app.id),
        build_id: Set(format!("b-{}", build.simple())),
        s3_prefix: Set(format!("builds/{build}/")),
        manifest_json: Set(manifest),
        created_at: Set(Utc::now().fixed_offset()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    build
}

pub(super) async fn seed(db: &DatabaseConnection, org: Uuid) -> World {
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace),
        org_id: Set(Some(org)),
        name: Set("ops".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app = seed_app(db, org, workspace, APP_SLUG, Some(manifest())).await;
    let (member, other, stranger) = (
        seed_user(db).await,
        seed_user(db).await,
        seed_user(db).await,
    );
    join(db, org, member).await;
    join(db, org, other).await;
    let (owner, admin, grantee) = (
        seed_user(db).await,
        seed_user(db).await,
        seed_user(db).await,
    );
    join_as(db, org, owner, org_members::OrgRole::Owner).await;
    join_as(db, org, admin, org_members::OrgRole::Admin).await;
    join(db, org, grantee).await;
    grant_factory(db, workspace, grantee).await;
    World {
        workspace,
        app,
        member: signed_in(member),
        other: signed_in(other),
        stranger: signed_in(stranger),
        owner: signed_in(owner),
        admin: signed_in(admin),
        grantee: signed_in(grantee),
    }
}

/// A signal for `user`, `ago` seconds old, written straight to Oxy's database
/// (no content: the inbox reads its words from an org store it does not need
/// here).
pub(super) struct Signal<'a> {
    pub org: Uuid,
    pub user: Uuid,
    pub app: Option<Uuid>,
    pub kind: &'a str,
    pub thread: Option<String>,
    pub ago: i64,
    pub read: bool,
}

pub(super) async fn signal(db: &DatabaseConnection, s: Signal<'_>) -> Uuid {
    let id = Uuid::new_v4();
    // Whole seconds: a Postgres round trip of a finer clock compares unequal
    // on Linux CI.
    let at = (Utc::now() - Duration::seconds(s.ago))
        .with_nanosecond(0)
        .expect("zero is a valid nanosecond")
        .fixed_offset();
    notifications::ActiveModel {
        id: Set(id),
        org_id: Set(s.org),
        user_id: Set(s.user),
        kind: Set(s.kind.into()),
        title: Set(String::new()),
        body: Set(None),
        link: Set(None),
        subject_kind: Set(None),
        subject_id: Set(None),
        created_at: Set(at),
        read_at: Set(s.read.then_some(at)),
        message_id: Set(None),
        app_id: Set(s.app),
        thread_key: Set(s.thread),
    }
    .insert(db)
    .await
    .expect("seed signal");
    id
}

pub(super) async fn read_at(
    db: &DatabaseConnection,
    id: Uuid,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    notifications::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query")
        .expect("the signal exists")
        .read_at
}
