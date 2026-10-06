//! What a publish to a sandbox queues and keeps: its Airhouse migrations
//! under their own task kind, applied only while the sandbox still serves
//! that build, and no OLTP migration at all; and its builds, pruned in a
//! window of their own so they never push out the build production would
//! roll back to.

use entity::app_builds;
use oxy_app::server::api::custom_apps_publish::{PublishTarget, publish_to};
use oxy_app::server::api::custom_apps_sandboxes::lock::SandboxLock;
use oxy_app::server::api::custom_apps_sandboxes::migrations_task::{
    self, SANDBOX_MIGRATIONS_KIND, SandboxMigrationsExecutor, SandboxMigrationsTask,
};
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, ops};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{Value, json};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{publish_build, seeded_tenant};
use crate::sandbox_publish::{
    app_row, app_with_two_sandboxes, build_pk, bundle, count, input, noop, publish_to_sandbox,
    queued, sandbox,
};
use crate::staging_functions::make_guest_staff;

const AIRHOUSE_SQL: &[u8] = b"CREATE SCHEMA IF NOT EXISTS app_sbx_air;
CREATE TABLE app_sbx_air.visits (visit_id VARCHAR NOT NULL, store VARCHAR);";
const OLTP_SQL: &[u8] = b"CREATE TABLE notes (id uuid PRIMARY KEY);";

/// A bundle that declares both kinds of migrations, published to a sandbox:
/// the Airhouse files are queued under the sandbox's own task kind, once per
/// (app, sandbox, build); the OLTP files are named in a warning and applied
/// nowhere; and nothing is queued for staging.
#[tokio::test]
async fn a_sandbox_publish_queues_its_own_airhouse_migrations_and_applies_no_oltp() {
    let t = seeded_tenant().await;
    let slug = "sbx-air";
    let app = app_with_two_sandboxes(&t, slug).await;
    let staging_tasks = queued(&t.db, app.id, "custom_app_staging_migrations").await;
    let declares = json!({
        "airhouseMigrations": { "dir": "airhouse-migrations" },
        "migrations": { "dir": "migrations" },
    });
    let tarball = || {
        bundle(
            slug,
            &[noop("noop", json!({ "route": true }))],
            declares.clone(),
            &[
                ("airhouse-migrations/0001_visits.sql", AIRHOUSE_SQL),
                ("migrations/0001_notes.sql", OLTP_SQL),
            ],
        )
    };
    let result = publish_to(
        input(&t, slug, "air-1", tarball()),
        PublishTarget::Sandbox(sandbox("a1")),
    )
    .await
    .expect("publish to dev-a1");
    let warned = result
        .warnings
        .iter()
        .any(|w| w.starts_with("1 OLTP migration file(s) were not applied"));
    assert!(warned, "{:?}", result.warnings);

    let build = build_pk(&t.db, app.id, "air-1").await.expect("the build");
    let run_id = format!("{SANDBOX_MIGRATIONS_KIND}:{}:dev-a1:{build}", app.id);
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue WHERE task_id = $1 AND queue_status = 'queued'",
            [run_id.clone().into()],
        ))
        .await
        .expect("read the queue")
        .expect("the sandbox's migrations are queued under their own run id");
    let spec: Value = row.try_get("", "spec").expect("spec");
    let task = SandboxMigrationsTask::from_spec(&serde_json::from_value(spec).expect("a spec"))
        .expect("a sandbox migrations task");
    assert_eq!(task.environment, "dev-a1");
    assert_eq!(task.build_pk, build);
    let files: Vec<&str> = task
        .migrations
        .iter()
        .map(|m| m.filename.as_str())
        .collect();
    assert_eq!(files, vec!["0001_visits.sql"], "Airhouse files only");
    assert_eq!(queued(&t.db, app.id, SANDBOX_MIGRATIONS_KIND).await, 1);
    assert_eq!(
        queued(&t.db, app.id, "custom_app_staging_migrations").await,
        staging_tasks,
        "nothing is queued for staging's homes"
    );
    let ledger = "SELECT count(*) AS n FROM custom_app_migrations WHERE app_id::text = $1";
    assert_eq!(count(&t.db, ledger, app.id).await, 0, "no OLTP file ran");
    let (source_type, _) = crate::staging_migration_tasks::run_row(&t.db, &run_id).await;
    assert_eq!(source_type, SANDBOX_MIGRATIONS_KIND);

    use agentic_core::delegation::TaskOutcome;
    // While the sandbox serves this build the task applies it: here it gets
    // as far as Airhouse, which this test does not have.
    let outcome = run_migrations(&t.db, &run_id, &task).await;
    let TaskOutcome::Failed(why) = outcome else {
        panic!("the task must reach for Airhouse while the sandbox serves its build: {outcome:?}");
    };
    assert!(why.contains("was not migrated"), "{why}");

    // A teardown, or another apply, holds the sandbox: nothing is applied.
    let held = SandboxLock::try_acquire(&t.db, app.id, "dev-a1")
        .await
        .expect("the lock")
        .expect("nobody holds it");
    let busy = migrations_task::run_with(&t.db, &task, &[])
        .await
        .expect_err("the sandbox is locked");
    assert!(busy.contains("still running"), "{busy}");
    held.release().await;

    // A later publish moved the sandbox on — as would creating it again under
    // the name: this build's files are not the sandbox's any more.
    publish_to(
        input(&t, slug, "air-2", tarball()),
        PublishTarget::Sandbox(sandbox("a1")),
    )
    .await
    .expect("publish again to dev-a1");
    let outcome = run_migrations(&t.db, &run_id, &task).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("a build the sandbox no longer serves is skipped, not failed: {outcome:?}");
    };
    assert!(answer.contains("no longer serves this build"), "{answer}");
    assert_eq!(queued(&t.db, app.id, SANDBOX_MIGRATIONS_KIND).await, 2);

    // A sandbox deleted since the publish gets nothing applied: its sibling
    // is the teardown's to drop, not this task's to create again.
    ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let outcome = run_migrations(&t.db, &run_id, &task).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("a deleted sandbox's migrations are skipped, not failed: {outcome:?}");
    };
    assert!(answer.contains("was deleted since the publish"), "{answer}");
    assert_eq!(count(&t.db, ledger, app.id).await, 0);
}

/// Run a queued sandbox migration through the executor production registers.
async fn run_migrations(
    db: &DatabaseConnection,
    run_id: &str,
    task: &SandboxMigrationsTask,
) -> agentic_core::delegation::TaskOutcome {
    use agentic_runtime::worker::TaskExecutor;
    let executor = SandboxMigrationsExecutor { db: db.clone() };
    let mut running = executor
        .execute(agentic_core::delegation::TaskAssignment {
            task_id: run_id.to_string(),
            parent_task_id: None,
            run_id: run_id.to_string(),
            spec: task.spec().expect("a spec"),
            policy: None,
        })
        .await
        .expect("the executor takes a sandbox migrations task");
    running.outcomes.recv().await.expect("an outcome")
}

/// Sandbox builds are kept in a window of their own: ten publishes to a
/// sandbox leave the build production serves and the one it served before —
/// the rollback target — in place, where one shared window of ten would have
/// pruned the older. The eleventh prunes the oldest *sandbox* build.
#[tokio::test]
async fn sandbox_publishes_do_not_prune_the_builds_production_rolls_back_to() {
    let t = seeded_tenant().await;
    let slug = "sbx-keep";
    let route = || [noop("noop", json!({ "route": true }))];
    let app_id = publish_build(&t, slug, demo_workspace_id(), "prod-1", true, &route())
        .await
        .app_id;
    publish_build(&t, slug, demo_workspace_id(), "prod-2", true, &route()).await;
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("a1"), &t.guest())
        .await
        .expect("create dev-a1");

    for i in 1..=10 {
        publish_to_sandbox(&t, slug, &format!("sbx-{i}"), "a1")
            .await
            .expect("publish to dev-a1");
    }
    let labels = |builds: Vec<app_builds::Model>| -> Vec<String> {
        let mut labels: Vec<String> = builds.into_iter().map(|b| b.build_id).collect();
        labels.sort();
        labels
    };
    let builds = || async {
        app_builds::Entity::find()
            .filter(app_builds::Column::AppId.eq(app_id))
            .all(&t.db)
            .await
            .expect("read the builds")
    };
    let kept = labels(builds().await);
    assert_eq!(kept.len(), 12, "{kept:?}");
    assert!(
        kept.contains(&"prod-1".to_string()),
        "the rollback target: {kept:?}"
    );
    assert!(
        kept.contains(&"prod-2".to_string()),
        "production's build: {kept:?}"
    );

    publish_to_sandbox(&t, slug, "sbx-11", "a1")
        .await
        .expect("an eleventh publish to dev-a1");
    let kept = labels(builds().await);
    assert_eq!(kept.len(), 12, "{kept:?}");
    assert!(
        !kept.contains(&"sbx-1".to_string()),
        "the oldest sandbox build goes: {kept:?}"
    );
    assert!(kept.contains(&"prod-1".to_string()) && kept.contains(&"prod-2".to_string()));
    assert!(kept.contains(&"sbx-11".to_string()));
}

/// A build a live sandbox serves is never pruned, however far outside the
/// sandbox window it falls: `dev-b2` keeps its one build through eleven
/// publishes to `dev-a1`, while `dev-a1`'s own oldest build — in the same
/// window, and no longer served — goes. Once `dev-b2` moves on, the next
/// publish prunes its old build too: the pointer was what kept it.
#[tokio::test]
async fn a_build_a_live_sandbox_serves_survives_outside_the_sandbox_window() {
    let t = seeded_tenant().await;
    let slug = "sbx-live";
    let app = app_with_two_sandboxes(&t, slug).await;
    let labels = || async {
        let builds = entity::app_builds::Entity::find()
            .filter(entity::app_builds::Column::AppId.eq(app.id))
            .all(&t.db)
            .await
            .expect("read the builds");
        builds
            .into_iter()
            .map(|b| b.build_id)
            .collect::<Vec<String>>()
    };
    publish_to_sandbox(&t, slug, "b2-only", "b2")
        .await
        .expect("publish to dev-b2");
    for i in 1..=11 {
        publish_to_sandbox(&t, slug, &format!("a1-{i}"), "a1")
            .await
            .expect("publish to dev-a1");
    }
    // Twelve sandbox-only builds, a window of ten: `b2-only` and `a1-1` are
    // outside it.
    let kept = labels().await;
    assert!(
        kept.contains(&"b2-only".to_string()),
        "dev-b2 still serves it: {kept:?}"
    );
    assert!(
        !kept.contains(&"a1-1".to_string()),
        "nothing serves a1-1: {kept:?}"
    );
    assert!(kept.contains(&"a1-11".to_string()), "{kept:?}");

    publish_to_sandbox(&t, slug, "b2-next", "b2")
        .await
        .expect("dev-b2 moves on");
    let kept = labels().await;
    assert!(
        !kept.contains(&"b2-only".to_string()),
        "no longer served, and outside the window: {kept:?}"
    );
    assert!(kept.contains(&"b2-next".to_string()), "{kept:?}");
}
