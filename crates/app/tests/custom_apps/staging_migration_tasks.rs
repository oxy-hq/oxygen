//! Drives the staging migration tasks a publish queues
//! (`custom_apps_nonproduction::staging_task`) through the executor production
//! registers for them — the worker fleet's half of a publish — and pins the
//! queue's half: one task per (app, build, store), queued without waiting.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use oxy_app::server::api::custom_apps_nonproduction::staging_task::{
    STAGING_MIGRATIONS_KIND, StagingMigrationTask, enqueue,
};
use oxy_app::server::api::custom_apps_nonproduction::staging_task_executor::StagingMigrationsExecutor;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// The app's staging migration tasks still queued, oldest first.
pub(crate) async fn queued(db: &DatabaseConnection, app_id: Uuid) -> Vec<(String, TaskSpec)> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT task_id, spec FROM agentic_task_queue \
             WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
               AND spec->'payload'->>'app_id' = $2 \
             ORDER BY created_at, task_id",
            [STAGING_MIGRATIONS_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| {
            let task_id: String = row.try_get("", "task_id").expect("task_id");
            let spec: serde_json::Value = row.try_get("", "spec").expect("spec");
            (task_id, serde_json::from_value(spec).expect("a TaskSpec"))
        })
        .collect()
}

/// Run one queued task through the registered executor, as a worker would,
/// and take it off the queue. Its outcome is what the run records.
pub(crate) async fn run_one(db: &DatabaseConnection, task_id: &str, spec: TaskSpec) -> TaskOutcome {
    let executor = StagingMigrationsExecutor { db: db.clone() };
    let mut running = executor
        .execute(TaskAssignment {
            task_id: task_id.to_string(),
            parent_task_id: None,
            run_id: task_id.to_string(),
            spec,
            policy: None,
        })
        .await
        .expect("the executor takes a staging task");
    let outcome = running.outcomes.recv().await.expect("an outcome");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'completed' WHERE task_id = $1",
        [task_id.into()],
    ))
    .await
    .expect("complete the task");
    outcome
}

/// Run every staging task queued for the app; the failures' messages.
pub(crate) async fn drain(db: &DatabaseConnection, app_id: Uuid) -> Vec<String> {
    let mut failures = Vec::new();
    for (task_id, spec) in queued(db, app_id).await {
        if let TaskOutcome::Failed(why) = run_one(db, &task_id, spec).await {
            failures.push(why);
        }
    }
    failures
}

/// The run a queued task is filed under: `(source_type, metadata)`.
pub(crate) async fn run_row(db: &DatabaseConnection, run_id: &str) -> (String, serde_json::Value) {
    let run = agentic_runtime::crud::get_run(db, run_id)
        .await
        .expect("read the run")
        .expect("the run exists");
    (
        run.source_type.unwrap_or_default(),
        run.metadata.unwrap_or_default(),
    )
}

/// Queueing the same (app, build, store) again is a no-op.
pub(crate) async fn assert_queued_once(db: &DatabaseConnection, spec: &TaskSpec) {
    let task = StagingMigrationTask::from_spec(spec).expect("a staging task");
    assert!(
        !enqueue(db, &task).await.expect("enqueue again"),
        "a second enqueue of {} queued it again",
        task.run_id()
    );
}
