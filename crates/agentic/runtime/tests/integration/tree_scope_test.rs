//! Every tree-scoped queue statement selects exactly its own tree.
//!
//! A task tree is a root and its `<root>.<n>…` descendants, keyed by
//! `agentic_task_queue.root_task_id`. Before that column, each statement
//! matched `task_id = root OR task_id LIKE root || '.%'`. Two neighbour shapes
//! that could leak are pinned here, each against a root that has its own
//! live rows:
//!
//! - **A shared prefix** (`…abc` vs `…abcd`). The `.` in the pattern already
//!   kept these apart, and still does.
//! - **A `_` in the root id** (`…a_c` vs `…aXc`). `_` is a LIKE wildcard, so
//!   the old pattern for `…a_c` also matched `…aXc.1`. Real root ids carry
//!   `_` (`custom_app_sandbox_oltp:…`), but no producer mints two ids that
//!   differ only where one has a `_`, so the leak was reachable in principle
//!   and not in practice.
//!
//! Orchestrator hardening plan, Cleanup
//! (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(tree_scope_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::crud::queue::TaskScope;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::integration_tests::test_db;

fn spec() -> TaskSpec {
    TaskSpec::Agent {
        agent_id: "test-agent".into(),
        question: "q".into(),
        extra: None,
    }
}

/// A root run in `workspace` with a queued root row and queued `.1` and
/// `.1.1` descendants (Global, so the recovery selections can see it).
async fn seed_tree(db: &DatabaseConnection, root: &str, workspace: Uuid) {
    crud::insert_run(db, root, "Q", None, "workflow", None, workspace)
        .await
        .unwrap();
    crud::update_task_status(db, root, "running", None)
        .await
        .unwrap();
    let (child, grandchild) = (format!("{root}.1"), format!("{root}.1.1"));
    for (task, parent) in [
        (root, None),
        (child.as_str(), Some(root)),
        (grandchild.as_str(), Some(child.as_str())),
    ] {
        crud::enqueue_task(db, task, root, parent, &spec(), None, TaskScope::Global)
            .await
            .unwrap();
    }
}

/// Two sibling roots, `a` and `b`, with live trees. `b` is the neighbour that
/// must never be selected through `a`.
async fn seed_pair(db: &DatabaseConnection, a: &str, b: &str, ws: Uuid) {
    seed_tree(db, a, ws).await;
    seed_tree(db, b, ws).await;
}

async fn statuses(db: &DatabaseConnection, root: &str) -> Vec<(String, String)> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT task_id, queue_status FROM agentic_task_queue \
          WHERE task_id = $1 OR left(task_id, length($1) + 1) = $1 || '.' \
          ORDER BY task_id",
        [root.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r.try_get("", "task_id").unwrap(),
            r.try_get("", "queue_status").unwrap(),
        )
    })
    .collect()
}

/// End `root`'s own rows one by one, so a neighbour's rows stay live
/// whatever the tree-scoped statements under test would have touched.
async fn end_own_rows(db: &DatabaseConnection, root: &str) {
    for task in [root.to_string(), format!("{root}.1"), format!("{root}.1.1")] {
        crud::cancel_queued_task(db, &task).await.unwrap();
    }
}

fn pairs(tag: &str) -> [(String, String); 2] {
    let p = format!("ts-{tag}-{}", Uuid::new_v4().simple());
    [
        (format!("{p}-abc"), format!("{p}-abcd")),
        (format!("{p}-a_c"), format!("{p}-aXc")),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_claim_takes_only_its_own_tree() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for (a, b) in pairs("claim") {
        seed_pair(&db, &a, &b, Uuid::new_v4()).await;
        let mut claimed = vec![];
        while let Some(row) = crud::claim_task_under_root(&db, "ts-worker", &a)
            .await
            .unwrap()
        {
            claimed.push(row.task_id);
        }
        claimed.sort();
        assert_eq!(
            claimed,
            vec![a.clone(), format!("{a}.1"), format!("{a}.1.1")]
        );
        assert!(
            statuses(&db, &b).await.iter().all(|(_, s)| s == "queued"),
            "a claim scoped to {a} took a row of {b}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_run_cancels_only_its_own_tree() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for (a, b) in pairs("cancel") {
        seed_pair(&db, &a, &b, Uuid::new_v4()).await;
        let cancelled = crud::cancel_queued_tasks_for_run(&db, &a).await.unwrap();
        assert_eq!(cancelled, 3, "{a}'s root, child and grandchild");
        assert!(
            statuses(&db, &b).await.iter().all(|(_, s)| s == "queued"),
            "cancelling {a} cancelled a row of {b}"
        );
    }
}

/// `complete_run_with_lost_outcome` refuses while the run's tree holds a live
/// row; a neighbour's live rows must not hold it.
#[tokio::test(flavor = "multi_thread")]
async fn a_neighbours_live_rows_do_not_hold_a_run_open() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for (a, b) in pairs("lost") {
        seed_pair(&db, &a, &b, Uuid::new_v4()).await;
        end_own_rows(&db, &a).await;
        let finished = crud::complete_run_with_lost_outcome(&db, &a, None)
            .await
            .unwrap();
        assert!(finished, "{b}'s live rows held {a} open");
    }
}

/// The global selection attributes each queued row to its own root only.
#[tokio::test(flavor = "multi_thread")]
async fn the_pending_global_selection_attributes_rows_to_their_own_root() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for (a, b) in pairs("pending") {
        let ws = Uuid::new_v4();
        seed_pair(&db, &a, &b, ws).await;
        end_own_rows(&db, &a).await;
        let pending: Vec<String> = crud::find_pending_global_runs(&db, Some(ws))
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.run_id)
            .collect();
        assert_eq!(pending, vec![b.clone()], "{a} has no queued row of its own");
    }
}

/// A dead letter fails its run unless the run's own tree is still live.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_letter_is_not_held_by_a_neighbours_tree() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    for (a, b) in pairs("dead") {
        seed_pair(&db, &a, &b, Uuid::new_v4()).await;
        end_own_rows(&db, &a).await;
        let outcome = crud::fail_dead_lettered_run(&db, &format!("{a}.1"), "dead")
            .await
            .unwrap();
        assert!(
            matches!(outcome, crud::DeadLetterRun::Failed { ref run_id } if *run_id == a),
            "{b}'s live tree held {a}'s dead letter: {outcome:?}"
        );
    }
}
