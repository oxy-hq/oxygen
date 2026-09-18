//! Real-DB e2e for the reset-in-place airway retry × mid-window resume.
//!
//! This is the oxy-side half of the P2c-4 release gate. The airway engine test
//! `resumable_backfill_resumes_without_re_pull_or_gap` proves that, GIVEN a
//! persisted cursor, a re-run resumes with no re-pull and no gap. This proves the
//! oxy layer keeps that cursor alive across a crash → retry: the reset-in-place
//! retry must revive the run's task, re-scope it globally-claimable, clear the
//! failed attempt's error + events, bump `retry_count` — and CRUCIALLY leave
//! `airway_run_extensions.resume_state` intact, so the re-driven run resumes from
//! where it crashed instead of re-extracting the whole window.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentic_airway::AirwayRunScopedStateStore;
use agentic_airway::extension::AirwayMigrator;
use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use airway::Schema;
use airway::state::{PipelineState, ResourceState, StateStore};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;

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
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
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
                tokio::time::sleep(Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}, retrying");
            }
            Err(e) => panic!("connect to test DB failed after 10 retries: {e}"),
        }
    }
    let db = db?;
    // Central then runtime (production order — see
    // oxy_test_utils::migration), then AirwayMigrator: airway_run_extensions.run_id
    // FKs to agentic_runs.id, so it must land after runtime.
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations failed")
        .finish()
        .await;
    Some(db)
}

/// Minimal `WorkflowWorkspaceContext` — the reset-in-place path never touches it
/// (only the clone-reseed fallback reads the workspace filesystem), so every
/// method is unreachable here. Mirrors the stub in `airway_run_test`.
struct TmpWorkspace {
    root: PathBuf,
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
}

/// One scalar column off a single-row query, as text (`NULL` → `None`).
async fn scalar(db: &DatabaseConnection, sql: &str, id: &str) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [id.into()],
    ))
    .await
    .unwrap()
    .and_then(|r| r.try_get::<Option<String>>("", "v").unwrap())
}

/// Seed a resumable Toast backfill that crashed mid-window: an `airway` run marked
/// `failed`, a `resume_state` cursor persisted through the real run-scoped store, a
/// `scope_owned` (backfill-chunk) task left `failed`, and a prior error event.
/// Returns the run id.
async fn seed_crashed_backfill(db: &Arc<DatabaseConnection>) -> String {
    let conn = db.as_ref();
    let run_id = format!("aw-rip-{}", uuid::Uuid::new_v4());
    crud::insert_run(conn, &run_id, "Q", None, "airway", None, uuid::Uuid::nil())
        .await
        .unwrap();
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO airway_run_extensions \
             (run_id, pipeline_name, concurrency, resources, retry_count) \
         VALUES ($1, 'p', 1, '[]'::jsonb, 0)",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();

    // The cursor run 1 committed mid-window, persisted exactly as the worker
    // would — under the seeded run's own workspace (nil, as `insert_run` above).
    let store =
        AirwayRunScopedStateStore::new(Arc::clone(db), run_id.clone(), uuid::Uuid::nil(), "p");
    let mut state = PipelineState::default();
    state.resource_states.insert(
        "orders".to_string(),
        ResourceState {
            incremental: None,
            custom: HashMap::from([(
                "__connector_state".to_string(),
                json!({ "high_water": "2026-06-15" }),
            )]),
        },
    );
    store.save(&state, &Schema::new("p"), 0).await.unwrap();

    // A backfill chunk's task is scope_owned; the crash left it failed.
    crud::enqueue_task(
        conn,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        },
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .unwrap();
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status='failed' WHERE task_id=$1",
        [run_id.clone().into()],
    ))
    .await
    .unwrap();

    // Prior attempt: a failed run with an error + an error event on the feed.
    crud::insert_event(
        conn,
        &run_id,
        0,
        "pipeline_error",
        &json!({ "error": "boom" }),
        0,
    )
    .await
    .unwrap();
    crud::update_run_failed(conn, &run_id, "boom")
        .await
        .unwrap();

    run_id
}

/// Reset-in-place retry of a crashed resumable backfill: revives the SAME run
/// (re-queued, globally claimable, error + events cleared, `retry_count` bumped)
/// while PRESERVING `resume_state`, and the run-scoped store reloads that cursor —
/// so the re-drive resumes mid-window instead of re-pulling the whole window.
#[tokio::test(flavor = "multi_thread")]
async fn reset_in_place_retry_preserves_resume_state_for_resume() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let run_id = seed_crashed_backfill(&db).await;

    // Preconditions: scoped+failed task, failed run, cursor + event present.
    assert_eq!(
        scalar(
            &db,
            "SELECT (scope_owned)::text AS v FROM agentic_task_queue WHERE task_id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("true"),
        "seed: a backfill chunk's task is scope_owned"
    );

    // Retry through the real entry point. workspace_id matches the seeded run
    // (Uuid::nil); the workspace stub is unused on the reset-in-place path.
    let ws = TmpWorkspace {
        root: std::env::temp_dir(),
    };
    let returned = agentic_pipeline::retry::retry_run(&db, uuid::Uuid::nil(), &ws, &run_id)
        .await
        .expect("retry_run");
    assert_eq!(
        returned, run_id,
        "reset-in-place retries the SAME run_id (no clone)"
    );

    // The run's task is revived and made globally claimable so the coordinator
    // re-drives it (a scope_owned task would never be re-claimed).
    assert_eq!(
        scalar(
            &db,
            "SELECT queue_status AS v FROM agentic_task_queue WHERE task_id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("queued"),
        "task must be back on the queue"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT (scope_owned)::text AS v FROM agentic_task_queue WHERE task_id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("false"),
        "task must be globally claimable after retry (mark_task_global)"
    );

    // The run is running again with the failed attempt's error cleared.
    assert_eq!(
        scalar(
            &db,
            "SELECT task_status AS v FROM agentic_runs WHERE id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("running"),
        "run must be running again"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT error_message AS v FROM agentic_runs WHERE id=$1",
            &run_id,
        )
        .await,
        None,
        "the prior attempt's error must be cleared"
    );

    // retry_count bumped; the failed attempt's events dropped.
    assert_eq!(
        scalar(
            &db,
            "SELECT retry_count::text AS v FROM airway_run_extensions WHERE run_id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("1"),
        "retry_count must be bumped to 1"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::text AS v FROM agentic_run_events WHERE run_id=$1",
            &run_id,
        )
        .await
        .as_deref(),
        Some("0"),
        "the prior attempt's events must be cleared"
    );

    // THE POINT: resume_state survived the retry, and the run-scoped store reloads
    // it — this is exactly what the re-driven run's source receives as prior_state,
    // so it resumes from 2026-06-15 rather than re-extracting the whole window.
    let store =
        AirwayRunScopedStateStore::new(Arc::clone(&db), run_id.clone(), uuid::Uuid::nil(), "p");
    let snap = store.load().await.unwrap();
    assert_eq!(
        snap.state
            .resource_states
            .get("orders")
            .and_then(|rs| rs.custom.get("__connector_state"))
            .and_then(|v| v.get("high_water"))
            .and_then(|v| v.as_str()),
        Some("2026-06-15"),
        "resume_state cursor must survive reset-in-place retry so the re-drive resumes"
    );
}
