//! `thread_has_live_run`: whether a thread may be deleted under its runs. A
//! run executing, or due to be picked up again, still writes its answer to
//! the thread and tells its asker through it; one waiting on the person's
//! answer, or ended, does not.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(thread_live_run_test)'

use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use super::eviction_safety_test::test_db;

/// A new thread row: a run's `thread_id` must name one.
async fn new_thread(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    let insert = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"INSERT INTO threads (id, title, input, output, source, "references")
           VALUES ($1, '', '', '', '', '[]')"#,
        [id.into()],
    );
    db.execute_raw(insert).await.unwrap();
    id
}

#[tokio::test]
async fn a_thread_is_live_while_a_run_on_it_executes_or_will_resume() {
    let db = test_db().await.expect("test db");
    let cases = [
        ("running", true),
        ("delegating", true),
        ("waiting_on_child", true),
        ("waiting_on_children", true),
        ("needs_resume", true),
        ("shutdown", true),
        ("awaiting_input", false),
        ("done", false),
        ("failed", false),
        ("cancelled", false),
        ("timed_out", false),
    ];
    let mut wrong = Vec::new();
    for (status, live) in cases {
        let thread = new_thread(&db).await;
        let run_id = Uuid::new_v4().to_string();
        crud::insert_run(
            &db,
            &run_id,
            "q",
            Some(thread),
            "analytics",
            None,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        crud::update_task_status(&db, &run_id, status, None)
            .await
            .unwrap();
        if crud::thread_has_live_run(&db, thread).await.unwrap() != live {
            wrong.push(status);
        }
    }
    assert!(wrong.is_empty(), "misjudged statuses: {wrong:?}");
}

/// One ended run does not hide a later one still going, and a live run on
/// another thread is not this thread's.
#[tokio::test]
async fn any_live_run_on_the_thread_counts_and_only_on_that_thread() {
    let db = test_db().await.expect("test db");
    let thread = new_thread(&db).await;
    let workspace = Uuid::new_v4();
    let ended = Uuid::new_v4().to_string();
    crud::insert_run(
        &db,
        &ended,
        "first",
        Some(thread),
        "analytics",
        None,
        workspace,
    )
    .await
    .unwrap();
    crud::update_task_status(&db, &ended, "done", None)
        .await
        .unwrap();
    assert!(!crud::thread_has_live_run(&db, thread).await.unwrap());

    let follow_up = Uuid::new_v4().to_string();
    crud::insert_run(
        &db,
        &follow_up,
        "second",
        Some(thread),
        "analytics",
        None,
        workspace,
    )
    .await
    .unwrap();
    assert!(crud::thread_has_live_run(&db, thread).await.unwrap());
    let other = new_thread(&db).await;
    assert!(!crud::thread_has_live_run(&db, other).await.unwrap());
}
