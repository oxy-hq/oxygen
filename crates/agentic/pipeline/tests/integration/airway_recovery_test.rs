//! Restart recovery of an airway run whose load had already finished.
//!
//! The shape from prod (BMG `amazon_vc`, 3 of 10 `failed` runs): every table
//! loaded, `load_completed` emitted, the worker's queue row stamped
//! `completed` — and then the process died before the coordinator wrote
//! `task_status = done`. Recovery found nothing to resume and retired the run
//! as `failed`; its events ended `load_completed` → `recovery_resumed`.
//!
//! These drive the real `recover_active_runs` over a run seeded through the
//! real submit path, then pin the result on the run row. Requires Docker (or
//! `OXY_DATABASE_URL`); self-skips otherwise.

use std::sync::Arc;

use agentic_airway::extension::pipeline_lease;
use agentic_pipeline::airway_run::{StartAirwayRequest, start_airway_run};
use agentic_runtime::crud;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};

use super::airway_run_test::{TmpWorkspace, test_db};

/// A seeded airway run, stranded the way a restart leaves it.
struct Stranded {
    db: DatabaseConnection,
    run_id: String,
    workspace_id: uuid::Uuid,
    pipeline_name: String,
    _dir: tempfile::TempDir,
}

/// Seed a run through `start_airway_run`, leave it `running` with no driver,
/// set its queue row to `queue_status`, take its single-flight lease as the
/// executor would, and write `events` as the coordinator would have.
async fn strand(queue_status: &str, events: &[(&str, Value)]) -> Option<Stranded> {
    let db = test_db().await?;
    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline_name = format!("recov_{}", uuid::Uuid::new_v4().simple());
    let yaml = format!(
        "name: {pipeline_name}\nsource:\n  kind: filesystem\n  config:\n    \
         base_path: /tmp/airway-recovery\n    pattern: \"*.jsonl\"\n    format: jsonl\n    \
         table_name: users\ndestination:\n  kind: memory\n  config:\n    \
         dataset_name: scratch\nresources:\n  - users\n"
    );
    std::fs::write(dir.path().join("p.airway.yml"), yaml).expect("write spec");
    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let workspace_id = uuid::Uuid::new_v4();
    let run_id = start_airway_run(
        &db,
        &ws,
        StartAirwayRequest {
            pipeline_ref: "p.airway.yml".to_string(),
            variables: None,
            thread_id: None,
            resources: Vec::new(),
            schedule_id: None,
            trigger: Some("test".to_string()),
            logical_date: None,
            retry_of: None,
            backfill_from: None,
            backfill_to: None,
        },
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    .expect("seed airway run");

    exec(
        &db,
        "UPDATE agentic_runs SET task_status = 'running', driver_id = NULL, \
         driver_heartbeat_at = NULL WHERE id = $1",
        vec![run_id.clone().into()],
    )
    .await;
    exec(
        &db,
        "UPDATE agentic_task_queue SET queue_status = $2 WHERE run_id = $1",
        vec![run_id.clone().into(), queue_status.into()],
    )
    .await;
    pipeline_lease::try_acquire(
        &db,
        workspace_id,
        &pipeline_name,
        &run_id,
        pipeline_lease::LEASE_TTL_SECS,
    )
    .await
    .expect("acquire the lease as the executor would at claim time");
    for (seq, (event_type, payload)) in events.iter().enumerate() {
        crud::insert_event(&db, &run_id, seq as i64, event_type, payload, 0)
            .await
            .expect("insert event");
    }
    Some(Stranded {
        db,
        run_id,
        workspace_id,
        pipeline_name,
        _dir: dir,
    })
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect("exec");
}

async fn recover(s: &Stranded) {
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(TmpWorkspace {
        root: s._dir.path().to_path_buf(),
        compiled: None,
    });
    agentic_pipeline::recovery::recover_active_runs(
        s.db.clone(),
        Arc::new(agentic_runtime::state::RuntimeState::new()),
        platform,
        Arc::new(agentic_pipeline::platform::IdentityResolver),
        None,
        None,
        None,
        Arc::new(agentic_runtime::orchestrator::router::NoopTaskRouter),
        Some(s.workspace_id),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
}

async fn status(s: &Stranded) -> agentic_runtime::entity::run::Model {
    crud::get_run(&s.db, &s.run_id)
        .await
        .expect("get_run")
        .expect("run row")
}

fn loaded(load_id: &str, table: &str) -> (&'static str, Value) {
    (
        "table_loaded",
        json!({"event_type": "table_loaded", "pipeline_name": "p", "load_id": load_id,
               "table": table, "rows": 10}),
    )
}

fn load_completed(load_id: &str) -> (&'static str, Value) {
    (
        "load_completed",
        json!({"event_type": "load_completed", "pipeline_name": "p", "load_id": load_id,
               "tables": ["vendor_sales", "vendor_forecasting"],
               "rows_loaded": {"vendor_sales": 10, "vendor_forecasting": 10},
               "duration_ms": 1000, "folds": []}),
    )
}

/// The observed prod shape: queue row `completed`, load finished. Must end
/// `done`, with no recovery marker, the lease freed, and the worker's metadata.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_load_interrupted_before_done_recovers_as_done() {
    let Some(s) = strand(
        "completed",
        &[
            loaded("L1", "vendor_sales"),
            loaded("L1", "vendor_forecasting"),
            load_completed("L1"),
        ],
    )
    .await
    else {
        eprintln!("skipping: no DB available");
        return;
    };

    recover(&s).await;

    let run = status(&s).await;
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "a load that emitted load_completed must not read failed after a restart \
         (error_message: {:?})",
        run.error_message
    );
    assert_eq!(run.error_message, None);
    assert_eq!(
        run.task_metadata.as_ref().and_then(|m| m.get("load_id")),
        Some(&json!("L1")),
        "the metadata the worker's Done outcome would have written"
    );

    let events = crud::get_all_events(&s.db, &s.run_id)
        .await
        .expect("events");
    assert_eq!(
        events.last().map(|e| e.event_type.as_str()),
        Some("load_completed"),
        "recovery of a finished run is invisible: no recovery_resumed marker"
    );
    let leased = pipeline_lease::list_for_workspace(&s.db, s.workspace_id)
        .await
        .expect("list leases")
        .into_iter()
        .any(|l| l.run_id == s.run_id);
    assert!(!leased, "lease for {} must be released", s.pipeline_name);
}

/// A `load_completed` on a run whose task is still `queued` may describe an
/// earlier attempt — a reset-in-place retry whose event delete failed, whose new
/// attempt has not started yet. Finishing that run would drop the retry, so the
/// runtime primitive refuses while any task is live and leaves it to a worker.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_with_live_work_is_not_finished_on_stale_evidence() {
    let Some(s) = strand(
        "queued",
        &[loaded("L1", "vendor_sales"), load_completed("L1")],
    )
    .await
    else {
        eprintln!("skipping: no DB available");
        return;
    };

    let finished = crud::complete_run_with_lost_outcome(&s.db, &s.run_id, None)
        .await
        .expect("complete_run_with_lost_outcome");

    assert!(!finished, "must refuse while a task is still queued");
    assert_eq!(status(&s).await.task_status.as_deref(), Some("running"));
    let entry = crud::get_queue_entry(&s.db, &s.run_id)
        .await
        .expect("queue entry")
        .expect("queue row");
    assert_eq!(
        entry.queue_status, "queued",
        "live work is never cancelled here"
    );

    // End to end: recovery as a whole must leave it to a worker too. The
    // spawned worker may claim the row and run it, so pin only what recovery
    // itself must not do — finish the run on the stale `load_completed`, or
    // cancel its task.
    recover(&s).await;
    let run = status(&s).await;
    assert_ne!(
        run.task_status.as_deref(),
        Some("done"),
        "recovery finished a run with live work on stale evidence"
    );
    let entry = crud::get_queue_entry(&s.db, &s.run_id)
        .await
        .expect("queue entry")
        .expect("queue row");
    assert_ne!(entry.queue_status, "cancelled");
}

/// A load that finished with a table failure carries real failure evidence, so
/// recovery keeps it `failed` rather than laundering it into `done`.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_with_a_failed_table_still_reads_failed_after_recovery() {
    let Some(s) = strand(
        "completed",
        &[
            loaded("L1", "vendor_sales"),
            (
                "table_load_failed",
                json!({"event_type": "table_load_failed", "pipeline_name": "p",
                       "load_id": "L1", "table": "vendor_forecasting",
                       "error": "commit conflict"}),
            ),
            load_completed("L1"),
        ],
    )
    .await
    else {
        eprintln!("skipping: no DB available");
        return;
    };

    recover(&s).await;

    assert_eq!(status(&s).await.task_status.as_deref(), Some("failed"));
}

/// A run that failed outright before the restart never reached
/// `load_completed`; recovery must leave it `failed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_errored_before_the_restart_still_reads_failed() {
    for kind in ["pipeline_error", "task_failed"] {
        let Some(s) = strand(
            "failed",
            &[
                loaded("L1", "vendor_sales"),
                (
                    kind,
                    json!({"event_type": kind, "pipeline_name": "p", "load_id": "L1",
                           "error": "boom"}),
                ),
            ],
        )
        .await
        else {
            eprintln!("skipping: no DB available");
            return;
        };

        recover(&s).await;

        assert_eq!(
            status(&s).await.task_status.as_deref(),
            Some("failed"),
            "{kind} before the restart"
        );
    }
}
