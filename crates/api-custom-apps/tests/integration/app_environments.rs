//! `app_environments`: the model behind staging, production and sandboxes
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3).
//!
//! Database-backed: each test gets its own database cloned from the migrated
//! template. The migration that dropped `apps.draft_build_id` /
//! `apps.published_build_id` has already run on it, so its backfill is
//! exercised by putting the columns back, seeding pointers and running
//! `DROP_POINTER_COLUMNS_BACKFILL_SQL` directly.

use crate::common::test_db;
use entity::{app_builds, apps, organizations, users, workspaces};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

// ── Seeding ─────────────────────────────────────────────────────────────────

pub(crate) async fn seed_user(conn: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("env-{id}@example.com"))),
        name: ActiveValue::Set("Environments Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed user");
    id
}

/// A fresh org, workspace and app with no build pointers.
pub(crate) async fn seed_app(conn: &DatabaseConnection) -> Uuid {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Env Org".into()),
        slug: ActiveValue::Set(format!("env-org-{}", org_id.simple())),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");

    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("Env Workspace".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed workspace");

    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        slug: ActiveValue::Set(format!("env-app-{}", app_id.simple())),
        name: ActiveValue::Set("Env App".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("env/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app");
    app_id
}

pub(crate) async fn seed_build(conn: &DatabaseConnection, app_id: Uuid, label: &str) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app_id),
        build_id: ActiveValue::Set(format!("{label}-{}", id.simple())),
        s3_prefix: ActiveValue::Set(format!("customer-apps/{app_id}/builds/{label}/")),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        validation_status: ActiveValue::Set("passed".into()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed build");
    id
}

/// Point staging (`draft`) and production (`published`) at builds directly,
/// with no history: the rows, as a pointer written outside the publish path
/// would leave them. `None` clears an environment that has a row.
pub(crate) async fn point(
    conn: &DatabaseConnection,
    app_id: Uuid,
    draft: Option<Uuid>,
    published: Option<Uuid>,
) {
    set_pointer(conn, app_id, &AppEnvironment::Staging, draft).await;
    set_pointer(conn, app_id, &AppEnvironment::Production, published).await;
}

/// Point one fixed environment at a build directly, with no history. `None`
/// clears it if it has a row.
pub(crate) async fn set_pointer<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    build: Option<Uuid>,
) {
    let statement = match build {
        Some(_) => {
            "INSERT INTO app_environments (app_id, name, kind, build_id) \
             VALUES ($1, $2, $2, $3) \
             ON CONFLICT (app_id, name) DO UPDATE SET build_id = EXCLUDED.build_id"
        }
        None => "UPDATE app_environments SET build_id = $3 WHERE app_id = $1 AND name = $2",
    };
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        statement,
        [app_id.into(), environment.name().into(), build.into()],
    ))
    .await
    .expect("point app");
}

/// The builds production and staging serve, from their environment rows —
/// what `apps.published_build_id` / `apps.draft_build_id` used to say.
pub(crate) async fn serving<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
) -> oxy_api_custom_apps::custom_apps_environments::FixedPointers {
    oxy_api_custom_apps::custom_apps_environments::fixed_pointers(conn, app_id)
        .await
        .expect("read the environments")
}

/// `(name, build_id)` for every environment of the app, ordered by name.
pub(crate) async fn env_rows(
    conn: &DatabaseConnection,
    app_id: Uuid,
) -> Vec<(String, Option<Uuid>)> {
    conn.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT name, build_id FROM app_environments WHERE app_id = $1 ORDER BY name",
        [app_id.into()],
    ))
    .await
    .expect("read environments")
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String>("", "name").expect("name"),
            row.try_get::<Option<Uuid>>("", "build_id")
                .expect("build_id"),
        )
    })
    .collect()
}

/// `(environment, build_id, action)` for every event of the app, oldest first.
pub(crate) async fn event_rows(
    conn: &DatabaseConnection,
    app_id: Uuid,
) -> Vec<(String, Option<Uuid>, String)> {
    conn.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT environment, build_id, action FROM app_environment_events \
         WHERE app_id = $1 ORDER BY at, environment",
        [app_id.into()],
    ))
    .await
    .expect("read events")
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String>("", "environment")
                .expect("environment"),
            row.try_get::<Option<Uuid>>("", "build_id")
                .expect("build_id"),
            row.try_get::<String>("", "action").expect("action"),
        )
    })
    .collect()
}

/// `(action, actor)` for every event of the app, oldest first.
async fn event_actors(conn: &DatabaseConnection, app_id: Uuid) -> Vec<(String, Option<Uuid>)> {
    conn.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT action, actor FROM app_environment_events \
         WHERE app_id = $1 ORDER BY at, environment",
        [app_id.into()],
    ))
    .await
    .expect("read event actors")
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String>("", "action").expect("action"),
            row.try_get::<Option<Uuid>>("", "actor").expect("actor"),
        )
    })
    .collect()
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

/// Put the dropped columns back, as the database was before the migration, so
/// the migration's backfill has pointers to read.
async fn restore_pointer_columns(conn: &DatabaseConnection) {
    conn.execute_unprepared(
        "ALTER TABLE apps ADD COLUMN IF NOT EXISTS draft_build_id UUID, \
         ADD COLUMN IF NOT EXISTS published_build_id UUID",
    )
    .await
    .expect("restore the pointer columns");
}

/// Set the dropped pointer columns of an app, as pre-migration data looked.
async fn set_pointer_columns(
    conn: &DatabaseConnection,
    app_id: Uuid,
    draft: Option<Uuid>,
    published: Option<Uuid>,
) {
    restore_pointer_columns(conn).await;
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE apps SET draft_build_id = $1, published_build_id = $2 WHERE id = $3",
        [draft.into(), published.into(), app_id.into()],
    ))
    .await
    .expect("set the pointer columns");
}

async fn run_backfill(conn: &DatabaseConnection) {
    conn.execute_unprepared(migration::DROP_POINTER_COLUMNS_BACKFILL_SQL)
        .await
        .expect("backfill runs");
}

// ── The migration that drops the pointer columns ────────────────────────────

#[tokio::test]
async fn the_pointer_columns_are_gone() {
    let conn = test_db().await;
    let columns = conn
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = 'apps' \
               AND column_name IN ('draft_build_id', 'published_build_id')",
        ))
        .await
        .expect("read columns");
    assert!(
        columns.is_empty(),
        "environments are the only pointer store; apps keeps no build pointer"
    );
}

#[tokio::test]
async fn backfill_maps_draft_to_staging_and_published_to_production() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    let live = seed_build(&conn, app, "live").await;
    set_pointer_columns(&conn, app, Some(draft), Some(live)).await;

    run_backfill(&conn).await;

    assert_eq!(
        env_rows(&conn, app).await,
        vec![
            ("production".into(), Some(live)),
            ("staging".into(), Some(draft))
        ]
    );
    // Every build a pointer names counts as served by staging, so each stays
    // promotable; the live one was production's too.
    assert_eq!(
        sorted(event_rows(&conn, app).await),
        sorted(vec![
            ("production".into(), Some(live), "backfill".into()),
            ("staging".into(), Some(draft), "backfill".into()),
            ("staging".into(), Some(live), "backfill".into()),
        ])
    );
}

#[tokio::test]
async fn backfill_gives_a_published_only_app_no_staging_row() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let live = seed_build(&conn, app, "live").await;
    set_pointer_columns(&conn, app, None, Some(live)).await;

    run_backfill(&conn).await;

    assert_eq!(
        env_rows(&conn, app).await,
        vec![("production".into(), Some(live))]
    );
    assert_eq!(
        sorted(event_rows(&conn, app).await),
        sorted(vec![
            ("production".into(), Some(live), "backfill".into()),
            ("staging".into(), Some(live), "backfill".into()),
        ])
    );
}

#[tokio::test]
async fn backfill_gives_a_draft_only_app_no_production_row() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    set_pointer_columns(&conn, app, Some(draft), None).await;

    run_backfill(&conn).await;

    assert_eq!(
        env_rows(&conn, app).await,
        vec![("staging".into(), Some(draft))]
    );
    assert_eq!(
        event_rows(&conn, app).await,
        vec![("staging".into(), Some(draft), "backfill".into())]
    );
}

#[tokio::test]
async fn backfill_skips_a_pointer_to_a_build_that_no_longer_exists() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    // No FK on the dropped columns, so a dangling pointer is representable.
    set_pointer_columns(&conn, app, Some(Uuid::new_v4()), None).await;

    run_backfill(&conn).await;

    assert!(env_rows(&conn, app).await.is_empty());
}

/// A row that exists is what every reader has read since Phase 1b: the
/// backfill never moves it to what the column says, and runs again harmlessly.
#[tokio::test]
async fn backfill_never_touches_a_row_that_exists_and_is_idempotent() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    let live = seed_build(&conn, app, "live").await;
    let current = seed_build(&conn, app, "current").await;
    point(&conn, app, Some(current), None).await;
    set_pointer_columns(&conn, app, Some(draft), Some(live)).await;

    run_backfill(&conn).await;
    let rows = env_rows(&conn, app).await;
    run_backfill(&conn).await;

    assert_eq!(
        rows,
        vec![
            ("production".into(), Some(live)),
            ("staging".into(), Some(current))
        ]
    );
    assert_eq!(env_rows(&conn, app).await, rows);
    // History only for the row the backfill created: staging's existing row
    // gets none, and the second run added nothing.
    assert_eq!(
        sorted(event_rows(&conn, app).await),
        sorted(vec![
            ("production".into(), Some(live), "backfill".into()),
            ("staging".into(), Some(live), "backfill".into()),
        ])
    );
}

#[tokio::test]
async fn down_puts_the_pointers_back_from_the_rows() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    let live = seed_build(&conn, app, "live").await;
    point(&conn, app, Some(draft), Some(live)).await;
    restore_pointer_columns(&conn).await;

    conn.execute_unprepared(migration::DROP_POINTER_COLUMNS_RESTORE_SQL)
        .await
        .expect("restore runs");

    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT draft_build_id, published_build_id FROM apps WHERE id = $1",
            [app.into()],
        ))
        .await
        .expect("read pointers")
        .expect("the app");
    assert_eq!(
        row.try_get::<Option<Uuid>>("", "draft_build_id")
            .expect("draft"),
        Some(draft)
    );
    assert_eq!(
        row.try_get::<Option<Uuid>>("", "published_build_id")
            .expect("published"),
        Some(live)
    );
}

// ── The name rules live in Rust and in a CHECK; they must agree ─────────────

#[tokio::test]
async fn database_and_rust_agree_on_environment_names() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let owner = seed_user(&conn).await;

    for name in [
        "dev-luong",
        "dev-a",
        "dev-a1-b2",
        "dev-abcdefghijkl",  // 12: longest allowed
        "dev-abcdefghijklm", // 13: too long
        "dev-",
        "dev--x",
        "dev-x-",
        "dev-UPPER",
        "dev-a--b",
        "dev-a_b",
    ] {
        let rust_accepts = AppEnvironment::parse(name).is_some();
        let db_accepts = conn
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO app_environments (app_id, name, kind, owner_user_id) \
                 VALUES ($1, $2, 'dev', $3)",
                [app.into(), name.into(), owner.into()],
            ))
            .await
            .is_ok();
        assert_eq!(
            rust_accepts, db_accepts,
            "Rust and the CHECK disagree on {name:?}"
        );
    }
}

// ── record_move / protected_build_ids ───────────────────────────────────────

use oxy_api_custom_apps::custom_apps_environments::{self as envs, EnvAction};

#[tokio::test]
async fn record_move_upserts_the_row_and_appends_an_event_per_move() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let actor = seed_user(&conn).await;
    let first = seed_build(&conn, app, "first").await;
    let second = seed_build(&conn, app, "second").await;

    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(first),
        EnvAction::Publish,
        Some(actor),
    )
    .await
    .expect("first move");
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(second),
        EnvAction::Publish,
        Some(actor),
    )
    .await
    .expect("second move");

    assert_eq!(
        env_rows(&conn, app).await,
        vec![("staging".into(), Some(second))]
    );
    assert_eq!(
        event_rows(&conn, app).await,
        vec![
            ("staging".into(), Some(first), "publish".into()),
            ("staging".into(), Some(second), "publish".into()),
        ]
    );
}

#[tokio::test]
async fn record_move_can_clear_an_environment() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let live = seed_build(&conn, app, "live").await;

    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Production,
        Some(live),
        EnvAction::Promote,
        None,
    )
    .await
    .expect("promote");
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Production,
        None,
        EnvAction::Unpublish,
        None,
    )
    .await
    .expect("unpublish");

    assert_eq!(
        env_rows(&conn, app).await,
        vec![("production".into(), None)]
    );
}

/// Insert a sandbox row the way `custom_apps_sandboxes::ops::create` does:
/// `kind = 'dev'`, an owner, no build.
pub(crate) async fn seed_sandbox(conn: &DatabaseConnection, app: Uuid, name: &str, owner: Uuid) {
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_environments (app_id, name, kind, owner_user_id, updated_by) \
         VALUES ($1, $2, 'dev', $3, $3)",
        [app.into(), name.into(), owner.into()],
    ))
    .await
    .expect("seed sandbox");
}

fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

/// A sandbox's row needs an owner and is created by the sandbox API, never as
/// a side effect of a pointer move: with no row, the move is `RecordNotFound`
/// and writes nothing — no row, no event.
#[tokio::test]
async fn record_move_never_creates_a_sandbox() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let build = seed_build(&conn, app, "b").await;

    let result = envs::record_move(
        &conn,
        app,
        &sandbox("luong"),
        Some(build),
        EnvAction::Publish,
        None,
    )
    .await;

    assert!(
        matches!(result, Err(sea_orm::DbErr::RecordNotFound(_))),
        "{result:?}"
    );
    assert!(env_rows(&conn, app).await.is_empty());
    assert!(event_rows(&conn, app).await.is_empty());
}

/// An existing sandbox's pointer moves, with its event; the row keeps its
/// owner and kind, and no other environment's row is touched.
#[tokio::test]
async fn record_move_moves_an_existing_sandbox_and_nothing_else() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let owner = seed_user(&conn).await;
    let actor = seed_user(&conn).await;
    let staging = seed_build(&conn, app, "staging").await;
    let build = seed_build(&conn, app, "sandbox").await;
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(staging),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("staging");
    seed_sandbox(&conn, app, "dev-a1", owner).await;
    seed_sandbox(&conn, app, "dev-b2", owner).await;

    envs::record_move(
        &conn,
        app,
        &sandbox("a1"),
        Some(build),
        EnvAction::Publish,
        Some(actor),
    )
    .await
    .expect("the sandbox exists");

    assert_eq!(
        env_rows(&conn, app).await,
        vec![
            ("dev-a1".into(), Some(build)),
            ("dev-b2".into(), None),
            ("staging".into(), Some(staging)),
        ]
    );
    assert_eq!(
        event_rows(&conn, app).await,
        vec![
            ("staging".into(), Some(staging), "publish".into()),
            ("dev-a1".into(), Some(build), "publish".into()),
        ]
    );
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT kind, owner_user_id, updated_by FROM app_environments \
             WHERE app_id = $1 AND name = 'dev-a1'",
            [app.into()],
        ))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.try_get::<String>("", "kind").unwrap(), "dev");
    assert_eq!(
        row.try_get::<Option<Uuid>>("", "owner_user_id").unwrap(),
        Some(owner),
        "a move never changes the owner"
    );
    assert_eq!(
        row.try_get::<Option<Uuid>>("", "updated_by").unwrap(),
        Some(actor)
    );
}

/// A sandbox being torn down takes no pointer move: a publish that raced the
/// delete must not put a build back behind a name that is going away.
#[tokio::test]
async fn record_move_refuses_a_sandbox_being_deleted() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let owner = seed_user(&conn).await;
    let build = seed_build(&conn, app, "b").await;
    seed_sandbox(&conn, app, "dev-a1", owner).await;
    conn.execute_unprepared(
        "UPDATE app_environments SET deleting_at = now() WHERE name = 'dev-a1'",
    )
    .await
    .expect("mark deleting");

    let result = envs::record_move(
        &conn,
        app,
        &sandbox("a1"),
        Some(build),
        EnvAction::Publish,
        None,
    )
    .await;

    assert!(
        matches!(result, Err(sea_orm::DbErr::RecordNotFound(_))),
        "{result:?}"
    );
    assert_eq!(env_rows(&conn, app).await, vec![("dev-a1".into(), None)]);
    assert!(event_rows(&conn, app).await.is_empty());
}

#[tokio::test]
async fn protected_build_ids_lists_every_environment_build() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let staging = seed_build(&conn, app, "staging").await;
    let live = seed_build(&conn, app, "live").await;
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(staging),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("staging");
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Production,
        Some(live),
        EnvAction::Promote,
        None,
    )
    .await
    .expect("production");

    let protected = sorted(
        envs::protected_build_ids(&conn, app)
            .await
            .expect("protected"),
    );

    assert_eq!(protected, sorted(vec![staging, live]));
}

// ── The shared transitions move the environments ────────────────────────────

use oxy_api_custom_apps::admin::apps::handlers as admin_apps;
use oxy_api_custom_apps::custom_apps_promote::AuditedBy;

/// `user` as a request's actor: a browser session, no key or token.
fn session_of(user: Uuid) -> oxy_app_core::audit::RequestActor {
    oxy_app_core::audit::RequestActor::session(oxy_auth::types::AuthenticatedUser {
        id: user,
        email: None,
        name: "Environments Test".to_string(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    })
}

#[tokio::test]
async fn publish_one_and_unpublish_one_move_production() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let actor = seed_user(&conn).await;
    // A different user unpublishes, so the actor assertion below can't pass by
    // reusing the publisher.
    let unpublisher = seed_user(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    // The draft as a publish leaves it: staging serves it, so production may
    // (invariant 1, checked by the production move `publish_one` goes through).
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(draft),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("staging serves the draft");

    assert!(
        admin_apps::publish_one(&conn, app, &session_of(actor), AuditedBy::Caller)
            .await
            .is_ok(),
        "publish_one"
    );
    assert_eq!(
        env_rows(&conn, app).await,
        vec![
            ("production".into(), Some(draft)),
            ("staging".into(), Some(draft)),
        ]
    );

    assert!(
        admin_apps::unpublish_one(&conn, app, &session_of(unpublisher), AuditedBy::Caller)
            .await
            .is_ok(),
        "unpublish_one"
    );
    assert_eq!(
        env_rows(&conn, app).await,
        vec![("production".into(), None), ("staging".into(), Some(draft))]
    );

    assert_eq!(
        event_rows(&conn, app).await,
        vec![
            ("staging".into(), Some(draft), "publish".into()),
            ("production".into(), Some(draft), "promote".into()),
            ("production".into(), None, "unpublish".into()),
        ]
    );
    // app_environment_events is append-only: an actor not written here can never be
    // backfilled.
    assert_eq!(
        event_actors(&conn, app).await,
        vec![
            ("publish".into(), None),
            ("promote".into(), Some(actor)),
            ("unpublish".into(), Some(unpublisher)),
        ]
    );
}

/// The console's Make Live goes through the one production move: a draft
/// staging never served — a pointer no publish wrote — is refused
/// `409 staging_never_served`, and production and its history stay as they
/// were.
#[tokio::test]
async fn publish_one_refuses_a_draft_staging_never_served() {
    use axum::response::IntoResponse;

    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let actor = seed_user(&conn).await;
    let draft = seed_build(&conn, app, "unstaged").await;
    point(&conn, app, Some(draft), None).await;

    let refused = admin_apps::publish_one(&conn, app, &session_of(actor), AuditedBy::Caller)
        .await
        .expect_err("staging never served the draft");
    let response = admin_apps::PromoteRefusal::from(refused).into_response();
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(body["code"], "staging_never_served", "{body}");
    assert_eq!(
        env_rows(&conn, app).await,
        vec![("staging".into(), Some(draft))],
        "production did not move"
    );
    assert!(event_rows(&conn, app).await.is_empty(), "nothing recorded");
}

// ── Builds retained from before the events stay rollback targets ────────────

/// A build retention kept from before the environment events has no event and
/// no `published_to`, so invariant 1 would refuse the console's rollback to it.
/// The retained-builds backfill gives it its `staging / backfill` event — and
/// none to a sandbox's build or a token's — after which the rollback ships it.
#[tokio::test]
async fn a_build_from_before_the_events_stays_a_rollback_target() {
    use oxy_api_custom_apps::custom_apps_promote::{ProductionMove, PromoteError, move_production};

    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let actor = seed_user(&conn).await;
    let old = seed_build(&conn, app, "pre-events").await;
    let sandboxed = seed_build(&conn, app, "sandboxed").await;
    let by_token = seed_build(&conn, app, "by-token").await;
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_builds SET published_to = CASE WHEN id = $1 THEN 'dev-a1' END, \
         published_token_id = CASE WHEN id = $2 THEN $2 END WHERE id IN ($1, $2)",
        [sandboxed.into(), by_token.into()],
    ))
    .await
    .expect("mark the sandbox's and the token's builds");
    let rollback = ProductionMove {
        app_id: app,
        build: old.into(),
        action: EnvAction::Rollback,
        actor: Some(actor),
        actor_via: None,
        expect_current: None,
        refuse_same: false,
    };

    let refused = move_production(&conn, &rollback)
        .await
        .expect_err("no event yet");
    assert!(
        matches!(&refused, PromoteError::Blocked(b) if b[0].code == "staging_never_served"),
        "{refused:?}"
    );
    for _ in 0..2 {
        conn.execute_unprepared(migration::RETAINED_BUILDS_STAGING_BACKFILL_SQL)
            .await
            .expect("run the retained-builds backfill");
    }
    assert_eq!(
        event_rows(&conn, app).await,
        vec![("staging".into(), Some(old), "backfill".into())],
        "one event, for the channel build alone, however often it runs"
    );
    let moved = move_production(&conn, &rollback)
        .await
        .expect("a build staging served goes live");
    assert_eq!(moved.app.id, app);
    assert_eq!(
        envs::pointer(&conn, app, &AppEnvironment::Production)
            .await
            .expect("read production"),
        Some(old)
    );
}

// ── Builds an environment serves are undeletable; app deletion still cascades ─

#[tokio::test]
async fn a_build_an_environment_serves_cannot_be_deleted_but_the_app_can() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let build = seed_build(&conn, app, "served").await;
    envs::record_move(
        &conn,
        app,
        &AppEnvironment::Staging,
        Some(build),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("serve the build");

    let delete_build = conn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM app_builds WHERE id = $1",
            [build.into()],
        ))
        .await;
    assert!(
        delete_build.is_err(),
        "the environment FK must refuse to delete a served build"
    );

    // NO ACTION (not RESTRICT) is checked at end of statement, so the cascade from
    // `apps` to both `app_builds` and `app_environments` completes.
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM apps WHERE id = $1",
        [app.into()],
    ))
    .await
    .expect("deleting the app cascades cleanly");
    assert!(env_rows(&conn, app).await.is_empty());
}
