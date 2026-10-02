//! A legacy slug holding `--` derives a production schema with a sibling's
//! shape: app `a`'s sandbox `dev-b` and an app `a--dev-b` both name
//! `app_a__dev_b` (reviews m4, n4). That schema is the legacy app's
//! production data, so no environment of the other app is ever given it: a
//! sandbox of that name cannot be created, nothing is dropped there, and an
//! apply that cannot run **fails and names the collision** — it never ends
//! "nothing to apply". The apply's guard is `apply_airhouse_to_environment`'s,
//! so it covers staging's sibling as it covers a sandbox's.

use entity::app_builds;
use oxy_app::server::api::custom_apps_migrations::{
    AirhouseDrop, AirhouseHome, apply_airhouse_over, declare_airhouse, drop_environment_schema_over,
};
use oxy_app::server::api::custom_apps_nonproduction::staging_task::{
    StagingBuild, StagingMigrationTask, StagingStore,
};
use oxy_app::server::api::custom_apps_nonproduction::staging_task_executor;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter, Statement};
use serde_json::json;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, publish_app, seeded_tenant};
use crate::sandbox_teardown::{AIRHOUSE_APP, AIRHOUSE_FILES, relations, sandbox, standin};

/// App `store-ops` beside a legacy app `store-ops--staging`: both names give
/// `app_store_ops__staging`. Staging's migration task for `store-ops` fails
/// with the collision named, and records nothing.
#[tokio::test]
async fn a_staging_apply_into_another_apps_own_schema_fails_and_names_it() {
    const LEGACY: &str = "store-ops--staging";
    let t = seeded_tenant().await;
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app_id = publish_app(&t, AIRHOUSE_APP, demo_workspace_id(), &[noop])
        .await
        .app_id;
    let build_pk = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("the build")
        .id;
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO apps (id, slug, name, org_id, project_id, branch, source_repo, status, \
                           source_type, source_config, created_at, updated_at) \
         SELECT $1, $2, name, org_id, project_id, branch, source_repo, status, \
                source_type, source_config, now(), now() FROM apps WHERE id = $3",
        [uuid::Uuid::new_v4().into(), LEGACY.into(), app_id.into()],
    ))
    .await
    .expect("seed the legacy app");

    let bundle: Vec<(String, Vec<u8>)> = AIRHOUSE_FILES
        .iter()
        .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
        .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        AIRHOUSE_APP,
    )
    .expect("the files pass production's rules");
    let build = StagingBuild {
        app_id,
        app_slug: AIRHOUSE_APP,
        workspace_id: demo_workspace_id(),
        org_id: t.org_id,
        build_pk,
    };
    let task = StagingMigrationTask::new(StagingStore::Airhouse, &build, &declared);

    let refused = staging_task_executor::run(&t.db, &task)
        .await
        .expect_err("an apply that cannot run must not end as done");
    assert!(
        refused.contains(LEGACY) && refused.contains("app_store_ops__staging"),
        "the failure names the app and the schema: {refused}"
    );
    let rows = entity::custom_app_migrations::Entity::find()
        .filter(entity::custom_app_migrations::Column::AppId.eq(app_id))
        .all(&t.db)
        .await
        .expect("read the ledger");
    assert!(rows.is_empty(), "nothing was recorded as applied: {rows:?}");
}

/// A legacy slug holding `--` derives a production schema with a sibling's
/// shape: `store-ops--dev-a1` and the sandbox `dev-a1` of `store-ops` are both
/// `app_store_ops__dev_a1` (review m4). That schema is the legacy app's
/// production data, so the sandbox is never given it: the name cannot be
/// created; a sandbox row that predates the rule has no migration applied
/// there; and its teardown drops nothing — neither a drop aimed straight at
/// it nor the queued task, though the ledger says it has a sibling.
#[tokio::test]
async fn a_legacy_double_hyphen_slugs_schema_is_never_a_sandboxs_sibling() {
    use oxy_app::server::api::custom_apps_environments::{self as envs, EnvAction};
    use oxy_app::server::api::custom_apps_nonproduction::staging_task::QueuedMigration;
    use oxy_app::server::api::custom_apps_sandboxes::migrations_task::{
        self, SandboxMigrationsTask,
    };
    use oxy_app::server::api::custom_apps_sandboxes::teardown::{self, SandboxTeardownTask};
    use oxy_app::server::api::custom_apps_sandboxes::{SandboxError, TeardownReason, ops};
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    const LEGACY: &str = "store-ops--dev-a1";
    const SHARED_NAME: &str = "app_store_ops__dev_a1";

    // Airhouse configured, and not there: reaching for it fails.
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("AIRHOUSE_BASE_URL", "http://127.0.0.1:9");
        std::env::set_var("AIRHOUSE_ADMIN_TOKEN", "unused");
        std::env::set_var("AIRHOUSE_WIRE_HOST", "127.0.0.1");
        std::env::set_var("AIRHOUSE_WIRE_PORT", "9");
    }
    let t = seeded_tenant().await;
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app_id = publish_app(&t, AIRHOUSE_APP, demo_workspace_id(), &[noop])
        .await
        .app_id;
    let app = entity::apps::Entity::find_by_id(app_id)
        .one(&t.db)
        .await
        .expect("read the app")
        .expect("the app");
    let build = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("the build")
        .id;
    // An app whose slug predates the no-`--` rule, in the same workspace.
    let legacy_id = uuid::Uuid::new_v4();
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO apps (id, slug, name, org_id, project_id, branch, source_repo, status, \
                           source_type, source_config, created_at, updated_at) \
         SELECT $1, $2, name, org_id, project_id, branch, source_repo, status, \
                source_type, source_config, now(), now() FROM apps WHERE id = $3",
        [legacy_id.into(), LEGACY.into(), app_id.into()],
    ))
    .await
    .expect("seed the legacy app");
    let sibling = AirhouseHome::for_environment(AIRHOUSE_APP, &sandbox("a1"))
        .expect("a valid slug")
        .expect("a sibling");
    let legacy_own = AirhouseHome::production(LEGACY).expect("a legacy slug names a schema");
    assert_eq!(sibling.schema(), SHARED_NAME);
    assert_eq!(legacy_own.schema(), SHARED_NAME, "the two names collide");

    // The name cannot be created; a handle that collides with nothing can.
    assert_eq!(
        ops::create(&t.db, &app, &sandbox("a1"), t.guest_id).await,
        Err(SandboxError::Reserved {
            name: "dev-a1".into(),
            app: LEGACY.into()
        })
    );
    ops::create(&t.db, &app, &sandbox("b2"), t.guest_id)
        .await
        .expect("dev-b2 collides with nothing");

    // A row that predates the rule, serving a build — and the legacy app's
    // table in the schema both names give.
    crate::app_environments::seed_sandbox(&t.db, app_id, "dev-a1", t.guest_id).await;
    envs::record_move(
        &t.db,
        app_id,
        &sandbox("a1"),
        Some(build),
        EnvAction::Publish,
        Some(t.guest_id),
    )
    .await
    .expect("point the sandbox");
    let client = standin().await;
    client
        .batch_execute(&format!(
            "CREATE SCHEMA {SHARED_NAME}; CREATE TABLE {SHARED_NAME}.orders (id INT)"
        ))
        .await
        .expect("the legacy app's table");

    // Its migrations are not applied, and the run says why — it does not end
    // "nothing to apply". Nothing reaches for Airhouse.
    let task = SandboxMigrationsTask {
        app_id,
        app_slug: AIRHOUSE_APP.into(),
        workspace_id: demo_workspace_id(),
        build_pk: build,
        environment: "dev-a1".into(),
        migrations: AIRHOUSE_FILES
            .iter()
            .map(|(path, sql)| QueuedMigration {
                filename: path.rsplit('/').next().expect("a name").to_string(),
                checksum: "unchecked".into(),
                sql: sql.to_string(),
            })
            .collect(),
    };
    let refused = migrations_task::run(&t.db, &task)
        .await
        .expect_err("an apply that cannot run must not end as done");
    assert!(
        refused.contains(LEGACY) && refused.contains(SHARED_NAME),
        "the failure names the app and the schema: {refused}"
    );

    // A drop aimed straight at the name is refused; the table is still there.
    let drop = AirhouseDrop {
        app_id,
        app_slug: AIRHOUSE_APP,
        workspace_id: demo_workspace_id(),
    };
    let refused = drop_environment_schema_over(&t.db, drop, &sandbox("a1"), &client)
        .await
        .expect_err("another app's own schema is never dropped");
    assert!(
        refused.to_string().contains("refusing to drop"),
        "{refused}"
    );
    assert_eq!(relations(&client, SHARED_NAME).await, vec!["orders"]);

    // Nor is that schema a sibling of the legacy app's own sandboxes: a slug
    // holding the separator names no sibling at all, so nothing of it drops.
    let legacy_drop = AirhouseDrop {
        app_id: legacy_id,
        app_slug: LEGACY,
        workspace_id: demo_workspace_id(),
    };
    let refused = drop_environment_schema_over(&t.db, legacy_drop, &sandbox("x"), &client)
        .await
        .expect_err("a legacy app has no sibling to drop");
    assert!(
        refused.to_string().contains("refusing to drop"),
        "{refused}"
    );
    assert_eq!(relations(&client, SHARED_NAME).await, vec!["orders"]);

    // The queued teardown of that row finishes without reaching for Airhouse,
    // though ledger rows say the sandbox has a sibling there.
    let bundle: Vec<(String, Vec<u8>)> = AIRHOUSE_FILES
        .iter()
        .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
        .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        AIRHOUSE_APP,
    )
    .expect("the files pass production's rules");
    apply_airhouse_over(&t.db, app_id, build, &declared, &sibling, &client)
        .await
        .expect("ledger rows under the shared name");
    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(t.guest_id),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
            [run_id.into()],
        ))
        .await
        .expect("read the queue")
        .expect("the teardown is queued");
    let spec: serde_json::Value = row.try_get("", "spec").expect("spec");
    let teardown_task =
        SandboxTeardownTask::from_spec(&serde_json::from_value(spec).expect("a TaskSpec"))
            .expect("a teardown payload");
    let done = teardown::run(&t.db, &teardown_task)
        .await
        .expect("torn down without Airhouse");
    assert!(done.contains("no Airhouse sibling"), "{done}");
    assert_eq!(
        relations(&client, SHARED_NAME).await,
        vec!["latest", "orders", "visits"],
        "nothing in the legacy app's schema was dropped"
    );
}
