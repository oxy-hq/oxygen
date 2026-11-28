//! The headless eval on a node with no working copy — a serve or worker pod,
//! where Workspace Health runs its agent probe. No database, no LLM: every case
//! here settles at the agent load or at the solver build right after it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{FsmTools, HeadlessError, run_agentic_eval};
use crate::platform::{PlatformContext, ProjectContext};

const AGENT_REF: &str = "_analytics.agentic.yml";

/// A stateless replica: no checkout, and a compile boundary that serves
/// `compiled` for [`AGENT_REF`] and nothing for any other ref.
struct Replica {
    compiled: Option<&'static str>,
    /// Whether this host provides the metric-tree runner and the anomaly
    /// store, as a request-built `OxyProjectContext` with a subject, a role
    /// and a database does.
    equipped: bool,
}

/// The workspace an equipped [`Replica`] answers for.
const WORKSPACE: uuid::Uuid = uuid::Uuid::from_u128(0x4545);

#[async_trait::async_trait]
impl ProjectContext for Replica {
    async fn resolve_agent(
        &self,
        agent_id: &str,
    ) -> Result<Option<String>, agentic_automation::WorkspaceReadError> {
        Ok((agent_id == AGENT_REF)
            .then_some(self.compiled)
            .flatten()
            .map(str::to_string))
    }
    async fn resolve_connector(&self, _db: &str) -> Option<agentic_connector::ConnectorConfig> {
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
    fn workspace_id(&self) -> uuid::Uuid {
        if self.equipped {
            WORKSPACE
        } else {
            uuid::Uuid::nil()
        }
    }
    fn metric_tree_runner(&self) -> Option<Arc<dyn agentic_analytics::MetricTreeRunner>> {
        self.equipped
            .then(|| Arc::new(fakes::NoRunner) as Arc<dyn agentic_analytics::MetricTreeRunner>)
    }
    fn anomaly_store(&self) -> Option<Arc<dyn agentic_analytics::anomaly_store::AnomalyStore>> {
        self.equipped.then(|| {
            Arc::new(fakes::NoAnomalies) as Arc<dyn agentic_analytics::anomaly_store::AnomalyStore>
        })
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for Replica {
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
    async fn resolve_automation_yaml(&self, _r: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
}

fn replica(compiled: Option<&'static str>) -> Arc<dyn PlatformContext> {
    Arc::new(Replica {
        compiled,
        equipped: false,
    })
}

/// THE case: the agent is in the compiled revision and there is no file to
/// read. The eval must load it from the revision and get as far as building
/// the solver — which then fails for the fake's reason (it resolves no
/// connector), not for a missing file.
#[tokio::test]
async fn an_agent_in_the_compiled_revision_loads_without_a_working_copy() {
    let err = run_agentic_eval(
        replica(Some("databases:\n  - warehouse\n")),
        AGENT_REF,
        "How many orders?".into(),
    )
    .await
    .expect_err("the fake resolves no connector, so the run cannot build");

    let HeadlessError::Run(msg) = &err else {
        panic!("the agent was not loaded from the compiled revision: {err}");
    };
    assert!(msg.contains("no databases configured"), "{msg}");
}

/// The control: an agent the revision does not hold, on a node with nothing to
/// fall back to, still fails at the load — so the case above is not passing
/// because the load stopped being checked.
#[tokio::test]
async fn an_agent_the_revision_does_not_hold_fails_at_the_load() {
    let err = run_agentic_eval(replica(None), AGENT_REF, "q".into())
        .await
        .expect_err("nothing serves this agent");

    assert!(matches!(err, HeadlessError::Load(_)), "got {err:?}");
    assert!(
        err.to_string().contains(AGENT_REF),
        "names the agent: {err}"
    );
}

/// A ref that would walk out of the workspace root on the disk fallback — an
/// absolute path, a `..` — is refused before anything is read, the containment
/// `ConfigManager::resolve_file` gave the probe when it went through it.
#[tokio::test]
async fn a_ref_outside_the_workspace_is_refused_before_the_load() {
    for bad in [
        "../_analytics.agentic.yml",
        "/etc/_analytics.agentic.yml",
        "agents/../../x",
        "",
    ] {
        let err = run_agentic_eval(replica(Some("databases: []\n")), bad, "q".into())
            .await
            .expect_err("refused");
        assert!(matches!(err, HeadlessError::Load(_)), "{bad:?}: {err:?}");
        assert!(
            err.to_string().contains("inside the workspace"),
            "{bad:?}: {err}"
        );
    }
}

/// A definition the revision holds that does not parse is the agent being
/// broken, not the probe being unable to run: it must not come back as
/// `Load`, which the health probe reports as Degraded rather than Unhealthy.
#[tokio::test]
async fn a_definition_that_does_not_parse_is_not_a_load_failure() {
    let err = run_agentic_eval(
        replica(Some("databases: [unterminated")),
        AGENT_REF,
        "q".into(),
    )
    .await
    .expect_err("the definition does not parse");

    assert!(matches!(err, HeadlessError::Invalid(_)), "got {err:?}");
    assert!(err.to_string().contains("YAML parse error"), "{err}");
}

/// G5: Slack's streaming run takes a ref, and on a node with no files it reads
/// the agent from the compiled revision — the same load the eval does.
#[tokio::test]
async fn a_streaming_run_reads_its_agent_from_the_revision() {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let err = super::run_agentic_streaming(
        replica(Some("databases:\n  - warehouse\n")),
        AGENT_REF,
        "q".into(),
        tx,
    )
    .await
    .expect_err("the fake resolves no connector");
    assert!(err.contains("no databases configured"), "{err}");
}

/// A stored classic path (a Slack preference saved before retirement) names
/// no compiled agent: the run fails at the load, naming the ref, rather than
/// starting with some other agent.
#[tokio::test]
async fn a_streaming_run_of_a_classic_path_is_a_load_failure() {
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let err = super::run_agentic_streaming(
        replica(Some("databases: []\n")),
        "agents/sales.agent.yml",
        "q".into(),
        tx,
    )
    .await
    .expect_err("no compiled agent has that path");
    assert!(err.contains("failed to load agentic config"), "{err}");
    assert!(err.contains("agents/sales.agent.yml"), "{err}");
}

/// G6: the librarian's one-turn answer reads its agent by ref from the
/// revision. The fake resolves no model, so getting that far is the proof
/// the definition loaded without a file.
#[tokio::test]
async fn a_one_turn_answer_reads_its_agent_from_the_revision() {
    let err = super::run_agentic_answer(
        replica(Some("instructions: be brief\n")),
        AGENT_REF,
        "q".into(),
    )
    .await
    .expect_err("the fake resolves no model");
    assert!(err.contains("no model configured"), "{err}");

    let err = super::run_agentic_answer(replica(None), AGENT_REF, "q".into())
        .await
        .expect_err("nothing serves this agent");
    assert!(err.contains("failed to load agentic config"), "{err}");
}

/// A headless FSM run's solver gets the tools a served chat's solver gets
/// (`start_pipeline`): the metric-tree runner and the anomaly store the
/// platform provides, and the platform's workspace, which scopes every anomaly
/// read and write. An eval of a "why did revenue drop" case otherwise has no
/// `explain` and no `list_anomalies` to call.
#[tokio::test]
async fn a_headless_fsm_solver_gets_the_metric_tree_and_anomaly_tools_the_platform_provides() {
    let platform = Replica {
        compiled: None,
        equipped: true,
    };

    let solver = FsmTools::of(&platform).equip(bare_solver().await);

    assert!(solver.has_metric_tree_runner(), "no metric-tree runner");
    assert!(solver.has_anomaly_store(), "no anomaly store");
    assert_eq!(solver.workspace_id(), WORKSPACE);
}

/// The control: a platform that provides neither (the CLI, a Workspace Health
/// probe with no subject) leaves the solver without either, so the case above
/// is the platform's answer forwarded, not a default.
#[tokio::test]
async fn a_platform_without_the_tools_leaves_the_headless_solver_without_them() {
    let platform = Replica {
        compiled: None,
        equipped: false,
    };

    let solver = FsmTools::of(&platform).equip(bare_solver().await);

    assert!(!solver.has_metric_tree_runner());
    assert!(!solver.has_anomaly_store());
    assert_eq!(solver.workspace_id(), uuid::Uuid::nil());
}

/// A real FSM solver with no tools attached, over one in-memory connector.
async fn bare_solver() -> agentic_analytics::AnalyticsSolver {
    let config = agentic_analytics::AgentConfig::from_yaml("instructions: be brief\n")
        .expect("the config parses");
    let connector: Arc<dyn agentic_connector::DatabaseConnector> =
        Arc::new(crate::react_run::tests::Five);
    let ctx = agentic_analytics::BuildContext {
        extra_connectors: [("warehouse".to_string(), connector)].into(),
        extra_default_connector: Some("warehouse".to_string()),
        ..Default::default()
    };
    let (solver, _) = config
        .build_solver_with_context(Path::new("."), ctx)
        .await
        .expect("the solver builds");
    solver
}

/// Stand-ins for the two ports: present is all the tests above ask of them.
mod fakes {
    use agentic_analytics::MetricTreeRunnerError;
    use agentic_analytics::anomaly_store::{
        AnomalyFilter, AnomalyRecord, AnomalyStore, AnomalyStoreError, DetectAndUpsertResult,
    };
    use oxy_airlayer_compat::engine::metric_tree_ops::{
        BenchmarkStatistic, ExplainConfig, ExplainResult, OpportunityResult,
    };
    use oxy_airlayer_compat::engine::query::QueryFilter;
    use oxy_airlayer_compat::{DatabaseConfig, SemanticLayer};

    fn unused() -> MetricTreeRunnerError {
        MetricTreeRunnerError::Op("not called in these tests".into())
    }

    pub(super) struct NoRunner;

    #[async_trait::async_trait]
    impl agentic_analytics::MetricTreeRunner for NoRunner {
        async fn load_layer(&self) -> Result<SemanticLayer, MetricTreeRunnerError> {
            Err(unused())
        }
        async fn list_databases(&self) -> Vec<DatabaseConfig> {
            vec![]
        }
        async fn run_explain(
            &self,
            _: String,
            _: String,
            _: (String, String),
            _: (String, String),
            _: Vec<QueryFilter>,
            _: ExplainConfig,
        ) -> Result<ExplainResult, MetricTreeRunnerError> {
            // bca: suppress(nargs) -- the MetricTreeRunner trait fixes this signature
            Err(unused())
        }
        async fn run_opportunity(
            &self,
            _: String,
            _: String,
            _: (String, String),
            _: BenchmarkStatistic,
        ) -> Result<OpportunityResult, MetricTreeRunnerError> {
            Err(unused())
        }
        async fn get_dimension_values(
            &self,
            _: String,
            _: String,
            _: u32,
        ) -> Result<Vec<String>, MetricTreeRunnerError> {
            Err(unused())
        }
        async fn run_time_series(
            &self,
            _: String,
            _: String,
            _: String,
            _: (String, String),
            _: Vec<QueryFilter>,
            _: Option<String>,
        ) -> Result<Vec<(String, f64)>, MetricTreeRunnerError> {
            // bca: suppress(nargs) -- the MetricTreeRunner trait fixes this signature
            Err(unused())
        }
    }

    pub(super) struct NoAnomalies;

    #[async_trait::async_trait]
    impl AnomalyStore for NoAnomalies {
        async fn list(
            &self,
            _: uuid::Uuid,
            _: AnomalyFilter,
        ) -> Result<Vec<AnomalyRecord>, AnomalyStoreError> {
            Ok(vec![])
        }
        async fn get(
            &self,
            _: uuid::Uuid,
            _: uuid::Uuid,
        ) -> Result<Option<AnomalyRecord>, AnomalyStoreError> {
            Ok(None)
        }
        async fn detect_and_upsert(
            &self,
            _: uuid::Uuid,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<(String, f64)>,
        ) -> Result<DetectAndUpsertResult, AnomalyStoreError> {
            Err(AnomalyStoreError::Detection(
                "not called in these tests".into(),
            ))
        }
        async fn get_explain_cache(
            &self,
            _: uuid::Uuid,
            _: uuid::Uuid,
        ) -> Result<Option<serde_json::Value>, AnomalyStoreError> {
            Ok(None)
        }
        async fn set_explain_cache(
            &self,
            _: uuid::Uuid,
            _: uuid::Uuid,
            _: serde_json::Value,
        ) -> Result<(), AnomalyStoreError> {
            Ok(())
        }
    }
}
