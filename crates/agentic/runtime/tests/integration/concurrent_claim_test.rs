//! Concurrent claimers never share a row.
//!
//! The claim is one `UPDATE … WHERE task_id = (SELECT … FOR UPDATE SKIP
//! LOCKED) RETURNING *`. Read alone it is right; these tests are the proof
//! under contention — many claimers released at once against the same rows,
//! each row taken exactly once, by the claimer the row then names, with one
//! claim's worth of budget and epoch.
//!
//! Scoped (`claim_task_under_root`) on purpose: it is the claim every
//! per-run worker uses, and scoping to a fresh root keeps these rows out of any
//! other test's way on the shared database.
//!
//! Orchestrator hardening plan, Cleanup
//! (`internal-docs/2026-10-13-orchestrator-hardening-plan.md`).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(concurrent_claim_test)'

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::crud::queue::TaskScope;
use sea_orm::DatabaseConnection;
use tokio::sync::Barrier;

use crate::integration_tests::test_db;

const CLAIMERS: usize = 16;

/// A root run with `children` queued child tasks under it (plus the root's own
/// row). Returns the root id and every queued task id.
async fn seed_tree(db: &DatabaseConnection, children: usize) -> (String, Vec<String>) {
    let root = format!("cc-{}", uuid::Uuid::new_v4());
    crud::insert_run(db, &root, "Q", None, "analytics", None, uuid::Uuid::nil())
        .await
        .unwrap();
    let spec = TaskSpec::Agent {
        agent_id: "test-agent".into(),
        question: "q".into(),
        extra: None,
    };
    let mut ids = vec![root.clone()];
    crud::enqueue_task(db, &root, &root, None, &spec, None, TaskScope::Scoped)
        .await
        .unwrap();
    for i in 1..=children {
        let child = format!("{root}.{i}");
        crud::enqueue_task(
            db,
            &child,
            &root,
            Some(&root),
            &spec,
            None,
            TaskScope::Scoped,
        )
        .await
        .unwrap();
        ids.push(child);
    }
    (root, ids)
}

/// Release `CLAIMERS` workers at once; each claims under `root` until the queue
/// is empty. Returns every `(task_id, worker_id)` claim that landed.
async fn race(db: &DatabaseConnection, root: &str) -> Vec<(String, String)> {
    let barrier = Arc::new(Barrier::new(CLAIMERS));
    let mut workers = Vec::with_capacity(CLAIMERS);
    for n in 0..CLAIMERS {
        let (db, root, barrier) = (db.clone(), root.to_string(), barrier.clone());
        workers.push(tokio::spawn(async move {
            let worker_id = format!("claimer-{n}");
            barrier.wait().await;
            let mut won = vec![];
            while let Some(row) = crud::claim_task_under_root(&db, &worker_id, &root)
                .await
                .expect("a claim never errors under contention")
            {
                won.push((row.task_id, worker_id.clone()));
            }
            won
        }));
    }
    let mut claims = vec![];
    for worker in workers {
        claims.extend(worker.await.expect("claimer panicked"));
    }
    claims
}

#[tokio::test]
async fn concurrent_claimers_take_each_row_exactly_once() {
    let Some(db) = test_db().await else {
        return;
    };
    let (root, ids) = seed_tree(&db, 40).await;

    let claims = race(&db, &root).await;

    let mut by_task: HashMap<&str, Vec<&str>> = HashMap::new();
    for (task, worker) in &claims {
        by_task.entry(task).or_default().push(worker);
    }
    assert_eq!(
        claims.len(),
        ids.len(),
        "every row claimed once: {by_task:?}"
    );
    for id in &ids {
        let winners = by_task.get(id.as_str()).cloned().unwrap_or_default();
        assert_eq!(winners.len(), 1, "{id} claimed by {winners:?}");
        let row = crud::get_queue_entry(&db, id).await.unwrap().unwrap();
        assert_eq!(row.queue_status, "claimed");
        assert_eq!(row.worker_id.as_deref(), Some(winners[0]), "{id}");
        assert_eq!(row.claim_count, 1, "{id} spent one claim of budget");
        assert_eq!(row.claim_epoch, 1, "{id} carries one claim's epoch");
    }
}

#[tokio::test]
async fn concurrent_claimers_on_one_row_have_one_winner() {
    let Some(db) = test_db().await else {
        return;
    };
    let (root, _) = seed_tree(&db, 0).await;

    let claims = race(&db, &root).await;

    assert_eq!(claims.len(), 1, "exactly one winner: {claims:?}");
    let row = crud::get_queue_entry(&db, &root).await.unwrap().unwrap();
    assert_eq!(row.worker_id.as_deref(), Some(claims[0].1.as_str()));
    assert_eq!((row.claim_count, row.claim_epoch), (1, 1));
}
