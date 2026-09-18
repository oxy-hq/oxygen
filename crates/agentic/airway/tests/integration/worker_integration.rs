//! End-to-end integration test for the airway worker.
//!
//! Spins up a real Postgres via testcontainers, runs the runtime +
//! airway migrators, then drives a full pipeline: a filesystem source
//! (a temp JSONL file) → the in-process memory destination. Proves the
//! whole chain — source factory, `Source::try_from_connector_with` bridge,
//! `AirwayPgStateStore`, the engine run, and the
//! `PipelineEvent → AirwayEvent` forwarder — works against a live DB.
//!
//! Requires Docker (or `OXY_DATABASE_URL` pointing at a throwaway PG).

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use agentic_airway::config::AirwayPipelineSpec;
use agentic_airway::extension::{load_audit, workspace_pipeline_state};
use agentic_airway::worker::AirwayWorker;
use agentic_core::delegation::TaskOutcome;
use sea_orm::EntityTrait;

use crate::harness::test_db;

#[tokio::test(flavor = "multi_thread")]
async fn worker_runs_filesystem_to_memory_end_to_end() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // Unique workspace + pipeline name so concurrent test runs don't collide on
    // the `airway_workspace_pipeline_state` primary key.
    let workspace_id = uuid::Uuid::new_v4();
    let pipeline_name = format!("it_fs_mem_{}", uuid::Uuid::new_v4().simple());

    // ── Temp JSONL source: three user records, one per line ───────────────
    let dir = tempfile::tempdir().expect("tempdir");
    let mut f = std::fs::File::create(dir.path().join("users.jsonl")).expect("create jsonl");
    writeln!(f, r#"{{"id": 1, "name": "Alice"}}"#).unwrap();
    writeln!(f, r#"{{"id": 2, "name": "Bob"}}"#).unwrap();
    writeln!(f, r#"{{"id": 3, "name": "Carol"}}"#).unwrap();
    f.flush().unwrap();

    let yaml = format!(
        r#"
name: {pipeline_name}
source:
  kind: filesystem
  config:
    base_path: {base}
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
"#,
        base = dir.path().display(),
    );
    let spec = AirwayPipelineSpec::from_yaml_str(&yaml).expect("parse spec");

    // ── Drive the worker ──────────────────────────────────────────────────
    // `AirwayAdmission::default()` is `permissive` / `production`, so this
    // drives the same admission path a normal run takes.
    let worker = AirwayWorker::new(
        Arc::new(db.clone()),
        agentic_airway::AirwayAdmission::default(),
    );
    // Normal run (no resumable-backfill run-scoped store). The third arg is
    // the owning run id used to stamp the engine load_id onto the run
    // extension; this test seeds no `agentic_runs`/extension row, so that
    // stamp is a best-effort no-op (matches `set_run_load_id`'s contract).
    let mut task = worker.execute(spec, None, "it-worker-run".to_string(), workspace_id);

    // Collect events until the task produces its terminal outcome.
    let mut event_types: Vec<String> = Vec::new();
    let outcome = loop {
        tokio::select! {
            ev = task.events.recv() => {
                match ev {
                    Some((event_type, _payload)) => event_types.push(event_type),
                    None => { /* event channel closed; keep waiting for outcome */ }
                }
            }
            oc = task.outcomes.recv() => {
                break oc.expect("worker produced an outcome");
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                panic!("airway run timed out; events so far: {event_types:?}");
            }
        }
    };

    // Drain any events still buffered after the outcome arrived.
    while let Ok(Some((event_type, _))) =
        tokio::time::timeout(Duration::from_millis(100), task.events.recv()).await
    {
        event_types.push(event_type);
    }

    // ── Assertions ────────────────────────────────────────────────────────
    match &outcome {
        TaskOutcome::Done { .. } => {}
        other => panic!("expected Done, got {other:?}; events: {event_types:?}"),
    }

    assert!(
        event_types.iter().any(|e| e == "load_started"),
        "missing load_started; got {event_types:?}"
    );
    assert!(
        event_types.iter().any(|e| e == "extract_completed"),
        "missing extract_completed; got {event_types:?}"
    );
    assert!(
        event_types.iter().any(|e| e == "load_completed"),
        "missing load_completed; got {event_types:?}"
    );

    // State store row was written by `AirwayPgStateStore::save` at the end of a
    // successful run — under THIS workspace's key, which is what makes the row
    // the run's own rather than one shared with every workspace of that name.
    let state_row =
        workspace_pipeline_state::Entity::find_by_id((workspace_id, pipeline_name.clone()))
            .one(&db)
            .await
            .expect("query airway_workspace_pipeline_state")
            .expect("a state row exists for this workspace after a successful run");
    assert!(
        state_row.version >= 1,
        "version should have advanced past the initial 0, got {}",
        state_row.version
    );

    // Audit row transitioned to completed.
    let audit_rows = load_audit::Entity::find()
        .all(&db)
        .await
        .expect("query load_audit");
    let ours: Vec<_> = audit_rows
        .into_iter()
        .filter(|r| r.pipeline_name == pipeline_name)
        .collect();
    assert_eq!(ours.len(), 1, "exactly one audit row for this run");
    assert_eq!(
        ours[0].workspace_id,
        Some(workspace_id),
        "the audit row must be attributed to the workspace that ran the load — \
         `pipeline_name` alone is not unique across workspaces"
    );
    assert_eq!(
        ours[0].status,
        load_audit::status::COMPLETED,
        "audit row should be completed; error_message={:?}",
        ours[0].error_message
    );
    // This airway revision infers + hashes the schema before
    // `record_load_start`, so even a first load carries a non-empty
    // fingerprint. The nullable column + empty-string→`None` mapping in
    // `AirwayPgStateStore::record_load_start` remains the defensive
    // contract for engine revisions that record the audit row *before*
    // schema inference; here we just assert a real hash landed.
    assert!(
        ours[0]
            .schema_hash
            .as_deref()
            .is_some_and(|h| !h.is_empty()),
        "expected a non-empty schema fingerprint, got {:?}",
        ours[0].schema_hash
    );
}
