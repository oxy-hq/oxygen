//! `app_environments`: the model behind staging, production and dev slots
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3).
//!
//! Database-backed: each test gets its own database cloned from the migrated
//! template, so the backfill is exercised by seeding pointers and running
//! `APP_ENVIRONMENTS_BACKFILL_SQL` directly.

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

/// Set the legacy pointer columns directly, the way pre-migration data looks.
pub(crate) async fn point(
    conn: &DatabaseConnection,
    app_id: Uuid,
    draft: Option<Uuid>,
    published: Option<Uuid>,
) {
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE apps SET draft_build_id = $1, published_build_id = $2 WHERE id = $3",
        [draft.into(), published.into(), app_id.into()],
    ))
    .await
    .expect("point app");
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

async fn run_backfill(conn: &DatabaseConnection) {
    conn.execute_unprepared(migration::APP_ENVIRONMENTS_BACKFILL_SQL)
        .await
        .expect("backfill runs");
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

// ── Backfill ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn backfill_maps_draft_to_staging_and_published_to_production() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    let live = seed_build(&conn, app, "live").await;
    point(&conn, app, Some(draft), Some(live)).await;

    run_backfill(&conn).await;

    assert_eq!(
        env_rows(&conn, app).await,
        vec![
            ("production".into(), Some(live)),
            ("staging".into(), Some(draft))
        ]
    );
    // Both builds count as served by staging; the live one also by production.
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
async fn backfill_gives_a_published_only_app_a_staging_row() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let live = seed_build(&conn, app, "live").await;
    point(&conn, app, None, Some(live)).await;

    run_backfill(&conn).await;

    assert_eq!(
        env_rows(&conn, app).await,
        vec![
            ("production".into(), Some(live)),
            ("staging".into(), Some(live))
        ]
    );
}

#[tokio::test]
async fn backfill_gives_a_draft_only_app_no_production_row() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    point(&conn, app, Some(draft), None).await;

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
    // No FK on the legacy columns, so a dangling pointer is representable.
    point(&conn, app, Some(Uuid::new_v4()), None).await;

    run_backfill(&conn).await;

    assert!(env_rows(&conn, app).await.is_empty());
    assert!(event_rows(&conn, app).await.is_empty());
}

#[tokio::test]
async fn backfill_is_idempotent() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    let live = seed_build(&conn, app, "live").await;
    point(&conn, app, Some(draft), Some(live)).await;

    run_backfill(&conn).await;
    let rows = env_rows(&conn, app).await;
    let events = event_rows(&conn, app).await.len();
    run_backfill(&conn).await;

    assert_eq!(env_rows(&conn, app).await, rows);
    assert_eq!(event_rows(&conn, app).await.len(), events);
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

use oxy_app::server::api::custom_apps_environments::{self as envs, EnvAction};

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

#[tokio::test]
async fn record_move_refuses_dev_slots() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let build = seed_build(&conn, app, "b").await;

    let result = envs::record_move(
        &conn,
        app,
        &AppEnvironment::Dev {
            handle: "luong".into(),
        },
        Some(build),
        EnvAction::Publish,
        None,
    )
    .await;

    assert!(
        result.is_err(),
        "dev slots are created by the dev-slot API, not record_move"
    );
    assert!(env_rows(&conn, app).await.is_empty());
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

// ── Pointer writes mirror into environments ─────────────────────────────────

use oxy_app::server::api::admin::apps::handlers as admin_apps;

#[tokio::test]
async fn publish_one_and_unpublish_one_mirror_production() {
    let conn = test_db().await;
    let app = seed_app(&conn).await;
    let actor = seed_user(&conn).await;
    // A different user unpublishes, so the actor assertion below can't pass by
    // reusing the publisher.
    let unpublisher = seed_user(&conn).await;
    let draft = seed_build(&conn, app, "draft").await;
    point(&conn, app, Some(draft), None).await;

    assert!(
        admin_apps::publish_one(&conn, app, actor).await.is_ok(),
        "publish_one"
    );
    assert_eq!(
        env_rows(&conn, app).await,
        vec![("production".into(), Some(draft))]
    );

    assert!(
        admin_apps::unpublish_one(&conn, app, unpublisher)
            .await
            .is_ok(),
        "unpublish_one"
    );
    assert_eq!(
        env_rows(&conn, app).await,
        vec![("production".into(), None)]
    );

    assert_eq!(
        event_rows(&conn, app).await,
        vec![
            ("production".into(), Some(draft), "promote".into()),
            ("production".into(), None, "unpublish".into()),
        ]
    );
    // app_environment_events is append-only: an actor not written here can never be
    // backfilled.
    assert_eq!(
        event_actors(&conn, app).await,
        vec![
            ("promote".into(), Some(actor)),
            ("unpublish".into(), Some(unpublisher)),
        ]
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
