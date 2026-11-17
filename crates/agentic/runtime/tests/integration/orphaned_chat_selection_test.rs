//! `find_stuck_runs` selects an `analytics` root a dead `oxy serve` replica
//! was driving, and nothing a live process still drives.
//!
//! The fleet runs no `ide` pod, so the one-shot startup pass that used to
//! resume a chat orphaned by a serve replica's death runs nowhere. The
//! periodic stranded tick is the replacement: it runs on every worker, so it
//! must decide from the database alone — a lapsed driver lease, plus the
//! queue predicate — and never from "the process that died was me".
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(orphaned_chat_selection_test)'

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::stuck_run_sweeper_test::test_db;

const GRACE: u64 = 30;

/// A root of `source_type` in workspace `ws`, in `status`, last touched ten
/// minutes ago (past the grace), with no driver lease.
async fn seed_root(db: &DatabaseConnection, ws: Uuid, source_type: &str, status: &str) -> String {
    let run_id = format!("orphan-{source_type}-{}", Uuid::new_v4());
    crud::insert_run(db, &run_id, "Q", None, source_type, None, ws)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = $2, updated_at = now() - interval '10 minutes' \
         WHERE id = $1",
        [run_id.clone().into(), status.into()],
    ))
    .await
    .unwrap();
    run_id
}

/// The lease a driver that died (a killed serve replica) left behind: still
/// stamped, its last heartbeat well past `DRIVER_LEASE_TTL_SECS`.
async fn lapse_lease(db: &DatabaseConnection, run_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET driver_id = 'direct-dead-replica', \
             driver_heartbeat_at = now() - interval '10 minutes' \
         WHERE id = $1",
        [run_id.into()],
    ))
    .await
    .unwrap();
}

/// A delegation the root's coordinator enqueued under its own scope, as a
/// direct drive does (`TaskScope::Scoped`). Returns the child's task id.
///
/// Both halves, in production's order: a delegation is a child run row
/// (`insert_child_run`) and then the queue row that drives it
/// (`transport.assign` → `enqueue_task`). The queue's `run_id` foreign key
/// means the row cannot exist without the run, so seeding only the queue half
/// does not model anything the orchestrator can produce.
async fn seed_scoped_child(db: &DatabaseConnection, root: &str) -> String {
    let child = format!("{root}.1");
    crud::insert_child_run(db, &child, root, "Q", "analytics", 0, None)
        .await
        .unwrap();
    crud::enqueue_task(
        db,
        &child,
        &child,
        Some(root),
        &TaskSpec::Agent {
            agent_id: "agents/sub.agentic.yml".into(),
            question: "Q".into(),
            extra: None,
        },
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .unwrap();
    child
}

/// Age a queue row past the grace — what the reaper's re-queue or a graceful
/// shutdown's release of a dead process's claim looks like a grace later.
async fn age_queue_row(db: &DatabaseConnection, task_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET updated_at = now() - interval '10 minutes' \
         WHERE task_id = $1",
        [task_id.into()],
    ))
    .await
    .unwrap();
}

async fn selected(db: &DatabaseConnection, ws: Uuid, run_id: &str) -> bool {
    crud::find_stuck_runs(db, GRACE, Some(ws))
        .await
        .unwrap()
        .iter()
        .any(|r| r.run_id == run_id)
}

/// Every status a serve replica's death can leave a chat root in is selected
/// once its lease has lapsed: `running` / `delegating` /
/// `waiting_on_child(ren)` (a crash), `needs_resume` (an old boot's stamp)
/// and `shutdown` (a graceful stop, which released the lease).
///
/// The list is `find_stuck_runs`' own status filter, in full — a status the
/// selection grows must be added here, or nothing says a chat left in it is
/// recovered.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_replicas_chat_root_is_selected() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    for status in [
        "running",
        "delegating",
        "waiting_on_child",
        "waiting_on_children",
        "needs_resume",
        "shutdown",
    ] {
        let crashed = seed_root(&db, ws, "analytics", status).await;
        lapse_lease(&db, &crashed).await;
        assert!(
            selected(&db, ws, &crashed).await,
            "a `{status}` chat root with a lapsed lease must be selected"
        );
    }
    // Released at a graceful shutdown: no lease on record at all.
    let released = seed_root(&db, ws, "analytics", "shutdown").await;
    assert!(selected(&db, ws, &released).await);
}

/// The anti-poaching half: a chat a live peer drives holds a fresh lease, and
/// is never selected — whatever its status, from any process.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_peers_chat_root_is_not_selected() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    for status in ["running", "delegating", "needs_resume", "shutdown"] {
        let live = seed_root(&db, ws, "analytics", status).await;
        assert!(
            crud::try_acquire_driver(&db, &live, "direct-live-replica")
                .await
                .unwrap()
        );
        assert!(
            !selected(&db, ws, &live).await,
            "a `{status}` chat root whose driver lease is fresh must not be selected"
        );
    }
}

/// A `claimed` entry is a live claim until the reaper says otherwise — the
/// lease lapsing does not change that.
#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_entry_still_shields_a_chat_root() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    let root = seed_root(&db, ws, "analytics", "delegating").await;
    lapse_lease(&db, &root).await;
    let child = seed_scoped_child(&db, &root).await;
    age_queue_row(&db, &child).await;
    crud::claim_task_under_root(&db, "a-live-worker", &root)
        .await
        .unwrap()
        .expect("the scoped child is claimable");

    assert!(!selected(&db, ws, &root).await);
}

/// A dead direct drive's delegation sits `queued` + `scope_owned`, and only
/// the root's recovery will ever claim it. For a chat root with a lapsed lease
/// that row is not a live coordinator's, so once it is past the grace it does
/// not shield the root. Inside the grace it still does (a live coordinator's
/// enqueue→claim gap, behind the lease). `workflow` keeps its old rule.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_scoped_row_does_not_shield_a_chat_root() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    let root = seed_root(&db, ws, "analytics", "delegating").await;
    lapse_lease(&db, &root).await;
    let child = seed_scoped_child(&db, &root).await;
    assert!(
        !selected(&db, ws, &root).await,
        "a scoped row inside the grace still shields the root"
    );
    age_queue_row(&db, &child).await;
    assert!(
        selected(&db, ws, &root).await,
        "a stale scoped row under a lapsed lease must not strand the chat"
    );

    let workflow = seed_root(&db, ws, "workflow", "delegating").await;
    lapse_lease(&db, &workflow).await;
    let wf_child = seed_scoped_child(&db, &workflow).await;
    age_queue_row(&db, &wf_child).await;
    assert!(
        !selected(&db, ws, &workflow).await,
        "workflow / airway keep the scoped-row shield unchanged"
    );
}

/// A root parked on a person holds no lease by design, and its coordinator —
/// if it has one — may be live on another process. It is not this selection's:
/// an answer re-drives it from whichever replica takes it.
#[tokio::test(flavor = "multi_thread")]
async fn an_awaiting_input_root_is_never_selected() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    let parked = seed_root(&db, ws, "analytics", "awaiting_input").await;
    assert!(!selected(&db, ws, &parked).await);
    let lapsed = seed_root(&db, ws, "analytics", "awaiting_input").await;
    lapse_lease(&db, &lapsed).await;
    assert!(!selected(&db, ws, &lapsed).await);
}

/// The kinds this selection does not own stay out: `builder` (removed with
/// the IDE) and a child run, which is recovered with its root.
#[tokio::test(flavor = "multi_thread")]
async fn builder_roots_and_child_runs_are_not_selected() {
    let Some(db) = test_db().await else {
        return;
    };
    let ws = Uuid::new_v4();
    let builder = seed_root(&db, ws, "builder", "running").await;
    lapse_lease(&db, &builder).await;
    assert!(!selected(&db, ws, &builder).await);

    let root = seed_root(&db, ws, "analytics", "delegating").await;
    let child_run = format!("{root}.1");
    crud::insert_run_with_parent(&db, &child_run, &root, "Q", "analytics", None, 0, ws)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = 'running', \
             updated_at = now() - interval '10 minutes' WHERE id = $1",
        [child_run.clone().into()],
    ))
    .await
    .unwrap();
    assert!(!selected(&db, ws, &child_run).await);
}
