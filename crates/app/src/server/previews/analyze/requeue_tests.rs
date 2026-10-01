//! A failed check is not final for its revision: the next caller re-queues the
//! same run in place, exactly once however many ask, and never a check that is
//! done or that a worker still holds. Skips when `OXY_DATABASE_URL` is unset.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use uuid::Uuid;

use super::tests::{seed_revision, seed_workspace};
use super::*;
use crate::server::test_support::{SKIP_MSG, test_db};

async fn exec(db: &DatabaseConnection, sql: &str, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [run_id.into()],
    ))
    .await
    .expect(sql);
}

/// What the runtime leaves behind when the task's last attempt failed: the run
/// `failed` with its error, the task row `failed`, the analyze row `finished`.
async fn fail(db: &DatabaseConnection, run_id: &str) {
    agentic_runtime::crud::update_run_failed(db, run_id, "database went away")
        .await
        .unwrap();
    exec(
        db,
        "UPDATE agentic_task_queue SET queue_status = 'failed' WHERE task_id = $1",
        run_id,
    )
    .await;
    enqueue::mark_finished(db, run_id).await.unwrap();
}

/// `(analyze row state, run task_status, run error, task queue_status)`.
async fn states(
    db: &DatabaseConnection,
    run_id: &str,
) -> (String, Option<String>, Option<String>, String) {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT r.state, a.task_status, a.error_message, q.queue_status \
             FROM workspace_preview_runs r \
             JOIN agentic_runs a ON a.id = r.run_id \
             JOIN agentic_task_queue q ON q.task_id = r.run_id \
             WHERE r.run_id = $1",
            [run_id.into()],
        ))
        .await
        .unwrap()
        .expect("the check's rows");
    (
        row.try_get("", "state").unwrap(),
        row.try_get("", "task_status").unwrap(),
        row.try_get("", "error_message").unwrap(),
        row.try_get("", "queue_status").unwrap(),
    )
}

async fn analyze_rows(db: &DatabaseConnection, ws: Uuid) -> usize {
    entity::workspace_preview_runs::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.workspace_id == ws)
        .count()
}

#[tokio::test]
async fn a_failed_check_is_requeued_in_place_by_the_next_caller() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let rev = seed_revision(&db, ws, "staging").await;
    let run_id = ensure_enqueued(&db, ws, "feat/x", rev)
        .await
        .unwrap()
        .unwrap();
    fail(&db, &run_id).await;

    // Two refreshes race to ask again: one re-queues the same run.
    let (a, b) = tokio::join!(
        ensure_enqueued(&db, ws, "feat/x", rev),
        ensure_enqueued(&db, ws, "feat/x", rev),
    );
    let requeued: Vec<String> = [a, b].into_iter().filter_map(|r| r.unwrap()).collect();
    assert_eq!(requeued, vec![run_id.clone()], "the same run, once");
    assert_eq!(analyze_rows(&db, ws).await, 1, "no second check row");
    assert_eq!(
        states(&db, &run_id).await,
        (
            "queued".to_string(),
            Some("running".to_string()),
            None,
            "queued".to_string()
        ),
        "row queued, run running with its old error cleared, task claimable"
    );

    // Queued again, it is no longer a failure to re-queue.
    assert_eq!(ensure_enqueued(&db, ws, "feat/x", rev).await.unwrap(), None);
}

#[tokio::test]
async fn a_done_check_or_one_a_worker_holds_is_never_requeued() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;

    let done_rev = seed_revision(&db, ws, "staging").await;
    let done = ensure_enqueued(&db, ws, "feat/x", done_rev)
        .await
        .unwrap()
        .unwrap();
    agentic_runtime::crud::update_run_done(&db, &done, "ok", None)
        .await
        .unwrap();
    assert_eq!(
        ensure_enqueued(&db, ws, "feat/x", done_rev).await.unwrap(),
        None
    );

    // Failed on the run, but the task is still claimed: a worker is driving
    // it, and re-queueing would run the check twice.
    let held_rev = seed_revision(&db, ws, "staging").await;
    let held = ensure_enqueued(&db, ws, "feat/x", held_rev)
        .await
        .unwrap()
        .unwrap();
    agentic_runtime::crud::update_run_failed(&db, &held, "cancelled elsewhere")
        .await
        .unwrap();
    exec(
        &db,
        "UPDATE agentic_task_queue SET queue_status = 'claimed' WHERE task_id = $1",
        &held,
    )
    .await;
    assert_eq!(
        ensure_enqueued(&db, ws, "feat/x", held_rev).await.unwrap(),
        None
    );
    assert_eq!(states(&db, &held).await.1.as_deref(), Some("failed"));
}
