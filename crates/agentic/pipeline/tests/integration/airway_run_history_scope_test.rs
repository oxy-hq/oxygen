//! Regression: an airway pipeline's run history belongs to one workspace.
//!
//! `list_airway_runs` backs `GET /{workspace_id}/agentic-airway/runs?pipeline_ref=`
//! and used to filter on `pipeline_ref` alone. A ref is a workspace-relative
//! path, and the same path (`pipelines/toast.airway.yml`) is common across
//! workspaces, so a member of one workspace was shown another workspace's run
//! ids, statuses, timestamps and backfill windows for a pipeline at that path.
//!
//! Seeds through the real submit path (`start_airway_run`) rather than raw
//! inserts, so the workspace id the listing filters on is the one production
//! stamps — not one this test assumes.
//!
//! Requires Docker (or `OXY_DATABASE_URL`). Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(airway_run_history_scope_test)'

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_pipeline::AirwayMigrator;
use agentic_pipeline::airway_run::{StartAirwayRequest, list_airway_runs, start_airway_run};
use agentic_runtime::migration::RuntimeMigrator;
use async_trait::async_trait;
use sea_orm::{Database, DatabaseConnection};
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
                            // Must match at every setup site — reuse hashes the
                            // config. See internal-docs/workspace-source.md.
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

    let db = Database::connect(&url)
        .await
        .expect("failed to connect to test DB");
    // Central first (`start_airway_run` resolves admission against the central
    // `airway_source_config` table), then runtime, then airway — production
    // order; see oxy_test_utils::migration.
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

/// A workspace whose `.airway.yml` is a compiled row: every ref resolves to
/// `yaml`, and there is no working copy. Everything else is unreachable on the
/// submit path.
struct CompiledWorkspace {
    yaml: String,
}

#[async_trait]
impl agentic_automation::WorkspaceContext for CompiledWorkspace {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!(
            "compiled workspace: connector '{name}' unavailable"
        ))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!(
            "compiled workspace: integration '{name}' unavailable"
        ))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _automation_ref: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("compiled workspace: not available".into())
    }
    async fn resolve_pipeline_yaml(&self, _pipeline_ref: &str) -> Result<Option<String>, String> {
        Ok(Some(self.yaml.clone()))
    }
}

fn request(pipeline_ref: &str) -> StartAirwayRequest {
    StartAirwayRequest {
        pipeline_ref: pipeline_ref.to_string(),
        variables: None,
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

async fn listed_run_ids(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
) -> Vec<String> {
    list_airway_runs(db, workspace_id, pipeline_ref, 50)
        .await
        .expect("list_airway_runs")
        .into_iter()
        .map(|r| r.run_id)
        .collect()
}

/// Two workspaces, one pipeline path: each listing returns only its own run.
#[tokio::test(flavor = "multi_thread")]
async fn run_history_does_not_leak_across_workspaces_sharing_a_pipeline_ref() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    // The realistic collision: the same conventional path in both workspaces.
    // Fresh workspace ids keep the reused test DB's accumulated rows out.
    let pipeline_ref = "pipelines/toast.airway.yml";
    let pipeline_name = format!("history_scope_{}", Uuid::new_v4().simple());
    let ws = CompiledWorkspace {
        yaml: format!(
            r#"
name: {pipeline_name}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-history-scope
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
        ),
    };
    let workspace_a = Uuid::new_v4();
    let workspace_b = Uuid::new_v4();

    let run_a = start_airway_run(
        &db,
        &ws,
        request(pipeline_ref),
        agentic_pipeline::TaskScope::Global,
        workspace_a,
    )
    .await
    .expect("start run in workspace A");
    let run_b = start_airway_run(
        &db,
        &ws,
        request(pipeline_ref),
        agentic_pipeline::TaskScope::Global,
        workspace_b,
    )
    .await
    .expect("start run in workspace B");
    assert_ne!(
        run_a, run_b,
        "precondition: submits in different workspaces must be distinct runs"
    );

    assert_eq!(
        listed_run_ids(&db, workspace_a, pipeline_ref).await,
        vec![run_a.clone()],
        "workspace A must see its own run and not workspace B's"
    );
    assert_eq!(
        listed_run_ids(&db, workspace_b, pipeline_ref).await,
        vec![run_b.clone()],
        "workspace B must see its own run and not workspace A's"
    );
}
