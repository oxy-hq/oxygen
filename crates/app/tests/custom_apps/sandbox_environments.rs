//! Sandbox environments (`internal-docs/custom-app-sandboxes.md`): what a
//! `dev-<handle>` environment resolves to, and its management routes.
//!
//! Database-backed: each test gets its own database cloned from the migrated
//! template.

use crate::app_environments::{seed_app, seed_build, seed_sandbox, seed_user};
use crate::common::test_db;
use entity::apps;
use oxy_app::server::api::custom_apps_env_resolve::{
    resolve_environment, resolve_function_environment, sandbox_row,
};
use oxy_app::server::api::custom_apps_environments::{self as envs, EnvAction};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait};
use uuid::Uuid;

fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

async fn app_row(conn: &DatabaseConnection, app_id: Uuid) -> apps::Model {
    apps::Entity::find_by_id(app_id)
        .one(conn)
        .await
        .expect("read app")
        .expect("app row")
}

// ── Resolution ──────────────────────────────────────────────────────────────

/// A sandbox serves its own build and nothing else: with no build it serves
/// nothing — it never borrows staging's or production's, on either resolve —
/// and once pointed it serves that build while its neighbour stays empty.
#[tokio::test]
async fn a_sandbox_resolves_its_own_build_and_never_another_environments() {
    let conn = test_db().await;
    let app_id = seed_app(&conn).await;
    let owner = seed_user(&conn).await;
    let staging = seed_build(&conn, app_id, "staging").await;
    let live = seed_build(&conn, app_id, "live").await;
    let mine = seed_build(&conn, app_id, "mine").await;
    for (environment, build, action) in [
        (AppEnvironment::Staging, staging, EnvAction::Publish),
        (AppEnvironment::Production, live, EnvAction::Promote),
    ] {
        envs::record_move(&conn, app_id, &environment, Some(build), action, None)
            .await
            .expect("fixed environment");
    }
    crate::app_environments::point(&conn, app_id, Some(staging), Some(live)).await;
    seed_sandbox(&conn, app_id, "dev-a1", owner).await;
    seed_sandbox(&conn, app_id, "dev-b2", owner).await;
    let app = app_row(&conn, app_id).await;

    for environment in [sandbox("a1"), sandbox("b2")] {
        let page = resolve_environment(&conn, &app, &environment)
            .await
            .expect("resolve");
        assert_eq!(page.environment, environment);
        assert_eq!(page.build_id, None, "{environment}: created with no build");
        let functions = resolve_function_environment(&conn, &app, &environment)
            .await
            .expect("resolve");
        assert_eq!(
            functions.build_id, None,
            "{environment}: the function resolve never falls back to another environment"
        );
    }

    envs::record_move(
        &conn,
        app_id,
        &sandbox("a1"),
        Some(mine),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("publish to the sandbox");

    for resolved in [
        resolve_environment(&conn, &app, &sandbox("a1")).await,
        resolve_function_environment(&conn, &app, &sandbox("a1")).await,
    ] {
        assert_eq!(resolved.expect("resolve").build_id, Some(mine));
    }
    assert_eq!(
        resolve_function_environment(&conn, &app, &sandbox("b2"))
            .await
            .expect("resolve")
            .build_id,
        None,
        "the other sandbox is untouched"
    );
    // Control: the fixed environments still answer their own.
    assert_eq!(
        resolve_environment(&conn, &app, &AppEnvironment::Staging)
            .await
            .expect("resolve")
            .build_id,
        Some(staging)
    );
    assert_eq!(
        resolve_environment(&conn, &app, &AppEnvironment::Production)
            .await
            .expect("resolve")
            .build_id,
        Some(live)
    );
}

/// A name nobody created, and a sandbox being torn down, both resolve to
/// nothing; `sandbox_row` answers `None` for either, and for an environment
/// that is not a sandbox at all.
#[tokio::test]
async fn an_absent_or_deleting_sandbox_resolves_to_nothing() {
    let conn = test_db().await;
    let app_id = seed_app(&conn).await;
    let owner = seed_user(&conn).await;
    let build = seed_build(&conn, app_id, "b").await;
    seed_sandbox(&conn, app_id, "dev-live", owner).await;
    seed_sandbox(&conn, app_id, "dev-gone", owner).await;
    for name in ["dev-live", "dev-gone"] {
        envs::record_move(
            &conn,
            app_id,
            &AppEnvironment::parse(name).unwrap(),
            Some(build),
            EnvAction::Publish,
            None,
        )
        .await
        .expect("publish");
    }
    // A row left with a build while deleting must still not resolve: the
    // marker alone takes it out of service.
    conn.execute_unprepared(
        "UPDATE app_environments SET deleting_at = now() WHERE name = 'dev-gone'",
    )
    .await
    .expect("mark deleting");
    let app = app_row(&conn, app_id).await;

    let live = sandbox_row(&conn, app_id, &sandbox("live"))
        .await
        .expect("read")
        .expect("an active sandbox has a row");
    assert_eq!(live.build_id, Some(build));
    assert_eq!(live.owner_user_id, Some(owner));

    for environment in [sandbox("gone"), sandbox("never")] {
        assert_eq!(
            sandbox_row(&conn, app_id, &environment)
                .await
                .expect("read"),
            None,
            "{environment}"
        );
        assert_eq!(
            resolve_function_environment(&conn, &app, &environment)
                .await
                .expect("resolve")
                .build_id,
            None,
            "{environment}"
        );
        assert_eq!(
            resolve_environment(&conn, &app, &environment)
                .await
                .expect("resolve")
                .build_id,
            None,
            "{environment}"
        );
    }
    for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
        assert_eq!(
            sandbox_row(&conn, app_id, &fixed).await.expect("read"),
            None,
            "{fixed} is not a sandbox"
        );
    }
}

// ── The semantic pin on a sandbox's data-plane reads ────────────────────────

const STAFF: &str = "sandbox-staff@example.com";

/// A `ready` staging revision of `workspace`: what a build pins.
pub(crate) async fn seed_staging_revision(
    conn: &DatabaseConnection,
    workspace: Uuid,
    sha: &str,
) -> Uuid {
    use sea_orm::{ActiveModelTrait, ActiveValue};
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    entity::revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set(sha.into()),
        branch: ActiveValue::Set(Some("feat".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("staging".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("seed revision");
    id
}

async fn pin_build(conn: &DatabaseConnection, build: Uuid, revision: Uuid) {
    use sea_orm::{DatabaseBackend, Statement};
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_builds SET semantic_revision_id = $1 WHERE id = $2",
        [revision.into(), build.into()],
    ))
    .await
    .expect("pin the build");
}

/// A data-plane request naming `environment` with an explicit credential, the
/// way `oxyc` and the SDK behind a bearer send it.
fn data_request(app: Uuid, environment: &str) -> axum::http::HeaderMap {
    use axum::http::HeaderValue;
    let mut h = axum::http::HeaderMap::new();
    h.insert(
        oxy_app::server::api::custom_apps_staging_pin::APP_HEADER,
        HeaderValue::from_str(&app.to_string()).unwrap(),
    );
    h.insert("authorization", HeaderValue::from_static("Bearer t"));
    h.insert("x-oxy-app-env", HeaderValue::from_str(environment).unwrap());
    h
}

/// The page's data reads in a sandbox read the pin of the build **that
/// sandbox** serves — not staging's, and none when its own build pins none —
/// for a viewer who may open non-production. Staging's own answer is the
/// control.
#[tokio::test]
async fn a_sandbox_data_request_reads_the_pin_of_the_sandboxs_own_build() {
    use oxy_app::server::api::custom_apps_staging_pin::staging_pin_for_data_request as pin_for;
    let conn = test_db().await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_OWNER", STAFF) };
    let app_id = seed_app(&conn).await;
    let owner = seed_user(&conn).await;
    let workspace = app_row(&conn, app_id).await.project_id;
    let staging_pin = seed_staging_revision(&conn, workspace, "sha-staging").await;
    let sandbox_pin = seed_staging_revision(&conn, workspace, "sha-sandbox").await;
    let staging_build = seed_build(&conn, app_id, "staging").await;
    let pinned_build = seed_build(&conn, app_id, "pinned").await;
    let plain_build = seed_build(&conn, app_id, "plain").await;
    pin_build(&conn, staging_build, staging_pin).await;
    pin_build(&conn, pinned_build, sandbox_pin).await;
    envs::record_move(
        &conn,
        app_id,
        &AppEnvironment::Staging,
        Some(staging_build),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("staging");
    for (name, build) in [("dev-a1", pinned_build), ("dev-b2", plain_build)] {
        seed_sandbox(&conn, app_id, name, owner).await;
        envs::record_move(
            &conn,
            app_id,
            &AppEnvironment::parse(name).unwrap(),
            Some(build),
            EnvAction::Publish,
            None,
        )
        .await
        .expect("point the sandbox");
    }
    let staff = Uuid::new_v4();
    let pin = |environment: &'static str| {
        let conn = conn.clone();
        async move {
            pin_for(
                &conn,
                &data_request(app_id, environment),
                &oxy_app::server::authz::Caller::without_credential(staff, STAFF),
                workspace,
            )
            .await
        }
    };

    assert_eq!(
        pin("dev-a1").await,
        Some(sandbox_pin),
        "its own build's pin"
    );
    assert_eq!(
        pin("dev-b2").await,
        None,
        "a sandbox whose build pins nothing reads the promoted model, never staging's pin"
    );
    assert_eq!(pin("dev-nobody").await, None, "no such sandbox");
    assert_eq!(pin("staging").await, Some(staging_pin), "control: staging");
    assert_eq!(pin("production").await, None);

    let customer = pin_for(
        &conn,
        &data_request(app_id, "dev-a1"),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), "customer@example.com"),
        workspace,
    )
    .await;
    assert_eq!(customer, None, "a sandbox is Oxy staff's");
    let elsewhere = pin_for(
        &conn,
        &data_request(app_id, "dev-a1"),
        &oxy_app::server::authz::Caller::without_credential(staff, STAFF),
        Uuid::new_v4(),
    )
    .await;
    assert_eq!(elsewhere, None, "the app must be this workspace's");
}
