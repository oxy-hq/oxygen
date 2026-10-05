//! Every recovery entry point asks the `RunPlatformResolver` which platform to
//! drive each root with, drives it with THAT platform, and does not drive a
//! root the resolver could not answer for (I7).
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(run_platform_resolver_test)'

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_pipeline::automation_run::{StartAutomationRequest, start_automation_run};
use agentic_pipeline::platform::{PlatformContext, RunPlatformResolver};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::automation_recovery_test::{FakePlatform, test_db};

/// The platform the resolver hands out: it records every automation ref it is
/// asked for, which is how the test knows a drive used it and not the base.
#[derive(Default)]
pub(crate) struct Marked {
    pub(crate) resolved: Mutex<Vec<String>>,
}

#[async_trait]
impl agentic_pipeline::platform::ProjectContext for Marked {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _: Option<&str>,
        _: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _: &str) -> Option<String> {
        None
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for Marked {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("marked: {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("marked: {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        workflow_ref: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        self.resolved.lock().unwrap().push(workflow_ref.to_string());
        Err("marked: the test only needs to see the ask".into())
    }
}

/// Hands every root the `Marked` platform except `refuse`, which it cannot
/// answer for.
struct Recording {
    seen: Mutex<Vec<String>>,
    refuse: String,
    marked: Arc<Marked>,
}

#[async_trait]
impl RunPlatformResolver for Recording {
    async fn platform_for(
        &self,
        root: &agentic_runtime::entity::run::Model,
        _base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        self.seen.lock().unwrap().push(root.id.clone());
        if root.id == self.refuse {
            return Err("registry lookup failed".into());
        }
        Ok(self.marked.clone())
    }
}

pub(crate) fn request(workflow_ref: &str) -> StartAutomationRequest {
    StartAutomationRequest {
        workflow_ref: workflow_ref.into(),
        variables: None,
        retry_from_run_id: None,
        cache_enabled: false,
        invalidate_steps: None,
        invalidate_iterations: None,
        thread_id: None,
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
    }
}

/// Two Global automation runs in a fresh workspace: one the resolver serves,
/// one it refuses.
async fn two_runs(db: &DatabaseConnection, tag: &str) -> (Uuid, String, String, Arc<Recording>) {
    let ws = Uuid::new_v4();
    let seed = |r: String| async move {
        start_automation_run(db, request(&r), crud::TaskScope::Global, ws)
            .await
            .expect("seed run")
    };
    let served = seed(format!("{tag}_served.procedure.yml")).await;
    let refused = seed(format!("{tag}_refused.procedure.yml")).await;
    let resolver = Arc::new(Recording {
        seen: Mutex::new(vec![]),
        refuse: refused.clone(),
        marked: Arc::new(Marked::default()),
    });
    (ws, served, refused, resolver)
}

/// The resolver was asked about both roots, the served root was driven with
/// the platform it returned, and the refused root was left undriven.
async fn assert_resolved_and_used(
    db: &DatabaseConnection,
    tag: &str,
    served: &str,
    refused: &str,
    r: &Recording,
) {
    let seen = r.seen.lock().unwrap().clone();
    assert!(
        seen.contains(&served.to_string()),
        "{tag}: resolver not asked about the served root: {seen:?}"
    );
    assert!(
        seen.contains(&refused.to_string()),
        "{tag}: resolver not asked about the refused root: {seen:?}"
    );
    let want = format!("{tag}_served.procedure.yml");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !r.marked.resolved.lock().unwrap().contains(&want) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{tag}: the served root was not driven with the resolved platform"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let refused_ref = format!("{tag}_refused.procedure.yml");
    assert!(
        !r.marked.resolved.lock().unwrap().contains(&refused_ref),
        "{tag}: the refused root was driven"
    );
    let row = crud::get_run(db, refused)
        .await
        .unwrap()
        .expect("refused run");
    assert_eq!(
        row.driver_id, None,
        "{tag}: the refused root took the driver lease"
    );
    assert_eq!(
        row.attempt, 0,
        "{tag}: the refused root spent recovery budget"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_uses_the_run_platform_resolver_for_every_root() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let base: Arc<dyn PlatformContext> = Arc::new(FakePlatform);
    let router = || -> Arc<dyn agentic_runtime::router::TaskRouter> {
        Arc::new(agentic_runtime::router::NoopTaskRouter)
    };
    let state = || Arc::new(RuntimeState::new());

    // The latency worker's path.
    let (ws, served, refused, r) = two_runs(&db, "pending").await;
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        state(),
        base.clone(),
        r.clone(),
        None,
        None,
        None,
        None,
        router(),
        Some(ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_resolved_and_used(&db, "pending", &served, &refused, &r).await;

    // The periodic tick's path: a queued Global run nobody claimed for longer
    // than the grace window is stranded.
    let (ws, served, refused, r) = two_runs(&db, "stranded").await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET updated_at = now() - interval '10 minutes' WHERE workspace_id = $1",
        [ws.into()],
    ))
    .await
    .unwrap();
    agentic_pipeline::recovery::recover_stranded_runs(
        db.clone(),
        state(),
        base.clone(),
        r.clone(),
        None,
        None,
        None,
        None,
        router(),
        Some(ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_resolved_and_used(&db, "stranded", &served, &refused, &r).await;

    // The one-shot startup pass.
    let (ws, served, refused, r) = two_runs(&db, "startup").await;
    agentic_pipeline::recovery::recover_active_runs(
        db.clone(),
        state(),
        base.clone(),
        r.clone(),
        None,
        None,
        None,
        None,
        router(),
        Some(ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;
    assert_resolved_and_used(&db, "startup", &served, &refused, &r).await;
}
