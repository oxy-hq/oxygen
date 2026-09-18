//! Facade integration test for `start_airway_run`.
//!
//! The worker path is covered by `agentic-airway`'s
//! `worker_integration`. This pins the *seeding* half: that
//! `start_airway_run` resolves + renders + validates the `.airway.yml`,
//! and atomically lands the three rows the coordinator needs —
//! `agentic_runs` (source_type=airway), `airway_run_extensions`, and
//! a queued `TaskSpec::Airway` in `agentic_task_queue`.
//!
//! Also pins one executor-side property, because it is the other half of the
//! same lease lifecycle: a dispatch failure must RELEASE the single-flight
//! lease the submit above acquired.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_airway::AirwayMigrator;
use agentic_airway::extension::run_extension;
use agentic_core::delegation::TaskSpec;
use agentic_pipeline::airway_run::{StartAirwayRequest, start_airway_run};
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use async_trait::async_trait;
use entity::airway_source_config;
use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use uuid::Uuid;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start Postgres testcontainer — is Docker running?"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let mut db = None;
    for attempt in 0..10 {
        match Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}");
            }
            Err(e) => panic!("connect after 10 retries: {e}"),
        }
    }
    let db = db?;
    // Central first — `airway_source_config` (Task 1) lives in this central
    // migrator, and `start_airway_run` now resolves against it via
    // `resolve_admission` (Task 3) on every call, so this binary needs the
    // table even though no test here touches it directly. See
    // oxy_test_utils::migration for the full ordering rationale.
    // `airway_run_extensions.run_id` and the queue row both FK / key off
    // `agentic_runs`.
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations")
        .finish()
        .await;
    Some(db)
}

/// Minimal `WorkspaceContext` whose `workspace_path` points at a real
/// temp dir so `start_airway_run` can read the `.airway.yml`. Every
/// other method is unreachable on this path.
///
/// `compiled` stands in for the host's compile-boundary hook: `Some(yaml)`
/// is a workspace whose `.airway.yml` is a compiled `airway_pipelines` row
/// (what a stateless replica sees), `None` is "read the filesystem".
struct TmpWorkspace {
    root: PathBuf,
    compiled: Option<String>,
}

#[async_trait]
impl agentic_pipeline::platform::ProjectContext for TmpWorkspace {
    async fn resolve_connector(
        &self,
        _db_name: &str,
    ) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _has_explicit_model: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var_name: &str) -> Option<String> {
        None
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for TmpWorkspace {
    fn workspace_path(&self) -> Option<&Path> {
        Some(&self.root)
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("tmp workspace: connector '{name}' unavailable"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("tmp workspace: integration '{name}' unavailable"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _workflow_ref: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("tmp workspace: not available".into())
    }
    async fn resolve_pipeline_yaml(&self, _pipeline_ref: &str) -> Result<Option<String>, String> {
        Ok(self.compiled.clone())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn start_airway_run_seeds_run_extension_and_queue() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline_name = format!("facade_{}", uuid::Uuid::new_v4().simple());

    // `{{ env }}` exercises templating *through the facade* — proves
    // `start_airway_run` renders before persisting.
    let yaml = format!(
        r#"
name: {pipeline_name}_{{{{ env }}}}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-facade
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
concurrency: 3
resources:
  - users
"#
    );
    std::fs::write(dir.path().join("p.airway.yml"), yaml).expect("write spec");

    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let request = StartAirwayRequest {
        pipeline_ref: "p.airway.yml".to_string(),
        variables: Some(serde_json::json!({ "env": "prod" })),
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    };

    let run_id = start_airway_run(
        &db,
        &ws,
        request,
        agentic_pipeline::TaskScope::Global,
        uuid::Uuid::nil(),
    )
    .await
    .expect("start_airway_run");

    let rendered_name = format!("{pipeline_name}_prod");

    // ── agentic_runs row ─────────────────────────────────────────────────
    let run = crud::get_run(&db, &run_id)
        .await
        .expect("query run")
        .expect("agentic_runs row exists");
    assert_eq!(run.source_type.as_deref(), Some("airway"));
    assert_eq!(run.question, format!("airway: {rendered_name}"));

    // ── airway_run_extensions row ────────────────────────────────────────
    let ext = run_extension::get_run_extension(&db, &run_id)
        .await
        .expect("query extension")
        .expect("airway_run_extensions row exists");
    assert_eq!(ext.pipeline_name, rendered_name);
    assert_eq!(ext.pipeline_ref.as_deref(), Some("p.airway.yml"));
    assert_eq!(ext.concurrency, 3);
    assert_eq!(
        ext.resources,
        serde_json::json!(["users"]),
        "selected resources persisted verbatim"
    );
    assert_eq!(
        ext.contract_policy, None,
        "no airway_source_config row for this source_kind: extension records no contract_policy"
    );
    assert_eq!(
        ext.environment, None,
        "no airway_source_config row for this source_kind: extension records no environment"
    );

    // ── queued TaskSpec::Airway ──────────────────────────────────────────
    let entry = crud::get_queue_entry(&db, &run_id)
        .await
        .expect("query queue")
        .expect("queue row exists");
    let spec: TaskSpec = serde_json::from_value(entry.spec).expect("deserialize spec");
    match spec {
        TaskSpec::Airway {
            pipeline_ref,
            variables,
            ..
        } => {
            assert_eq!(pipeline_ref, "p.airway.yml");
            assert_eq!(
                variables.and_then(|v| v.get("env").cloned()),
                Some(serde_json::json!("prod")),
            );
        }
        other => panic!("expected TaskSpec::Airway, got {other:?}"),
    }

    // ── run-history list surfaces this run ───────────────────────────────
    // Same workspace id the run was started under (nil, as in local mode).
    let runs =
        agentic_pipeline::airway_run::list_airway_runs(&db, uuid::Uuid::nil(), "p.airway.yml", 50)
            .await
            .expect("list_airway_runs");
    assert!(
        runs.iter().any(|r| r.run_id == run_id),
        "started run must appear in the pipeline's run history"
    );
    // A different pipeline_ref must not match.
    let other = agentic_pipeline::airway_run::list_airway_runs(
        &db,
        uuid::Uuid::nil(),
        "nope.airway.yml",
        50,
    )
    .await
    .expect("list_airway_runs other");
    assert!(
        !other.iter().any(|r| r.run_id == run_id),
        "run must not leak into an unrelated pipeline's history"
    );
}

/// The compile-boundary regression: seeding a run must work on a node with
/// **no workspace directory at all** — the shape of every durable-worker
/// replica, and of the `serve` fleet. Before the boundary port, resolution
/// canonicalised the workspace root first, so an absent working copy failed
/// with "workspace root is not accessible" no matter what was queued.
///
/// Templating still happens at parse time on the compiled body, so the early
/// validation `start_airway_run` exists for is intact and the worker renders
/// the same document from the same `variables`.
#[tokio::test(flavor = "multi_thread")]
async fn start_airway_run_reads_the_compile_boundary_without_a_working_copy() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let pipeline_name = format!("compiled_{}", uuid::Uuid::new_v4().simple());
    let yaml = format!(
        r#"
name: {pipeline_name}_{{{{ env }}}}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-compiled
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
concurrency: 2
resources:
  - users
"#
    );

    // No tempdir: the workspace root does not exist on this machine.
    let absent = PathBuf::from(format!(
        "/nonexistent-oxy-workspace/{}",
        uuid::Uuid::new_v4()
    ));
    assert!(!absent.exists(), "precondition: no working copy");
    assert!(
        agentic_pipeline::pipeline_ref::resolve_pipeline_ref(&absent, "p.airway.yml").is_err(),
        "the FS path must be unavailable for this test to mean anything"
    );

    let ws = TmpWorkspace {
        root: absent,
        compiled: Some(yaml),
    };
    let request = StartAirwayRequest {
        pipeline_ref: "pipelines/p.airway.yml".to_string(),
        variables: Some(serde_json::json!({ "env": "prod" })),
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    };

    let run_id = start_airway_run(
        &db,
        &ws,
        request,
        agentic_pipeline::TaskScope::Global,
        uuid::Uuid::nil(),
    )
    .await
    .expect("a compiled pipeline must be runnable with no working copy");

    // The rendered name proves the compiled body went through the same
    // `from_yaml_with_vars` render the worker will repeat.
    let ext = run_extension::get_run_extension(&db, &run_id)
        .await
        .expect("query extension")
        .expect("airway_run_extensions row exists");
    assert_eq!(ext.pipeline_name, format!("{pipeline_name}_prod"));
    assert_eq!(ext.pipeline_ref.as_deref(), Some("pipelines/p.airway.yml"));

    // `variables` still ride the queue spec, so the worker re-renders rather
    // than inheriting a pre-rendered document.
    let entry = crud::get_queue_entry(&db, &run_id)
        .await
        .expect("query queue")
        .expect("queue row exists");
    let spec: TaskSpec = serde_json::from_value(entry.spec).expect("deserialize spec");
    match spec {
        TaskSpec::Airway { variables, .. } => assert_eq!(
            variables.and_then(|v| v.get("env").cloned()),
            Some(serde_json::json!("prod")),
        ),
        other => panic!("expected TaskSpec::Airway, got {other:?}"),
    }
}

/// The containment guard runs BEFORE the backend choice, so a traversal ref
/// can't address a compiled row either — a host that would happily hand back a
/// body never sees the ref, and no run row is seeded.
///
/// The fixture is built so the guard is the ONLY thing that can reject. Both
/// halves of that mattered: an earlier version used a stub body with no
/// `destination:`, so `AirwayPipelineSpec` refused it whatever the ref was and
/// `expect_err` passed with the guard deleted. This one hands back
/// [`pipeline_yaml`] — a spec that parses and submits — so letting a traversal
/// ref through would produce a *successful* run and fail here.
///
/// And it asserts the guard's own message rather than merely the absence of a
/// resolved path: `TmpWorkspace::resolve_pipeline_yaml` ignores the ref, so no
/// path is ever constructed on this route and "the error doesn't name the
/// workspace root" was true by construction.
#[tokio::test(flavor = "multi_thread")]
async fn start_airway_run_contains_the_ref_on_the_compiled_path() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = TmpWorkspace {
        root: PathBuf::from("/nonexistent-oxy-workspace"),
        compiled: Some(pipeline_yaml("contained")),
    };
    // The reason the guard must give for each shape, so a ref rejected by some
    // *later* check can't be mistaken for containment holding.
    for (bad, want) in [
        ("../../etc/passwd", "must not contain `..` segments"),
        ("/etc/passwd", "must be relative to the workspace"),
        ("a/../../b", "must not contain `..` segments"),
    ] {
        let request = StartAirwayRequest {
            pipeline_ref: bad.to_string(),
            variables: None,
            thread_id: None,
            resources: Vec::new(),
            schedule_id: None,
            trigger: None,
            logical_date: None,
            retry_of: None,
            backfill_from: None,
            backfill_to: None,
        };
        let err = start_airway_run(
            &db,
            &ws,
            request,
            agentic_pipeline::TaskScope::Global,
            uuid::Uuid::nil(),
        )
        .await
        .expect_err("a traversal ref must be rejected before any backend")
        .to_string();
        assert!(
            err.contains(want),
            "`{bad}` must be refused by the containment guard ({want}), got: {err}"
        );
        assert!(
            !err.contains("nonexistent-oxy-workspace"),
            "errors must quote only the ref, never a resolved path: {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn start_airway_run_rejects_missing_pipeline_file() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let request = StartAirwayRequest {
        pipeline_ref: "does-not-exist.airway.yml".to_string(),
        variables: None,
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    };
    let err = start_airway_run(
        &db,
        &ws,
        request,
        agentic_pipeline::TaskScope::Global,
        uuid::Uuid::nil(),
    )
    .await
    .expect_err("missing file must error before any DB write");
    assert!(
        err.to_string().contains("does-not-exist.airway.yml"),
        "got: {err}"
    );
}

/// Regression: a dispatch failure must release the single-flight lease.
///
/// The lease is taken at SUBMIT (`start_airway_run`), and every release site
/// used to live in a submit, retry or backfill path — nothing covered the
/// executor. So an unreadable/absent `.airway.yml`, a parse error or an
/// unresolvable destination left the run terminal with its lease still held
/// for the full 6h TTL. Observed on dev as a run reaching `failed` 38ms after
/// creation while blocking its pipeline for 27 minutes.
///
/// Asserts BOTH halves: the dispatch still fails (we did not paper over the
/// error to free the lease), and the lease row is gone.
#[tokio::test(flavor = "multi_thread")]
async fn airway_dispatch_failure_releases_the_lease() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let run_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4();
    let pipeline_name = format!("dispatch_fail_{}", uuid::Uuid::new_v4().simple());

    // Hold the lease exactly as a submit would have.
    let acquired = agentic_airway::extension::pipeline_lease::try_acquire(
        &db,
        workspace_id,
        &pipeline_name,
        &run_id,
        agentic_airway::extension::pipeline_lease::LEASE_TTL_SECS,
    )
    .await
    .expect("acquire lease");
    assert!(
        matches!(
            acquired,
            agentic_airway::extension::pipeline_lease::LeaseAcquisition::Acquired
        ),
        "precondition: lease must be held before dispatch, got {acquired:?}"
    );

    // `pipeline_ref` does not exist on disk, so `execute_airway` fails at
    // resolution — upstream of `worker.execute`, i.e. no engine is running.
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    });
    let executor = agentic_pipeline::executor::PipelineTaskExecutor::bare(platform, db.clone());

    let assignment = agentic_core::delegation::TaskAssignment {
        task_id: uuid::Uuid::new_v4().to_string(),
        parent_task_id: None,
        run_id: run_id.clone(),
        spec: TaskSpec::Airway {
            pipeline_ref: "no-such-pipeline.airway.yml".to_string(),
            variables: None,
            resources: Vec::new(),
            backfill_from: None,
            backfill_to: None,
            // `None` = airway's defaults. This test is about the lease being
            // released when dispatch fails, not about admission.
            contract_policy: None,
            environment: None,
        },
        policy: None,
    };

    // `ExecutingTask` is not `Debug`, so match rather than `expect_err`.
    let err =
        match agentic_runtime::orchestrator::worker::TaskExecutor::execute(&executor, assignment)
            .await
        {
            Ok(_) => panic!("dispatch must fail for a missing pipeline_ref"),
            Err(e) => e,
        };
    assert!(
        err.contains("no-such-pipeline.airway.yml"),
        "error should name the unresolvable ref, got: {err}"
    );

    let still_held =
        agentic_airway::extension::pipeline_lease::list_for_workspace(&db, workspace_id)
            .await
            .expect("list leases")
            .into_iter()
            .any(|l| l.run_id == run_id);
    assert!(
        !still_held,
        "dispatch failed but the lease survived — the pipeline is blocked for the full TTL"
    );
}

/// Regression: recovery force-failing an airway run must release its lease.
///
/// `resume_from_state` dispatches airway into its `_` arm, which resumes only
/// from `suspend_data` — and airway runs never suspend (no HITL) — so recovery
/// CANNOT resume an airway run and force-fails it via `mark_recovery_failed`.
/// The single-flight lease was taken at submit and nothing else freed it, so
/// the pipeline stayed blocked for the full 6h TTL.
///
/// Reaching that requires the run's queued task row to be GONE — claimed by a
/// worker that died, or dead-lettered. A merely interrupted run still has its
/// queued `TaskSpec::Airway`, and recovery re-drives it successfully, leaking
/// nothing; the test drops the queue row for exactly this reason. Verified by
/// diagnostic: with the row present the run stays `running` with no error and
/// `mark_recovery_failed` never fires.
///
/// Releasing there is sound without a liveness predicate because recovery
/// holds the driver lease: it has already concluded the run is dead, and on the
/// success path it would have RE-DRIVEN it — a strictly stronger act.
#[tokio::test(flavor = "multi_thread")]
async fn recovery_failure_releases_the_airway_lease() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline_name = format!("recov_{}", uuid::Uuid::new_v4().simple());
    let yaml = format!(
        r#"
name: {pipeline_name}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-recovery
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
resources:
  - users
"#
    );
    std::fs::write(dir.path().join("p.airway.yml"), yaml).expect("write spec");

    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let workspace_id = uuid::Uuid::new_v4();
    // Seed through the real submit path so the run, its extension and its
    // lease are exactly what production would have left behind.
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

    // Make it look like a run stranded by a restart: root, `running`, no live
    // driver — the exact shape `get_resumable_root_runs` selects.
    sea_orm::ConnectionTrait::execute_raw(
        &db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE agentic_runs SET task_status = 'running', driver_id = NULL, \
             driver_heartbeat_at = NULL WHERE id = $1",
            [run_id.clone().into()],
        ),
    )
    .await
    .expect("strand the run");

    // Drop the queued task row too. With it present, recovery simply re-drives
    // the queued `TaskSpec::Airway` and SUCCEEDS — which is the common
    // interrupted case and correctly leaks nothing. The leak needs a run whose
    // work is gone, so recovery must fall through to `resume_from_state`, where
    // airway has no checkpoint and no suspension to resume from.
    sea_orm::ConnectionTrait::execute_raw(
        &db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "DELETE FROM agentic_task_queue WHERE run_id = $1",
            [run_id.clone().into()],
        ),
    )
    .await
    .expect("drop queued task");

    // The lease is taken at CLAIM now, not at submit, so stand in for the
    // executor and acquire it. This assertion previously read "submit must
    // have taken the lease" and failed loudly when that stopped being true —
    // which is exactly what it was written to do.
    agentic_airway::extension::pipeline_lease::try_acquire(
        &db,
        workspace_id,
        &pipeline_name,
        &run_id,
        agentic_airway::extension::pipeline_lease::LEASE_TTL_SECS,
    )
    .await
    .expect("acquire the lease as the executor would at claim time");

    let held_before =
        agentic_airway::extension::pipeline_lease::list_for_workspace(&db, workspace_id)
            .await
            .expect("list leases")
            .into_iter()
            .any(|l| l.run_id == run_id);
    assert!(
        held_before,
        "precondition: the run must hold its lease before recovery runs"
    );

    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    });
    agentic_pipeline::recovery::recover_active_runs(
        db.clone(),
        Arc::new(agentic_runtime::state::RuntimeState::new()),
        platform,
        None,
        None,
        None,
        None,
        Arc::new(agentic_runtime::orchestrator::router::NoopTaskRouter),
        Some(workspace_id),
        None,
    )
    .await;

    // Pin the CHAIN, not just the outcome: without this, a future change that
    // makes recovery skip airway roots entirely would leave the lease released
    // for an unrelated reason and the assertion below would go green while
    // testing nothing.
    let after = crud::get_run(&db, &run_id)
        .await
        .expect("get_run")
        .expect("run row");
    assert_eq!(
        after.task_status.as_deref(),
        Some("failed"),
        "precondition: recovery must have force-failed the run"
    );

    let still_held =
        agentic_airway::extension::pipeline_lease::list_for_workspace(&db, workspace_id)
            .await
            .expect("list leases")
            .into_iter()
            .any(|l| l.run_id == run_id);
    assert!(
        !still_held,
        "recovery force-failed the run but its lease survived — the pipeline is \
         blocked for the full 6h TTL (reached when the run's queued task row is \
         gone: claimed-and-orphaned, or dead-lettered)"
    );
}

fn unique_kind(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("airway-run-test-{id}")),
        git_namespace_id: Set(None),
        git_remote_url: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        path: Set(None),
        last_opened_at: Set(None),
        created_by: Set(None),
        org_id: Set(None),
        status: Set(WorkspaceStatus::Ready),
        error: Set(None),
        monthly_vlm_budget_micros: Set(None),
        current_revision_id: Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

/// Pins that `start_airway_run` threads `resolve_admission`'s two fields
/// onto the *correct* field of the queued `TaskSpec::Airway` **and** the
/// `airway_run_extensions` row — not just that they're populated, and not
/// just in one of the two places. Both writes must come from the same
/// `admission` binding (Task 4's invariant): if the extension row and the
/// queued spec ever disagreed, that would be worse than recording nothing,
/// since it would look authoritative while being wrong. `contract_policy`
/// and `environment` use deliberately distinguishable values (not, say,
/// both `"sandbox"`), so a transposition bug (`contract_policy:
/// admission.environment` or vice versa) or a dropped field would fail one
/// of the assertions below. The empty-table case
/// (`start_airway_run_seeds_run_extension_and_queue`) can't catch that class
/// of bug: `None`/`None` is symmetric under a swap.
#[tokio::test(flavor = "multi_thread")]
async fn start_airway_run_threads_resolved_admission_onto_queued_spec() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let ws_id = seed_workspace(&db).await;
    let kind = unique_kind("filesystem");
    airway_source_config::ActiveModel {
        source_kind: Set(kind.clone()),
        workspace_id: Set(Some(ws_id)),
        contract_policy: Set(Some("require_declared".to_string())),
        environment: Set(Some("sandbox".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed admission config row");

    let dir = tempfile::tempdir().expect("tempdir");
    let pipeline_name = format!("admission_{}", Uuid::new_v4().simple());
    let yaml = format!(
        r#"
name: {pipeline_name}
source:
  kind: {kind}
  config:
    base_path: /tmp/airway-facade
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
concurrency: 1
resources:
  - users
"#
    );
    std::fs::write(dir.path().join("p.airway.yml"), yaml).expect("write spec");

    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let request = StartAirwayRequest {
        pipeline_ref: "p.airway.yml".to_string(),
        variables: None,
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    };

    let run_id = start_airway_run(
        &db,
        &ws,
        request,
        agentic_pipeline::TaskScope::Global,
        ws_id,
    )
    .await
    .expect("start_airway_run");

    let entry = crud::get_queue_entry(&db, &run_id)
        .await
        .expect("query queue")
        .expect("queue row exists");
    let spec: TaskSpec = serde_json::from_value(entry.spec).expect("deserialize spec");
    match spec {
        TaskSpec::Airway {
            contract_policy,
            environment,
            ..
        } => {
            assert_eq!(
                contract_policy.as_deref(),
                Some("require_declared"),
                "contract_policy must carry the resolved contract_policy, not environment"
            );
            assert_eq!(
                environment.as_deref(),
                Some("sandbox"),
                "environment must carry the resolved environment, not contract_policy"
            );
        }
        other => panic!("expected TaskSpec::Airway, got {other:?}"),
    }

    // ── airway_run_extensions row: same admission, same run ────────────────
    let ext = run_extension::get_run_extension(&db, &run_id)
        .await
        .expect("query extension")
        .expect("airway_run_extensions row exists");
    assert_eq!(
        ext.contract_policy.as_deref(),
        Some("require_declared"),
        "extension row must record the resolved contract_policy, not environment"
    );
    assert_eq!(
        ext.environment.as_deref(),
        Some("sandbox"),
        "extension row must record the resolved environment, not contract_policy"
    );
}

fn pipeline_yaml(name: &str) -> String {
    format!(
        r#"
name: {name}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-coalesce
    pattern: "*.jsonl"
    format: jsonl
    table_name: users
destination:
  kind: memory
  config:
    dataset_name: scratch
resources:
  - users
"#
    )
}

fn req(pipeline_ref: &str, variables: Option<serde_json::Value>) -> StartAirwayRequest {
    StartAirwayRequest {
        pipeline_ref: pipeline_ref.to_string(),
        variables,
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: Some("test".to_string()),
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    }
}

/// Submitting the same pipeline twice must return ONE run, not two.
///
/// Coalescing is what makes "queue instead of reject" safe. Without it, ten
/// clicks become ten identical runs that then serialize one at a time — the
/// backlog problem that motivated refusing contended submits in the first
/// place. This is the property that replaced the 409.
#[tokio::test(flavor = "multi_thread")]
async fn submitting_twice_coalesces_onto_one_run() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let name = format!("coal_{}", uuid::Uuid::new_v4().simple());
    std::fs::write(dir.path().join("p.airway.yml"), pipeline_yaml(&name)).expect("write");
    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let workspace_id = uuid::Uuid::new_v4();

    let first = start_airway_run(
        &db,
        &ws,
        req("p.airway.yml", None),
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    .expect("first submit");
    let second = start_airway_run(
        &db,
        &ws,
        req("p.airway.yml", None),
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    .expect("second submit must be accepted, not refused");

    assert_eq!(
        first, second,
        "a second submit must coalesce onto the queued run, not create another"
    );
}

/// Different variables are different work and must NOT coalesce.
///
/// A coalesce that ignored variables would hand one caller another caller's
/// parameters — silently serving the wrong result rather than failing.
#[tokio::test(flavor = "multi_thread")]
async fn different_variables_do_not_coalesce() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let name = format!("vars_{}", uuid::Uuid::new_v4().simple());
    std::fs::write(dir.path().join("p.airway.yml"), pipeline_yaml(&name)).expect("write");
    let ws = TmpWorkspace {
        root: dir.path().to_path_buf(),
        compiled: None,
    };
    let workspace_id = uuid::Uuid::new_v4();

    let a = start_airway_run(
        &db,
        &ws,
        req("p.airway.yml", Some(serde_json::json!({"env": "a"}))),
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    .expect("first");
    let b = start_airway_run(
        &db,
        &ws,
        req("p.airway.yml", Some(serde_json::json!({"env": "b"}))),
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    .expect("second");

    assert_ne!(a, b, "different variables must produce distinct runs");
}

/// A lease whose holder has already terminalized must be reclaimable at once.
///
/// TTL is the backstop, not the reclaim policy. Before the widened predicate a
/// pipeline whose holder died — or was force-failed by crash recovery — stayed
/// blocked for the full six hours with nothing running, which is exactly how a
/// wedged `quickbooks_financials` lease was found on dev.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_holder_does_not_block_the_pipeline() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    use agentic_airway::extension::pipeline_lease::{self, LeaseAcquisition};

    let workspace_id = uuid::Uuid::new_v4();
    let pipeline = format!("live_{}", uuid::Uuid::new_v4().simple());
    let dead = uuid::Uuid::new_v4().to_string();

    crud::insert_run(&db, &dead, "Q", None, "airway", None, workspace_id)
        .await
        .unwrap();
    assert!(matches!(
        pipeline_lease::try_acquire(&db, workspace_id, &pipeline, &dead, 3600)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    ));

    // A live holder still blocks — the guard must not be toothless.
    let next = uuid::Uuid::new_v4().to_string();
    assert!(
        matches!(
            pipeline_lease::try_acquire(&db, workspace_id, &pipeline, &next, 3600)
                .await
                .unwrap(),
            LeaseAcquisition::Held { .. }
        ),
        "a live holder must still hold the lease"
    );

    // Terminalize it; the lease is now reclaimable without waiting out the TTL.
    sea_orm::ConnectionTrait::execute_raw(
        &db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "UPDATE agentic_runs SET task_status = 'failed' WHERE id = $1",
            [dead.clone().into()],
        ),
    )
    .await
    .unwrap();

    assert!(
        matches!(
            pipeline_lease::try_acquire(&db, workspace_id, &pipeline, &next, 3600)
                .await
                .unwrap(),
            LeaseAcquisition::Acquired
        ),
        "a terminal holder must not block for the full TTL"
    );
}
