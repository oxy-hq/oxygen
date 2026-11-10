//! Run retention (`server::router::run_retention`) against a real schema: what goes,
//! what a live reference keeps, and that a backlog drains across passes.
//! Database-backed (`Schema::All`): the guards read tables from four domains.

use std::time::Duration;

use oxy_app::server::router::run_retention::{Pruned, prune};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

const DAYS: u32 = 90;

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

struct World {
    db: DatabaseConnection,
    workspace: Uuid,
}

impl World {
    async fn new() -> Self {
        let (db, _) = fresh_db(Schema::All).await;
        let workspace = Uuid::new_v4();
        exec(
            &db,
            "INSERT INTO workspaces (id, name) VALUES ($1, 'retention')",
            vec![workspace.into()],
        )
        .await;
        Self { db, workspace }
    }

    /// A run last touched `age_days` ago. Returns its id (a UUID, as every
    /// guard table that names a run stores one).
    async fn run(
        &self,
        age_days: i32,
        status: &str,
        thread: Option<Uuid>,
        parent: Option<&str>,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        exec(
            &self.db,
            "INSERT INTO agentic_runs \
               (id, question, created_at, updated_at, thread_id, parent_run_id, task_status, workspace_id) \
             VALUES ($1, 'q', now() - make_interval(days => $2), now() - make_interval(days => $2), \
                     $3, $4, $5, $6)",
            vec![
                id.clone().into(),
                age_days.into(),
                thread.into(),
                parent.map(str::to_string).into(),
                status.into(),
                self.workspace.into(),
            ],
        )
        .await;
        id
    }

    /// A chat thread for runs to belong to.
    async fn thread(&self) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO threads (id, title, input, output, source, \"references\") \
             VALUES ($1, 't', 'i', 'o', 's', '[]')",
            vec![id.into()],
        )
        .await;
        id
    }

    async fn old_done(&self) -> String {
        self.run(200, "done", None, None).await
    }

    async fn exists(&self, id: &str) -> bool {
        self.db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT 1 AS one FROM agentic_runs WHERE id = $1",
                [id.into()],
            ))
            .await
            .unwrap()
            .is_some()
    }

    async fn prune(&self) -> Pruned {
        prune(&self.db, DAYS, 200, Duration::from_secs(60))
            .await
            .unwrap()
    }
}

fn uuid(id: &str) -> Uuid {
    id.parse().unwrap()
}

#[tokio::test]
async fn finished_root_runs_past_the_window_go_with_their_children() {
    let w = World::new().await;
    let old = w.old_done().await;
    let failed = w.run(120, "failed", None, None).await;
    let child = w.run(200, "done", None, Some(&old)).await;
    let grandchild = w.run(200, "done", None, Some(&child)).await;
    let recent = w.run(10, "done", None, None).await;
    let running = w.run(200, "running", None, None).await;
    let unset = w.run(200, "done", None, None).await;
    exec(
        &w.db,
        "UPDATE agentic_runs SET task_status = NULL WHERE id = $1",
        vec![unset.clone().into()],
    )
    .await;

    let pruned = w.prune().await;

    assert_eq!(
        pruned,
        Pruned {
            roots: 2,
            runs: 4,
            more: false
        }
    );
    for gone in [&old, &failed, &child, &grandchild] {
        assert!(!w.exists(gone).await, "{gone} should be deleted");
    }
    for kept in [&recent, &running, &unset] {
        assert!(w.exists(kept).await, "{kept} should be kept");
    }
    assert_eq!(
        w.prune().await,
        Pruned::default(),
        "a second pass finds nothing"
    );
}

#[tokio::test]
async fn a_thread_goes_only_once_all_of_it_is_idle() {
    let w = World::new().await;
    let idle = w.thread().await;
    let idle_runs = [
        w.run(300, "done", Some(idle), None).await,
        w.run(100, "failed", Some(idle), None).await,
    ];
    let active = w.thread().await;
    let active_runs = [
        w.run(300, "done", Some(active), None).await,
        w.run(5, "done", Some(active), None).await,
    ];
    let live = w.thread().await;
    let live_runs = [
        w.run(300, "done", Some(live), None).await,
        w.run(300, "running", Some(live), None).await,
    ];

    let pruned = w.prune().await;

    assert_eq!(pruned.roots, 2, "{pruned:?}");
    for gone in &idle_runs {
        assert!(!w.exists(gone).await, "idle thread run {gone} should go");
    }
    for kept in active_runs.iter().chain(&live_runs) {
        assert!(w.exists(kept).await, "{kept} is in a thread still in use");
    }
}

#[tokio::test]
async fn a_run_something_live_still_reads_is_kept() {
    let w = World::new().await;
    let preview = w.old_done().await;
    exec(
        &w.db,
        "INSERT INTO workspace_preview_runs \
           (run_id, workspace_id, branch, preview_key, revision_id, kind, state) \
         VALUES ($1, $2, 'b', 'k', $3, 'analyze', 'finished')",
        vec![
            preview.clone().into(),
            w.workspace.into(),
            Uuid::new_v4().into(),
        ],
    )
    .await;

    let estimate = w.old_done().await;
    let experiment = Uuid::new_v4();
    exec(
        &w.db,
        "INSERT INTO experiment_runs \
           (id, workspace_id, registered_at, state, next_check_at, created_at, updated_at, \
            experiment_id, design_id, registered_by, estimate_task_id) \
         VALUES ($1, $2, now(), 'reported', now(), now(), now(), $3, $3, $3, $4)",
        vec![
            experiment.into(),
            w.workspace.into(),
            Uuid::new_v4().into(),
            uuid(&estimate).into(),
        ],
    )
    .await;

    let (dispatched, settled_flip) = (w.old_done().await, w.old_done().await);
    for (run, status) in [(&dispatched, "dispatched"), (&settled_flip, "applied")] {
        exec(
            &w.db,
            "INSERT INTO experiment_flips \
               (id, run_id, unit, target_state, due_at, wave, status, created_at, agentic_run_id) \
             VALUES ($1, $2, $3, 'on', now(), 1, $4, now(), $5)",
            vec![
                Uuid::new_v4().into(),
                experiment.into(),
                Uuid::new_v4().to_string().into(),
                status.into(),
                uuid(run).into(),
            ],
        )
        .await;
    }

    let (pending, settled_proposal) = (w.old_done().await, w.old_done().await);
    for (run, status) in [(&pending, "pending"), (&settled_proposal, "failed")] {
        exec(
            &w.db,
            "INSERT INTO experiment_proposals \
               (id, workspace_id, name, request_id, status, request, alpha, requested_by) \
             VALUES ($1, $2, 'p', $3, $4, '{}', 0.05, $3)",
            vec![
                uuid(run).into(),
                w.workspace.into(),
                Uuid::new_v4().into(),
                status.into(),
            ],
        )
        .await;
    }

    let (polled, settled_automation) = (w.old_done().await, w.old_done().await);
    for (run, status) in [(&polled, "running"), (&settled_automation, "completed")] {
        exec(
            &w.db,
            "INSERT INTO customer_app_automation_runs (id, workspace_id, procedure_id, status) \
             VALUES ($1, $2, 'proc', $3)",
            vec![uuid(run).into(), w.workspace.into(), status.into()],
        )
        .await;
    }

    let pruned = w.prune().await;

    assert_eq!(pruned.roots, 3, "{pruned:?}");
    for kept in [&preview, &estimate, &dispatched, &pending, &polled] {
        assert!(
            w.exists(kept).await,
            "{kept} is still read by something live"
        );
    }
    for gone in [&settled_flip, &settled_proposal, &settled_automation] {
        assert!(
            !w.exists(gone).await,
            "{gone} is referenced only by settled rows"
        );
    }
}

#[tokio::test]
async fn a_backlog_drains_across_passes() {
    let w = World::new().await;
    for _ in 0..5 {
        w.old_done().await;
    }

    let first = prune(&w.db, DAYS, 2, Duration::ZERO).await.unwrap();
    assert_eq!(
        first,
        Pruned {
            roots: 2,
            runs: 2,
            more: true
        },
        "a spent budget stops after one full batch"
    );

    let rest = prune(&w.db, DAYS, 2, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(
        rest,
        Pruned {
            roots: 3,
            runs: 3,
            more: false
        }
    );
}

/// A 0-day window is every finished run; the setting maps 0 to "off", and
/// `prune` refuses it too rather than empty the table.
#[tokio::test]
async fn a_zero_day_window_is_refused_and_deletes_nothing() {
    let w = World::new().await;
    let run = w.run(1, "done", None, None).await;
    assert!(prune(&w.db, 0, 200, Duration::from_secs(60)).await.is_err());
    assert!(w.exists(&run).await);
}
