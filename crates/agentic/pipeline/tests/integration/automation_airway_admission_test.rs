//! An airway step **inside an automation** must be admitted under the same
//! `airway_source_config` policy a scheduled or manual run of the same
//! pipeline gets.
//!
//! Before the `AirwayAdmissionResolver` port existed, both automation dispatch
//! sites hard-coded `contract_policy: None, environment: None` on the
//! `TaskSpec::Airway` they queued, so an operator who set `require_declared`
//! for a source kind in the admin UI had it applied to manual and scheduled
//! runs and **silently ignored** by any airway step inside an automation — the
//! run fell back to airway's built-in `permissive` / `production` default.
//!
//! `resolver_absent_is_the_pre_port_bug` pins the exact shape of that bug, and
//! `an_automation_airway_step_carries_the_resolved_admission` is the assertion
//! that fails without the fix.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_airway::AirwayMigrator;
use agentic_automation::config::{AirwayConfig, AutomationConfig, TaskConfig, TaskType};
use agentic_automation::extension::AutomationRunState;
use agentic_automation::{AutomationDecider, AutomationDecision};
use agentic_core::delegation::TaskSpec;
use agentic_pipeline::airway_config::PipelineAirwayAdmissionResolver;
use agentic_runtime::migration::RuntimeMigrator;
use async_trait::async_trait;
use entity::airway_source_config;
use entity::workspaces::{self, WorkspaceStatus};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use serde_json::json;
use uuid::Uuid;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

/// Same fixture as `airway_config_test` / `airway_run_test`: central migrator
/// first (it owns `airway_source_config`), then `AirwayMigrator`.
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

    let db = Database::connect(&url).await.ok()?;
    // Central → runtime → domain, in production order. The token returned by
    // `migrate_shared_test_db` has no public constructor, so a domain migrator
    // can only run on proof the first two already did.
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

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("automation-admission-test-{id}")),
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
        default_branch: Set(None),
        repo_subdir: Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

/// Workspace whose `.airway.yml` is served from the compile boundary, with no
/// working copy on disk at all — the stateless-replica shape the durable
/// worker actually runs in.
struct CompiledOnlyWorkspace {
    root: PathBuf,
    pipeline_yaml: String,
}

#[async_trait]
impl agentic_automation::WorkspaceContext for CompiledOnlyWorkspace {
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
        Err(format!("connector '{name}' unavailable"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("integration '{name}' unavailable"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("not available".into())
    }
    async fn resolve_pipeline_yaml(&self, _pipeline_ref: &str) -> Result<Option<String>, String> {
        Ok(Some(self.pipeline_yaml.clone()))
    }
}

const PIPELINE_REF: &str = "pipelines/sales.airway.yml";

/// A one-task automation whose only step is `type: airway`.
fn airway_automation_state() -> AutomationRunState {
    AutomationRunState {
        run_id: format!("automation-admission-{}", Uuid::new_v4()),
        workflow: AutomationConfig {
            name: "admission".into(),
            tasks: vec![TaskConfig {
                name: "load_sales".into(),
                task_type: TaskType::Airway(AirwayConfig {
                    pipeline: PIPELINE_REF.to_string(),
                    resources: None,
                }),
                export: None,
                cache: None,
            }],
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        },
        workflow_yaml_hash: String::new(),
        workflow_context: json!({}),
        variables: None,
        trace_id: "admission-trace".into(),
        current_step: 0,
        results: HashMap::new(),
        render_context: json!({}),
        pending_children: HashMap::new(),
        decision_version: 0,
        step_hashes: HashMap::new(),
        retry_from_run_id: None,
        cache_enabled: false,
        prior_step_hashes: HashMap::new(),
        prior_results: HashMap::new(),
        initial_render_context: json!({}),
        invalidate_iterations: HashMap::new(),
    }
}

/// Pull the queued `TaskSpec::Airway` out of the decider's decision.
fn airway_spec(decision: AutomationDecision) -> TaskSpec {
    match decision {
        AutomationDecision::DelegateStep { spec, .. } => {
            assert!(
                matches!(spec, TaskSpec::Airway { .. }),
                "an `airway` step must delegate a TaskSpec::Airway, got {spec:?}"
            );
            spec
        }
        other => panic!("expected DelegateStep for the airway step, got {other:?}"),
    }
}

/// The regression this port exists for.
///
/// Seed a **global** `airway_source_config` row plus a **sparse workspace
/// override** that sets only `environment`, so the assertion also proves the
/// automation path goes through the real field-by-field merge (inheriting
/// `contract_policy` from the global row) rather than reading one row whole.
///
/// The two values are deliberately distinguishable, so a transposition bug
/// (`contract_policy: admission.environment` or vice versa) fails loudly
/// instead of passing on a symmetric fixture.
#[tokio::test(flavor = "multi_thread")]
async fn an_automation_airway_step_carries_the_resolved_admission() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let workspace_id = seed_workspace(&db).await;
    let source_kind = format!("toast-{}", Uuid::new_v4());

    airway_source_config::ActiveModel {
        source_kind: Set(source_kind.clone()),
        workspace_id: Set(None),
        contract_policy: Set(Some("require_declared".to_string())),
        environment: Set(Some("production".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed global row");

    airway_source_config::ActiveModel {
        source_kind: Set(source_kind.clone()),
        workspace_id: Set(Some(workspace_id)),
        // Sparse: `contract_policy` omitted, so it must be inherited.
        contract_policy: Set(None),
        environment: Set(Some("sandbox".to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace override");

    let workspace = Arc::new(CompiledOnlyWorkspace {
        // Deliberately nonexistent: the admission must resolve through the
        // compile boundary, never a working copy.
        root: PathBuf::from("/nonexistent-oxy-workspace/automation-admission"),
        pipeline_yaml: format!(
            r#"
name: sales
source:
  kind: {source_kind}
  config:
    base_path: /tmp/airway-automation-admission
    pattern: "*.jsonl"
    format: jsonl
    table_name: orders
destination:
  kind: memory
  config:
    dataset_name: scratch
concurrency: 1
resources:
  - orders
"#
        ),
    });

    let decider = AutomationDecider::new(None).with_airway_admission_resolver(Arc::new(
        PipelineAirwayAdmissionResolver::new(db.clone(), workspace, workspace_id),
    ));

    let (_state, decision) = decider
        .decide(airway_automation_state(), None, None, None)
        .await;

    match airway_spec(decision) {
        TaskSpec::Airway {
            pipeline_ref,
            contract_policy,
            environment,
            ..
        } => {
            assert_eq!(pipeline_ref, PIPELINE_REF);
            assert_eq!(
                contract_policy.as_deref(),
                Some("require_declared"),
                "an automation-dispatched airway run must carry the resolved \
                 contract_policy (inherited from the global row), not None"
            );
            assert_eq!(
                environment.as_deref(),
                Some("sandbox"),
                "the sparse workspace override must win for the field it sets"
            );
        }
        other => panic!("expected TaskSpec::Airway, got {other:?}"),
    }
}

/// Without a resolver injected the dispatch site still queues both fields as
/// `None` — airway's `permissive` / `production` default. This is the exact
/// pre-port behaviour, kept as the control for the test above and as the
/// contract for hosts that wire no resolver (the inline Data-App runner).
#[tokio::test(flavor = "multi_thread")]
async fn resolver_absent_is_the_pre_port_bug() {
    let decider = AutomationDecider::new(None);
    let (_state, decision) = decider
        .decide(airway_automation_state(), None, None, None)
        .await;

    match airway_spec(decision) {
        TaskSpec::Airway {
            contract_policy,
            environment,
            ..
        } => {
            assert_eq!(contract_policy, None);
            assert_eq!(environment, None);
        }
        other => panic!("expected TaskSpec::Airway, got {other:?}"),
    }
}

/// A resolver that cannot answer must **fail the step**, not queue it under a
/// silently-defaulted `permissive`. A tightened policy quietly not applying is
/// indistinguishable in the data from a deployment that never set one — the
/// failure this whole surface exists to make impossible.
#[tokio::test(flavor = "multi_thread")]
async fn an_unresolvable_admission_fails_the_step_rather_than_defaulting() {
    struct Broken;
    #[async_trait]
    impl agentic_automation::AirwayAdmissionResolver for Broken {
        async fn resolve_for_pipeline(
            &self,
            _pipeline_ref: &str,
        ) -> Result<agentic_core::delegation::ResolvedAdmission, String> {
            Err("airway_source_config unreadable".into())
        }
    }

    let decider = AutomationDecider::new(None).with_airway_admission_resolver(Arc::new(Broken));
    let (_state, decision) = decider
        .decide(airway_automation_state(), None, None, None)
        .await;

    match decision {
        AutomationDecision::Fail { error, .. } => {
            assert!(
                error.contains("airway_source_config unreadable"),
                "the underlying cause must survive into the step error: {error}"
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}
