//! A chat run is never lease-free while it starts.
//!
//! `PipelineBuilder::start` inserts the run row early, and the start can then
//! take tens of seconds (a cold DuckDB connector build) before
//! `drive_with_coordinator` exists to take the driver lease. The one-shot
//! startup pass (`recover_active_runs`, which runs a second time
//! `DRIVER_LEASE_TTL_SECS` after boot) selects any `running` root with no live
//! lease as orphaned. On 23 Oct it found run `3f3c24b6…` mid-start, re-launched
//! it, failed it as "has no checkpoint — resubmit the question" (sending the
//! asker a failure notice), and the start then drove the same run to `done`.
//! The start now stamps the lease the moment it inserts the row.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(starting_run_recovery_test)'

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext, ProjectContext};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::automation_recovery_test::test_db;

/// A workspace whose agent reads one database, and whose connector lookup —
/// the start's step after the insert — waits for the test to open `gate`.
struct HeldStart {
    reached: Arc<Notify>,
    gate: Arc<Notify>,
}

#[async_trait::async_trait]
impl ProjectContext for HeldStart {
    async fn resolve_agent_yaml(&self, _agent_id: &str) -> Option<String> {
        Some("databases:\n  - warehouse\n".into())
    }
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
        self.reached.notify_one();
        self.gate.notified().await;
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _explicit: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var: &str) -> Option<String> {
        None
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for HeldStart {
    fn workspace_path(&self) -> Option<&Path> {
        Some(Path::new(""))
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
}

async fn run_in(db: &DatabaseConnection, ws: Uuid) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id FROM agentic_runs WHERE workspace_id = $1",
        [ws.into()],
    ))
    .await
    .unwrap()
    .map(|r| r.try_get("", "id").unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_startup_pass_leaves_a_run_that_is_still_starting_alone() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let (reached, gate) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let platform = Arc::new(HeldStart {
        reached: reached.clone(),
        gate: gate.clone(),
    });

    // A chat question, held mid-start right after its row was inserted.
    let start = {
        let (db, platform) = (db.clone(), platform.clone());
        tokio::spawn(async move {
            PipelineBuilder::new(platform)
                .workspace_id(ws)
                .question("Which oxymart stores sell the most?")
                .analytics("agents/a.agentic.yml")
                .start(&db)
                .await
                .map(|started| started.run_id)
        })
    };
    tokio::time::timeout(Duration::from_secs(30), reached.notified())
        .await
        .expect("the start reached its connector lookup");
    let run_id = run_in(&db, ws).await.expect("the start inserted its run");

    // The startup pass, as it runs `DRIVER_LEASE_TTL_SECS` after boot.
    let base: Arc<dyn PlatformContext> = platform.clone();
    agentic_pipeline::recovery::recover_active_runs(
        db.clone(),
        Arc::new(RuntimeState::new()),
        base,
        Arc::new(IdentityResolver),
        None,
        None,
        None,
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;

    let row = crud::get_run(&db, &run_id).await.unwrap().expect("run");
    assert_eq!(
        (row.task_status.as_deref(), row.error_message.as_deref()),
        (Some("running"), None),
        "the startup pass retired a run its start was still building"
    );
    assert!(
        row.driver_id
            .as_deref()
            .is_some_and(|d| d.starts_with("direct-")),
        "the start holds the lease: {:?}",
        row.driver_id
    );

    // The start goes on. With no connector it fails as a start does, and
    // that write frees the lease.
    gate.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(30), start)
        .await
        .expect("the start finished")
        .expect("no panic");
    assert!(result.is_err(), "no database: the start fails");
    let row = crud::get_run(&db, &run_id).await.unwrap().expect("run");
    assert_eq!(row.task_status.as_deref(), Some("failed"));
    assert!(
        !row.error_message
            .as_deref()
            .unwrap_or_default()
            .contains("recovery failed"),
        "failed by its own start, not by recovery: {:?}",
        row.error_message
    );
    assert_eq!(row.driver_id, None, "a terminal write clears the lease");
}

/// A start whose caller stops waiting — the request's `TimeoutLayer` firing,
/// or a disconnect — is dropped before any driver exists. On 24 Oct the first
/// ask after a boot timed out at 60 s this way and its run stayed `running`
/// for seven hours with no events and a lease nobody renewed, so reopening the
/// thread resumed a stream that never produced anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_dropped_before_its_driver_fails_its_run_and_frees_the_lease() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let (reached, gate) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let platform = Arc::new(HeldStart {
        reached: reached.clone(),
        gate,
    });

    let start = {
        let (db, platform) = (db.clone(), platform.clone());
        tokio::spawn(async move {
            PipelineBuilder::new(platform)
                .workspace_id(ws)
                .question("Which oxymart stores sell the most?")
                .analytics("agents/a.agentic.yml")
                .start(&db)
                .await
                .map(|started| started.run_id)
        })
    };
    tokio::time::timeout(Duration::from_secs(30), reached.notified())
        .await
        .expect("the start reached its connector lookup");
    let run_id = run_in(&db, ws).await.expect("the start inserted its run");
    let row = crud::get_run(&db, &run_id).await.unwrap().expect("run");
    assert!(row.driver_id.is_some(), "the start holds the lease");

    // The handler's future goes, and the start with it, mid-build.
    start.abort();
    assert!(start.await.unwrap_err().is_cancelled());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let row = loop {
        let row = crud::get_run(&db, &run_id).await.unwrap().expect("run");
        if row.task_status.as_deref() != Some("running") || tokio::time::Instant::now() > deadline {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        (row.task_status.as_deref(), row.error_message.as_deref()),
        (Some("failed"), Some(agentic_pipeline::START_INTERRUPTED)),
        "a dropped start ends its run"
    );
    assert_eq!(
        (row.driver_id, row.driver_heartbeat_at),
        (None, None),
        "and frees its lease"
    );
}
