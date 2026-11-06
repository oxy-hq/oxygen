//! `m20261105_000001_add_task_queue_root_task_id`: the backfill derives each
//! existing row's true root, sends no wakes, and leaves the column `NOT NULL`;
//! the insert trigger then sets it on every insert path.
//!
//! Runs on a scratch database of its own in the test server: the migration is
//! applied part way, rows are seeded the way the previous deploy wrote them,
//! and then the rest is applied. The shared test database is never stepped
//! back.
//!
//! Orchestrator hardening plan, Cleanup
//! (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(root_task_id_migration_test)'

use std::time::Duration;

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::crud::queue::TaskScope;
use agentic_runtime::migration::RuntimeMigrator;
use futures::StreamExt;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::integration_tests::{test_db, test_db_url};

const MIGRATION: &str = "m20261105_000001_add_task_queue_root_task_id";

fn spec() -> TaskSpec {
    TaskSpec::Agent {
        agent_id: "test-agent".into(),
        question: "q".into(),
        extra: None,
    }
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql).await.unwrap();
}

/// A fresh database next to the shared one; returns its name and URL.
async fn scratch_db(admin: &DatabaseConnection, url: &str) -> (String, String) {
    let name = format!("rtq_{}", Uuid::new_v4().simple());
    exec(admin, &format!("CREATE DATABASE \"{name}\"")).await;
    let base = url.rsplit_once('/').expect("a database URL has a path").0;
    (name.clone(), format!("{base}/{name}"))
}

/// Every payload sent on the queue's channel, as it arrives.
async fn listen(url: &str) -> (tokio_postgres::Client, mpsc::UnboundedReceiver<String>) {
    let (client, mut conn) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut messages = futures::stream::poll_fn(move |cx| conn.poll_message(cx));
        while let Some(Ok(message)) = messages.next().await {
            if let tokio_postgres::AsyncMessage::Notification(n) = message {
                let _ = tx.send(n.payload().to_string());
            }
        }
    });
    client
        .batch_execute("LISTEN oxy_task_enqueued")
        .await
        .unwrap();
    (client, rx)
}

async fn root_of(db: &DatabaseConnection, task_id: &str) -> String {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT root_task_id FROM agentic_task_queue WHERE task_id = $1",
        [task_id.into()],
    ))
    .await
    .unwrap()
    .unwrap_or_else(|| panic!("no queue row {task_id}"))
    .try_get("", "root_task_id")
    .unwrap()
}

async fn enqueue(db: &DatabaseConnection, task: &str, run: &str, parent: Option<&str>) {
    crud::enqueue_task(db, task, run, parent, &spec(), None, TaskScope::Scoped)
        .await
        .unwrap();
}

async fn child_run(db: &DatabaseConnection, child: &str, parent: &str) {
    crud::insert_child_run(db, child, parent, "q", "analytics", 0, None)
        .await
        .unwrap();
}

/// The trees, as `(task_id, expected root)`, seeded under prefix `p`:
///
/// - a root, a delegated child and grandchild (each on its own child run, as
///   the coordinator writes them), and a child queued on the root's run (as
///   tests and the fan-out's virtual rows write them);
/// - a non-root re-assigned with `parent_task_id: None` (the coordinator's
///   resume of a delegating node);
/// - a root whose id contains `.`, which `split_part` would get wrong;
/// - a row whose run chain names no ancestor of it, which falls back to the
///   first `.` segment.
async fn seed(db: &DatabaseConnection, p: &str) -> Vec<(String, String)> {
    let (root, dotted) = (format!("{p}-root"), format!("{p}-v1.2"));
    for run in [&root, &dotted] {
        crud::insert_run(db, run, "Q", None, "analytics", None, Uuid::nil())
            .await
            .unwrap();
    }
    let (c1, c11, c2, c3) = (
        format!("{root}.1"),
        format!("{root}.1.1"),
        format!("{root}.2"),
        format!("{root}.3"),
    );
    let d1 = format!("{dotted}.1");
    child_run(db, &c1, &root).await;
    child_run(db, &c11, &c1).await;
    child_run(db, &c3, &root).await;
    child_run(db, &d1, &dotted).await;

    enqueue(db, &root, &root, None).await;
    enqueue(db, &c1, &c1, Some(&root)).await;
    enqueue(db, &c11, &c11, Some(&c1)).await;
    enqueue(db, &c2, &root, Some(&root)).await;
    enqueue(db, &c3, &c3, None).await;
    enqueue(db, &dotted, &dotted, None).await;
    enqueue(db, &d1, &d1, Some(&dotted)).await;
    let stray = format!("{p}-stray.4");
    enqueue(db, &stray, &root, None).await;

    let mut want: Vec<(String, String)> = [&root, &c1, &c11, &c2, &c3]
        .into_iter()
        .map(|t| (t.clone(), root.clone()))
        .collect();
    want.push((dotted.clone(), dotted.clone()));
    want.push((d1, dotted));
    want.push((stray, format!("{p}-stray")));
    want
}

async fn assert_schema(db: &DatabaseConnection) {
    let nullable: String = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT is_nullable FROM information_schema.columns \
              WHERE table_name = 'agentic_task_queue' AND column_name = 'root_task_id'",
        ))
        .await
        .unwrap()
        .expect("the column exists")
        .try_get("", "is_nullable")
        .unwrap();
    assert_eq!(
        nullable, "NO",
        "the backfill is complete, so the column is NOT NULL"
    );
    for index in ["idx_task_queue_root_task_id", "idx_task_queue_run_id"] {
        let found = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT 1 AS n FROM pg_indexes WHERE indexname = $1",
                [index.into()],
            ))
            .await
            .unwrap();
        assert!(found.is_some(), "{index} exists");
    }
}

async fn recv_within(rx: &mut mpsc::UnboundedReceiver<String>, d: Duration) -> Option<String> {
    tokio::time::timeout(d, rx.recv()).await.ok().flatten()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_backfill_derives_each_rows_root_without_waking_anyone() {
    let (Some(admin), Some(url)) = (test_db().await, test_db_url().await) else {
        eprintln!("skipping: no DB available");
        return;
    };
    let (name, scratch_url) = scratch_db(&admin, &url).await;
    let db = Database::connect(&scratch_url).await.unwrap();
    let before = RuntimeMigrator::migrations()
        .iter()
        .position(|m| m.name() == MIGRATION)
        .expect("the migration is registered");
    RuntimeMigrator::up(&db, Some(before as u32)).await.unwrap();

    let p = format!("mt-{}", Uuid::new_v4().simple());
    let want = seed(&db, &p).await;
    let (_client, mut wakes) = listen(&scratch_url).await;
    while recv_within(&mut wakes, Duration::from_millis(300))
        .await
        .is_some()
    {}

    RuntimeMigrator::up(&db, None).await.unwrap();

    assert_eq!(
        recv_within(&mut wakes, Duration::from_millis(500)).await,
        None,
        "the backfill UPDATE changes no queue_status, so it must not wake a poller"
    );
    for (task, root) in &want {
        assert_eq!(&root_of(&db, task).await, root, "root of {task}");
    }
    assert_schema(&db).await;
    the_trigger_sets_it_on_every_insert_path(&db, &p, &mut wakes).await;

    drop(db);
    exec(&admin, &format!("DROP DATABASE \"{name}\" WITH (FORCE)")).await;
}

/// After the migration: `enqueue_task`'s upsert (fresh and conflicting) and
/// `requeue_task`'s raw INSERT all get the true root without naming it, and a
/// fresh enqueue still wakes (the listener is live).
async fn the_trigger_sets_it_on_every_insert_path(
    db: &DatabaseConnection,
    p: &str,
    wakes: &mut mpsc::UnboundedReceiver<String>,
) {
    let (root, dotted) = (format!("{p}-root"), format!("{p}-v1.2"));
    let (c4, c5, d2) = (
        format!("{root}.4"),
        format!("{root}.5"),
        format!("{dotted}.2"),
    );
    child_run(db, &c4, &root).await;
    child_run(db, &c5, &root).await;
    child_run(db, &d2, &dotted).await;

    enqueue(db, &c4, &c4, Some(&root)).await;
    assert_eq!(root_of(db, &c4).await, root, "enqueue_task, a fresh child");
    assert!(
        recv_within(wakes, Duration::from_secs(5)).await.is_some(),
        "a fresh enqueue still wakes"
    );
    enqueue(db, &d2, &d2, Some(&dotted)).await;
    assert_eq!(
        root_of(db, &d2).await,
        dotted,
        "enqueue_task under a dotted root"
    );
    enqueue(db, &c4, &c4, None).await;
    assert_eq!(
        root_of(db, &c4).await,
        root,
        "the upsert's conflict path keeps it"
    );
    crud::requeue_task(db, &c5, &spec()).await.unwrap();
    assert_eq!(root_of(db, &c5).await, root, "requeue_task's raw INSERT");
}
