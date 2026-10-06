//! The queued teardown of a sandbox (`custom_apps_sandboxes::teardown`), run
//! through the executor production registers: one sandbox's silo, secrets and
//! row go, and everything else stays; a failing step leaves it deleting; a
//! payload naming a fixed environment removes nothing. What happens when two
//! runs meet is `sandbox_teardown_races`'.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use entity::apps;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_environments::{self as envs, EnvAction};
use oxy_app::server::api::custom_apps_migrations::{
    AirhouseHome, apply_airhouse_over, declare_airhouse,
};
use oxy_app::server::api::custom_apps_sandboxes::teardown::{
    self, SANDBOX_TEARDOWN_KIND, SandboxTeardownExecutor, SandboxTeardownTask,
};
use oxy_app::server::api::custom_apps_sandboxes::{SandboxError, TeardownReason, ops};
use oxy_app::server::api::custom_apps_storage::{
    self as storage, PutOptions, RetentionPolicy, Silo,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_app, seeded_tenant};
use crate::sandbox_teardown::{AIRHOUSE_FILES, sandbox, standin};

/// A scratch state dir (the filesystem storage backend) and a fixed
/// encryption key (the secret store), set before anything reads either.
pub(crate) fn use_scratch_homes() -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!("oxy-sandbox-teardown-{}", Uuid::new_v4()));
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        std::env::set_var("OXY_STATE_DIR", &tmp);
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
    tmp
}

/// A published app, and the `app_builds.id` of its one build.
pub(crate) async fn published(t: &Tenant, slug: &str) -> (apps::Model, Uuid) {
    let noop = FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    };
    let app_id = publish_app(t, slug, demo_workspace_id(), &[noop])
        .await
        .app_id;
    let app = apps::Entity::find_by_id(app_id)
        .one(&t.db)
        .await
        .expect("read the app")
        .expect("the app");
    let build = entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(app_id))
        .one(&t.db)
        .await
        .expect("read the build")
        .expect("the publish recorded a build")
        .id;
    (app, build)
}

/// A sandbox serving `build`, holding one object and one secret.
pub(crate) async fn furnished_sandbox(t: &Tenant, app: &apps::Model, build: Uuid, handle: &str) {
    let environment = sandbox(handle);
    ops::create(&t.db, app, &environment, &t.guest())
        .await
        .expect("create the sandbox");
    envs::record_move(
        &t.db,
        app.id,
        &environment,
        Some(build),
        EnvAction::Publish,
        Some(t.guest_id),
    )
    .await
    .expect("publish to the sandbox");
    put_object(app.id, &environment, &format!("uploads/{handle}.bin")).await;
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(
            &t.db,
            app.id,
            Some(&environment.name()),
            "TOKEN",
            handle,
            t.guest_id,
        )
        .await
        .expect("set the sandbox's secret");
}

pub(crate) async fn put_object(app: Uuid, environment: &AppEnvironment, pathname: &str) {
    storage::put(
        &Silo::for_environment(app, environment),
        pathname,
        b"x".to_vec(),
        PutOptions::default(),
        &RetentionPolicy::default(),
    )
    .await
    .expect("put");
}

pub(crate) async fn objects(app: Uuid, environment: &AppEnvironment) -> usize {
    storage::list(&Silo::for_environment(app, environment), None, None, None)
        .await
        .expect("list")
        .objects
        .len()
}

pub(crate) async fn secret(app: &apps::Model, environment: &str) -> Option<String> {
    let manager = SecretManagerService::new(app.project_id);
    manager.clear_cache().await;
    manager
        .get_secret(&format!("apps/{}/{environment}/TOKEN", app.id))
        .await
}

/// Whether the sandbox's row is there, and marked deleting.
pub(crate) async fn row(db: &DatabaseConnection, app: Uuid, name: &str) -> Option<bool> {
    entity::app_environments::Entity::find_by_id((app, name.to_string()))
        .one(db)
        .await
        .expect("read the row")
        .map(|row| row.deleting_at.is_some())
}

async fn count(db: &DatabaseConnection, sql: &str, app: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [app.into()],
    ))
    .await
    .expect("count")
    .expect("a row")
    .try_get("", "n")
    .expect("n")
}

async fn invoke(db: &DatabaseConnection, app: Uuid, build: Uuid, environment: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_function_invocations \
           (id, app_id, build_id, function_name, mode, status, environment) \
         VALUES ($1, $2, $3, 'noop', 'route', 'success', $4)",
        [
            Uuid::new_v4().into(),
            app.into(),
            build.into(),
            environment.into(),
        ],
    ))
    .await
    .expect("record an invocation");
}

/// The queued task of `run_id`, as a worker claims it.
pub(crate) async fn queued(db: &DatabaseConnection, run_id: &str) -> TaskSpec {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue WHERE task_id = $1 AND queue_status = 'queued'",
            [run_id.into()],
        ))
        .await
        .expect("read the queue")
        .expect("the teardown is queued");
    let spec: serde_json::Value = row.try_get("", "spec").expect("spec");
    serde_json::from_value(spec).expect("a TaskSpec")
}

/// Run a queued teardown through the executor production registers.
pub(crate) async fn run_queued(db: &DatabaseConnection, run_id: &str) -> TaskOutcome {
    let spec = queued(db, run_id).await;
    let executor = SandboxTeardownExecutor { db: db.clone() };
    let mut running = executor
        .execute(TaskAssignment {
            task_id: run_id.to_string(),
            parent_task_id: None,
            run_id: run_id.to_string(),
            spec,
            policy: None,
        })
        .await
        .expect("the executor takes a teardown task");
    let outcome = running.outcomes.recv().await.expect("an outcome");
    // As the worker records it: the task is over, done or failed.
    let ended = match &outcome {
        TaskOutcome::Failed(_) => "failed",
        _ => "completed",
    };
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = $1 WHERE task_id = $2",
        [ended.into(), run_id.into()],
    ))
    .await
    .expect("end the task");
    outcome
}

/// Airhouse configured, and not there: nothing listens on port 9. A run
/// that reaches for a sibling fails; one that does not, passes.
pub(crate) fn point_airhouse_nowhere() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("AIRHOUSE_BASE_URL", "http://127.0.0.1:9");
        std::env::set_var("AIRHOUSE_ADMIN_TOKEN", "unused");
        std::env::set_var("AIRHOUSE_WIRE_HOST", "127.0.0.1");
        std::env::set_var("AIRHOUSE_WIRE_PORT", "9");
    }
}

/// The sandbox's sibling as a publish to it leaves it: migrated, with its
/// files in the ledger.
pub(crate) async fn migrated_sibling(
    t: &Tenant,
    app: &apps::Model,
    build: Uuid,
    handle: &str,
) -> AirhouseHome {
    let bundle: Vec<(String, Vec<u8>)> = AIRHOUSE_FILES
        .iter()
        .map(|(path, sql)| (path.to_string(), sql.as_bytes().to_vec()))
        .collect();
    let declared = declare_airhouse(
        Some(&json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } })),
        &bundle,
        &app.slug,
    )
    .expect("the files pass production's rules");
    let home = AirhouseHome::for_environment(&app.slug, &sandbox(handle))
        .expect("a valid slug")
        .expect("a sibling");
    apply_airhouse_over(&t.db, app.id, build, &declared, &home, &standin().await)
        .await
        .expect("apply to the sandbox's sibling");
    home
}

/// The app's teardown tasks still queued.
pub(crate) async fn teardowns_queued(db: &DatabaseConnection, app: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM agentic_task_queue \
         WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
           AND spec->'payload'->>'app_id' = $2",
        [SANDBOX_TEARDOWN_KIND.into(), app.to_string().into()],
    ))
    .await
    .expect("count")
    .expect("a row")
    .try_get("", "n")
    .expect("n")
}

/// Delete one of two sandboxes and run its queued teardown: its object,
/// its secret and its row go; the other sandbox, staging's secret,
/// production's object, and every invocation, event and audit row stay.
/// Until the task has run the name is taken; after it, free — and a stale
/// second run of the same teardown does not touch the sandbox created
/// under the name since.
#[tokio::test]
async fn the_queued_teardown_removes_one_sandboxs_homes_and_row_and_keeps_the_rest() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let (app, build) = published(&t, "sbx-teardown").await;
    furnished_sandbox(&t, &app, build, "a1").await;
    furnished_sandbox(&t, &app, build, "b2").await;
    put_object(app.id, &AppEnvironment::Production, "uploads/prod.bin").await;
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(&t.db, app.id, Some("staging"), "TOKEN", "stg", t.guest_id)
        .await
        .expect("set staging's secret");
    invoke(&t.db, app.id, build, "dev-a1").await;
    let invocations = "SELECT count(*) AS n FROM app_function_invocations \
                       WHERE app_id = $1 AND environment = 'dev-a1'";
    let events = "SELECT count(*) AS n FROM app_environment_events \
                  WHERE app_id = $1 AND environment = 'dev-a1'";
    let events_before = count(&t.db, events, app.id).await;
    assert!(events_before >= 1, "the publish to dev-a1 is on record");

    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    // Marked, not gone: the name stays taken until the task has run.
    assert_eq!(row(&t.db, app.id, "dev-a1").await, Some(true));
    assert_eq!(
        objects(app.id, &sandbox("a1")).await,
        1,
        "the task removes it"
    );
    assert_eq!(
        ops::create(&t.db, &app, &sandbox("a1"), &t.guest()).await,
        Err(SandboxError::Deleting("dev-a1".into()))
    );

    let outcome = run_queued(&t.db, &run_id).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the teardown failed: {outcome:?}");
    };
    assert!(answer.contains("dev-a1 torn down (deleted)"), "{answer}");
    assert!(answer.contains("1 secrets deleted"), "{answer}");

    assert_eq!(row(&t.db, app.id, "dev-a1").await, None, "the row is gone");
    assert_eq!(objects(app.id, &sandbox("a1")).await, 0);
    assert_eq!(secret(&app, "dev-a1").await, None);
    // The neighbour, staging and production are untouched.
    assert_eq!(row(&t.db, app.id, "dev-b2").await, Some(false));
    assert_eq!(objects(app.id, &sandbox("b2")).await, 1);
    assert_eq!(secret(&app, "dev-b2").await.as_deref(), Some("b2"));
    assert_eq!(secret(&app, "staging").await.as_deref(), Some("stg"));
    assert_eq!(objects(app.id, &AppEnvironment::Production).await, 1);
    // What ran in the sandbox stays on record.
    assert_eq!(count(&t.db, invocations, app.id).await, 1);
    assert_eq!(
        count(&t.db, events, app.id).await,
        events_before + 1,
        "the publish, and the unpublish the delete recorded"
    );
    let audit = "SELECT count(*) AS n FROM audit_events \
                 WHERE action LIKE 'app.environment.%' AND environment = 'dev-a1' \
                   AND target_id LIKE $1::text || '/%'";
    assert_eq!(count(&t.db, audit, app.id).await, 2, "created and deleted");

    // The name is free again; a stale second run of the old teardown must
    // not remove what the new sandbox holds.
    furnished_sandbox(&t, &app, build, "a1").await;
    let stale = SandboxTeardownTask::from_spec(&TaskSpec::Custom {
        kind: SANDBOX_TEARDOWN_KIND.into(),
        payload: json!({
            "app_id": app.id, "app_slug": app.slug, "org_id": app.org_id,
            "workspace_id": app.project_id, "environment": "dev-a1",
            "reason": "deleted", "marked_at_micros": 1,
        }),
    })
    .expect("a teardown payload");
    let skipped = teardown::run(&t.db, &stale).await.expect("a stale run");
    assert!(skipped.contains("nothing was removed"), "{skipped}");
    assert_eq!(row(&t.db, app.id, "dev-a1").await, Some(false));
    assert_eq!(objects(app.id, &sandbox("a1")).await, 1);
    assert_eq!(secret(&app, "dev-a1").await.as_deref(), Some("a1"));
    let _ = std::fs::remove_dir_all(tmp);
}

/// A payload naming a fixed environment is refused before any home is
/// touched: staging's secret survives a teardown task that names staging.
#[tokio::test]
async fn a_teardown_that_names_staging_removes_nothing() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let (app, _build) = published(&t, "sbx-fixed").await;
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(&t.db, app.id, Some("staging"), "TOKEN", "stg", t.guest_id)
        .await
        .expect("set staging's secret");
    let task = SandboxTeardownTask {
        app_id: app.id,
        app_slug: app.slug.clone(),
        org_id: app.org_id,
        workspace_id: app.project_id,
        environment: "staging".into(),
        reason: "deleted".into(),
        marked_at_micros: 1,
    };
    let refused = teardown::run(&t.db, &task).await.expect_err("staging");
    assert!(refused.contains("is not a sandbox"), "{refused}");
    assert_eq!(secret(&app, "staging").await.as_deref(), Some("stg"));
    assert_eq!(row(&t.db, app.id, "staging").await, Some(false));
    let _ = std::fs::remove_dir_all(tmp);
}
