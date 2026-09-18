//! High-level API for starting and driving agentic pipelines.
//!
//! [`PipelineBuilder`] encapsulates config loading, connector resolution,
//! solver building, and pipeline startup. Both the HTTP layer and the CLI
//! use this crate — no domain logic is duplicated.

pub mod agent_run;
pub mod airway_config;
pub mod airway_run;
pub mod automation_run;
pub mod backfill;
mod db_transient;
pub mod executor;
pub mod pipeline_ref;
pub mod platform;
pub mod recovery;
pub mod retry;
pub mod revert;
pub mod scheduler;
pub mod usage;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_analytics::SchemaCatalog;
use agentic_analytics::config::AgentConfig;
use agentic_builder::{BuilderAppRunner, BuilderTestRunner};
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::handle::{PipelineHandle, PipelineOutcome};
use agentic_runtime::state::RuntimeState;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::platform::{BuilderBridges, PlatformContext, ProjectContext};

// ── Re-exports for consumers ────────────────────────────────────────────────

pub use crate::revert::{RevertedFile, revert_builder_file_changes};
pub use agentic_airway::{
    AirwayMigrator, DiscoveredColumn, DiscoveredTable, SOURCE_TYPE as AIRWAY_SOURCE_TYPE,
    event_handler as airway_event_handler,
};
/// Re-export so HTTP/CLI don't import domain crates directly.
pub use agentic_analytics::AnalyticsRunMeta;
pub use agentic_analytics::SchemaCatalog as AnalyticsSchemaCatalog;
pub use agentic_analytics::extension::AnalyticsMigrator;
pub use agentic_analytics::{AnalyticsMetricSink, SharedMetricSink};
pub use agentic_automation::{
    AutomationMigrator, SOURCE_TYPE as WORKFLOW_SOURCE_TYPE,
    WorkspaceContext as WorkflowWorkspaceContext, WorkspaceReadError,
};
pub use agentic_builder::BuilderAppRunner as BuilderAppRunnerTrait;
pub use agentic_builder::BuilderTestRunner as BuilderTestRunnerTrait;
pub use agentic_builder::KnowledgeCard;
pub use agentic_builder::onboarding;
pub use agentic_core::human_input::{
    AutoAcceptInputProvider, HumanInputHandle, HumanInputProvider,
};
pub use agentic_llm::LlmClient;
pub use agentic_llm::{AnthropicProvider, OpenAiProvider};
/// Re-exported so HTTP/CLI consumers name the seed-function ownership arg
/// through the facade instead of importing `agentic-runtime` directly.
pub use agentic_runtime::crud::TaskScope;

// ── ThinkingMode ────────────────────────────────────────────────────────────

/// Thinking mode preset for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ThinkingMode {
    #[default]
    Auto,
    ExtendedThinking,
}

impl ThinkingMode {
    /// Serialize for DB storage. Always returns `Some` so analytics queries can
    /// filter on `thinking_mode` without special-casing NULL. NULL in the
    /// column means "mode was never set" (e.g. legacy rows), not "Auto".
    pub fn to_db(self) -> Option<String> {
        match self {
            Self::Auto => Some("auto".to_string()),
            Self::ExtendedThinking => Some("extended_thinking".to_string()),
        }
    }

    pub fn is_extended(self) -> bool {
        matches!(self, Self::ExtendedThinking)
    }
}

// ── PipelineBuilder ─────────────────────────────────────────────────────────

/// Builder for starting agentic pipelines.
///
/// Encapsulates all domain-specific setup (config loading, connector
/// resolution, solver building) behind a clean API. Both the HTTP layer
/// and the CLI use this builder.
pub struct PipelineBuilder {
    platform: Arc<dyn PlatformContext>,
    builder_bridges: Option<BuilderBridges>,
    domain: Option<Domain>,
    question: String,
    thread_id: Option<Uuid>,
    thinking_mode: ThinkingMode,
    schema_cache: Option<Arc<Mutex<HashMap<String, SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn BuilderAppRunner>>,
    /// When set, use this run_id and skip the DB `insert_run` call.
    /// Used for delegation children where the coordinator already created
    /// the run via `insert_run_with_parent`.
    existing_run_id: Option<String>,
    /// Override the default human input provider for the builder domain.
    /// When set, passed through to `BuilderPipelineParams.human_input`.
    human_input: Option<agentic_core::human_input::HumanInputHandle>,
    /// Override the default LLM client for the builder domain. Used by the
    /// onboarding flow where the chosen model is not yet in `config.yml`.
    builder_llm_override: Option<LlmClient>,
    /// Onboarding-only LLM metadata persisted into run metadata so a cold
    /// resume (server restart while suspended mid-onboarding) can rebuild
    /// the same client. None for non-onboarding runs.
    builder_llm_metadata: Option<BuilderLlmMetadata>,
    /// Reference cards to pre-populate in the builder solver's cached
    /// system prefix.  Set per onboarding phase by the HTTP route;
    /// empty for interactive builder runs (which rely on the
    /// `lookup_reference` tool).
    builder_knowledge_cards: Vec<KnowledgeCard>,
    /// Skip the Interpreting LLM call after Solving completes.  Set
    /// for onboarding runs whose UI collapses the trace.
    builder_skip_interpreting: bool,
    /// Restrict the builder solver's tool list to the named tools.
    /// Set per onboarding phase to drop irrelevant tools (dbt, etc.).
    builder_tool_allowlist: Option<Vec<String>>,
    /// Analytics SQL-generation mode. When `true`, the analytics FSM
    /// terminates after producing SQL (skip executing + interpreting
    /// for pre-validated paths; LIMIT-0 smoke check + terminate for
    /// LLM-generated SQL). Used by the automation `type: agent` step
    /// when `AgentTaskConfig.output.mode == Sql`.
    analytics_sql_mode: bool,
    /// Workspace owning the run; stamped onto `agentic_runs.workspace_id`
    /// at insert. HTTP handlers read this from the `/{workspace_id}/...`
    /// path; CLI / eval default to the nil UUID (= `LOCAL_WORKSPACE_ID`,
    /// the single-workspace local serve mode's implicit id).
    workspace_id: Uuid,
}

enum Domain {
    Analytics { agent_id: String },
    Builder { model: Option<String> },
}

/// Onboarding-flow LLM metadata. Persisted into the run's `metadata` JSON
/// at create time so a cold resume can reconstruct the same `LlmClient`
/// — crucial because mid-onboarding the chosen model isn't yet in
/// `config.yml`, so the normal `resolve_model` lookup would fail and the
/// resumed run would call the LLM with an empty API key.
#[derive(Debug, Clone)]
pub struct BuilderLlmMetadata {
    pub vendor: String,
    pub model_ref: String,
    pub key_var: String,
}

#[derive(Debug)]
pub enum PipelineError {
    Config(String),
    Build(String),
    Db(sea_orm::DbErr),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Build(msg) => write!(f, "build error: {msg}"),
            Self::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl From<sea_orm::DbErr> for PipelineError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Db(e)
    }
}

impl std::error::Error for PipelineError {}

/// Error from [`PipelineBuilder::start`], carrying the persisted `run_id`
/// when one was created before the failure.
///
/// On a fast failure (e.g. a broken semantics file) the run row is still
/// inserted and marked `failed`, so it appears in the thread's run history.
/// Callers (the HTTP layer) need that id to de-duplicate the live failed
/// state against the persisted run — without it the frontend renders the
/// user message and error twice.
#[derive(Debug)]
pub struct PipelineStartError {
    /// `Some` when a run row was inserted (and marked failed) before the
    /// error; `None` when the failure happened before any row existed
    /// (e.g. config not found) or for delegation children.
    pub run_id: Option<String>,
    pub source: PipelineError,
}

impl std::fmt::Display for PipelineStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegate to the source so existing `format!("{e}")` callers are
        // unaffected by the wrapper.
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for PipelineStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<PipelineError> for PipelineStartError {
    fn from(source: PipelineError) -> Self {
        Self {
            run_id: None,
            source,
        }
    }
}

/// Transition a run row to `failed` on a start/resume error, recording the
/// cause in `error_message`. Best-effort: a DB failure here (including the
/// no-row case where config loading failed before the run was inserted)
/// must not mask the original pipeline error, so we only log it.
async fn mark_run_failed_best_effort(db: &DatabaseConnection, run_id: &str, err: &PipelineError) {
    let msg = err.to_string();
    if let Err(e) = agentic_runtime::crud::update_run_failed(db, run_id, &msg).await {
        tracing::debug!(
            run_id,
            error = %e,
            original = %msg,
            "could not mark run failed after pipeline error (run row may not exist yet)"
        );
    }
}

impl PipelineBuilder {
    pub fn new(platform: Arc<dyn PlatformContext>) -> Self {
        Self {
            platform,
            builder_bridges: None,
            domain: None,
            question: String::new(),
            thread_id: None,
            thinking_mode: ThinkingMode::Auto,
            schema_cache: None,
            builder_test_runner: None,
            builder_app_runner: None,
            existing_run_id: None,
            human_input: None,
            builder_llm_override: None,
            builder_llm_metadata: None,
            builder_knowledge_cards: Vec::new(),
            builder_skip_interpreting: false,
            builder_tool_allowlist: None,
            analytics_sql_mode: false,
            workspace_id: Uuid::nil(),
        }
    }

    /// Set the workspace that owns the run. HTTP handlers pass the
    /// `/{workspace_id}/...` path segment; CLI / eval leave at the
    /// default nil UUID (= local single-workspace mode).
    pub fn workspace_id(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = workspace_id;
        self
    }

    /// Supply the four builder-domain port impls. Required before starting
    /// the builder pipeline; ignored for analytics runs.
    pub fn with_builder_bridges(mut self, bridges: BuilderBridges) -> Self {
        self.builder_bridges = Some(bridges);
        self
    }

    /// Override the human input provider for the builder domain.
    pub fn human_input(mut self, provider: agentic_core::human_input::HumanInputHandle) -> Self {
        self.human_input = Some(provider);
        self
    }

    /// Override the LLM client used for the builder domain.
    ///
    /// Used by the onboarding flow where the chosen model isn't yet present in
    /// `config.yml`, so the normal `resolve_model` → `build_llm_client` path
    /// can't find it. The caller is responsible for constructing a client with
    /// the correct vendor/provider and API key.
    pub fn with_builder_llm_client(mut self, client: LlmClient) -> Self {
        self.builder_llm_override = Some(client);
        self
    }

    /// Persist onboarding-only LLM metadata (vendor / model_ref / key_var)
    /// into the run's `metadata` JSON so a cold resume can reconstruct an
    /// identical `LlmClient` via [`with_builder_llm_client`]. Only meaningful
    /// for the onboarding flow — non-onboarding builder runs leave this
    /// unset and rely on `resolve_model` against the project's `config.yml`.
    pub fn with_builder_llm_metadata(mut self, meta: BuilderLlmMetadata) -> Self {
        self.builder_llm_metadata = Some(meta);
        self
    }

    /// Pre-populate the builder solver's cached system prefix with
    /// the given reference cards.  Called by the HTTP route when the
    /// request carries an `OnboardingContext`; cards come from
    /// `OnboardingContext::knowledge_cards()`.
    pub fn knowledge_cards(mut self, cards: Vec<KnowledgeCard>) -> Self {
        self.builder_knowledge_cards = cards;
        self
    }

    /// Skip the builder's Interpreting LLM call.  Set for onboarding
    /// runs (the UI collapses the per-phase trace and shows CTAs in
    /// place of the synthesized summary).
    pub fn skip_interpreting(mut self, skip: bool) -> Self {
        self.builder_skip_interpreting = skip;
        self
    }

    /// Restrict the builder's tool list to the named tools.  Called
    /// by the HTTP route when the request carries an
    /// `OnboardingContext`; the allowlist comes from
    /// `OnboardingContext::tool_allowlist()`.
    pub fn tool_allowlist(mut self, names: Vec<String>) -> Self {
        self.builder_tool_allowlist = Some(names);
        self
    }

    /// Run an analytics agent in SQL-generation mode. The analytics
    /// FSM terminates after SQL is produced — pre-validated paths
    /// (semantic model, verified `.sql`, vendor engine) skip
    /// executing entirely; LLM-generated SQL runs a `LIMIT 0` smoke
    /// check before terminating. Used by the automation `type: agent`
    /// step when `AgentTaskConfig.output.mode == Sql`. No-op for the
    /// builder agent.
    pub fn analytics_sql_mode(mut self) -> Self {
        self.analytics_sql_mode = true;
        self
    }

    /// Use an existing run_id instead of generating a new one.
    ///
    /// When set, `start()` skips the DB `insert_run` call — the caller
    /// (typically the coordinator) is responsible for having already
    /// created the run row.  Used for delegation children.
    pub fn existing_run(mut self, run_id: String) -> Self {
        self.existing_run_id = Some(run_id);
        self
    }

    /// Configure for the analytics domain.
    pub fn analytics(mut self, agent_id: &str) -> Self {
        self.domain = Some(Domain::Analytics {
            agent_id: agent_id.to_string(),
        });
        self
    }

    /// Configure for the builder domain.
    pub fn builder(mut self, model: Option<String>) -> Self {
        self.domain = Some(Domain::Builder { model });
        self
    }

    /// Set the user's question.
    pub fn question(mut self, q: &str) -> Self {
        self.question = q.to_string();
        self
    }

    /// Link to a conversation thread.
    pub fn thread(mut self, id: Uuid) -> Self {
        self.thread_id = Some(id);
        self
    }

    pub fn thinking_mode(mut self, mode: ThinkingMode) -> Self {
        self.thinking_mode = mode;
        self
    }

    /// Set schema cache (shared across requests in HTTP mode).
    pub fn schema_cache(mut self, cache: Arc<Mutex<HashMap<String, SchemaCatalog>>>) -> Self {
        self.schema_cache = Some(cache);
        self
    }

    pub fn test_runner(mut self, runner: Arc<dyn BuilderTestRunner>) -> Self {
        self.builder_test_runner = Some(runner);
        self
    }

    /// Set builder app runner — backs the `run_app` tool.
    pub fn app_runner(mut self, runner: Arc<dyn BuilderAppRunner>) -> Self {
        self.builder_app_runner = Some(runner);
        self
    }

    /// Build and start the pipeline.
    ///
    /// Inserts the run record in the database, starts the domain pipeline,
    /// and returns a [`StartedPipeline`] with an erased handle.
    pub async fn start(
        mut self,
        db: &DatabaseConnection,
    ) -> Result<StartedPipeline, PipelineStartError> {
        let domain = self.domain.take().ok_or_else(|| {
            PipelineError::Config("domain not set (call .analytics() or .builder())".into())
        })?;

        // When an existing_run_id is provided (delegation child), use it and
        // skip the DB insert — the coordinator already created the run row.
        let (run_id, skip_db_insert) = match self.existing_run_id.take() {
            Some(id) => (id, true),
            None => (Uuid::new_v4().to_string(), false),
        };
        let ctx_root = self.platform.context_root().await;
        // `ctx_root` owns the materialised tempdir (stateless fleet) and must
        // outlive the resolve_context + semantic-catalog load below — keep it
        // bound in this scope.
        let base_dir = ctx_root.path().to_path_buf();

        let result = match domain {
            Domain::Analytics { agent_id } => {
                self.start_analytics(db, &run_id, &agent_id, &base_dir, skip_db_insert)
                    .await
            }
            Domain::Builder { model } => {
                self.start_builder(db, &run_id, model, &base_dir, skip_db_insert)
                    .await
            }
        };

        // Single chokepoint: any failure after the run row was inserted must
        // transition it to a terminal (`failed`) state. The run row is the
        // source of truth the SSE handler uses to close hung streams, so a
        // build failure that never reaches the orchestrator (e.g. a broken
        // semantics file) still produces a terminal event for the client.
        // Skipped for delegation children — the parent coordinator owns that
        // run row and will finalize it.
        if let Err(e) = &result
            && !skip_db_insert
        {
            mark_run_failed_best_effort(db, &run_id, e).await;
        }
        result.map_err(|source| PipelineStartError {
            // Surface the run_id only when a row was actually inserted, so
            // the caller can reconcile it with the persisted run history.
            run_id: (!skip_db_insert).then(|| run_id.clone()),
            source,
        })
    }

    /// Resume a suspended run with the user's answer.
    ///
    /// Rebuilds the solver + orchestrator from config, then calls
    /// `orchestrator.resume(resume_data, answer)` instead of `run(intent)`.
    /// Does NOT insert a new run record — the existing run is reused.
    ///
    /// `source_type` must be `"analytics"` or `"builder"`.
    pub async fn resume(
        self,
        db: &DatabaseConnection,
        run_id: &str,
        source_type: &str,
        agent_id: &str,
        model: Option<String>,
        resume_data: agentic_core::human_input::SuspendedRunData,
        answer: String,
    ) -> Result<StartedPipeline, PipelineError> {
        let ctx_root = self.platform.context_root().await;
        // `ctx_root` owns the materialised tempdir (stateless fleet) and must
        // outlive the resolve_context + semantic-catalog load below — keep it
        // bound in this scope.
        let base_dir = ctx_root.path().to_path_buf();

        agentic_runtime::crud::update_run_running(db, run_id).await?;

        let result = match source_type {
            "analytics" => {
                self.resume_analytics(db, run_id, agent_id, &base_dir, resume_data, answer)
                    .await
            }
            "builder" => {
                self.resume_builder(db, run_id, model, &base_dir, resume_data, answer)
                    .await
            }
            _ => Err(PipelineError::Config(format!(
                "cold resume not supported for source_type: {source_type}"
            ))),
        };

        // Single chokepoint (mirrors `start`): the run row was just moved
        // back to `running`, so any resume failure must move it to `failed`
        // — otherwise the open SSE stream on the suspended run waits forever.
        if let Err(e) = &result {
            mark_run_failed_best_effort(db, run_id, e).await;
        }
        result
    }

    async fn start_analytics(
        self,
        db: &DatabaseConnection,
        run_id: &str,
        agent_id: &str,
        base_dir: &std::path::Path,
        skip_db_insert: bool,
    ) -> Result<StartedPipeline, PipelineError> {
        // `resolve_agent_yaml` returns the compiled agent definition when the
        // compile boundary is enabled; `None` falls through to the FS read below.
        let config = match self.platform.resolve_agent_yaml(agent_id).await {
            Some(yaml) => {
                AgentConfig::from_yaml(&yaml).map_err(|e| PipelineError::Config(format!("{e}")))?
            }
            None => {
                let config_path = base_dir.join(agent_id);
                let config_path = if config_path.exists() {
                    config_path
                } else {
                    let with_ext = base_dir.join(format!("{}.agentic.yml", agent_id));
                    if with_ext.exists() {
                        with_ext
                    } else {
                        config_path // will produce a clear "not found" error
                    }
                };
                AgentConfig::from_file(&config_path)
                    .map_err(|e| PipelineError::Config(format!("{e}")))?
            }
        };

        // Insert run + extension (skipped for delegation children — the
        // coordinator already created the run via insert_run_with_parent).
        let source_type = "analytics";
        if !skip_db_insert {
            let metadata = serde_json::json!({
                "agent_id": agent_id,
                "thinking_mode": self.thinking_mode.to_db(),
            });
            agentic_runtime::crud::insert_run(
                db,
                run_id,
                &self.question,
                self.thread_id,
                source_type,
                Some(metadata),
                self.workspace_id,
            )
            .await?;
            agentic_analytics::insert_run_meta(db, run_id, agent_id, self.thinking_mode.to_db())
                .await?;
        }

        // Resolve project model + connectors via the platform port.
        let project_model = self
            .platform
            .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
            .await;

        // Resolve databases + connectors.
        //
        // Effective set = `agent.databases` ∪ databases referenced from the
        // agent's `context:` glob. No fallback to "everything in
        // config.yml" — an agent only gets connector access to what it
        // explicitly declares. The previous auto-fill (f7c474080) was
        // unwound because a misparsed agent (e.g. classic `.agent.yml`
        // shape silently dropped to defaults) would otherwise inherit
        // the whole workspace's database surface and start chasing SQL
        // it has no business running.
        let mut effective_databases: Vec<String> = config.databases.clone();
        if let Ok(resolved) = config.resolve_context(base_dir) {
            for db_name in resolved.referenced_databases {
                if !effective_databases.contains(&db_name) {
                    effective_databases.push(db_name);
                }
            }
        }
        let resolved = platform::resolve_connectors(&effective_databases, &*self.platform).await;
        let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
        connectors.extend(resolved.pre_built);

        // Automation runner.
        //
        // Only injected when the agent's `context:` glob actually
        // resolves to automation files. Without an explicit declaration
        // we don't hand the FSM a SubrunRunner at all — an empty list
        // would cause `OxyAutomationRunner::search()` to fall back to
        // `workspace.list_automation_files()` (every automation in the
        // project), and a misparsed/empty agent would then chain
        // `search_automations` → arbitrary automation → another agent
        // step → another empty agent → recursion. The previous
        // behavior was "always inject and let the runner fall back"
        // — unwound here so an agent only sees automations it asked for.
        let subrun_runner: Option<Arc<dyn agentic_core::subrun::SubrunRunner>> = config
            .resolve_context(base_dir)
            .ok()
            .map(|ctx| ctx.automation_files)
            .filter(|files| !files.is_empty())
            .map(|files| {
                let workspace: Arc<dyn agentic_automation::WorkspaceContext> =
                    self.platform.clone();
                let runner = agentic_automation::OxyAutomationRunner::new(workspace)
                    .with_automation_files(files);
                Arc::new(runner) as Arc<dyn agentic_core::subrun::SubrunRunner>
            });

        let (history, prior_spec_hint) = if let Some(tid) = self.thread_id {
            let turns = agentic_runtime::crud::get_thread_history(db, tid, 10)
                .await
                .unwrap_or_default();
            let history: Vec<agentic_analytics::ConversationTurn> = turns
                .into_iter()
                .map(|t| agentic_analytics::ConversationTurn {
                    question: t.question,
                    answer: t.answer,
                })
                .collect();
            (history, None)
        } else {
            (vec![], None)
        };

        // Build params.
        let params = agentic_analytics::PipelineParams {
            config,
            base_dir: base_dir.to_path_buf(),
            agent_id: agent_id.to_string(),
            connectors,
            default_connector: effective_databases.first().cloned(),
            question: self.question,
            history,
            prior_spec_hint,
            schema_cache: self.schema_cache,
            project_model,
            timezone: self.platform.timezone(),
            use_extended_thinking: self.thinking_mode.is_extended(),
            subrun_runner,
            metric_tree_runner: self.platform.metric_tree_runner(),
            anomaly_store: self.platform.anomaly_store(),
            workspace_id: self.workspace_id,
            metric_sink: self.platform.metric_sink(),
            human_input: self.human_input.clone(),
            sql_generation_mode: self.analytics_sql_mode,
            // Forward the platform's preagg wiring so the analytics
            // Specifying stage can short-circuit to local Parquet.
            preagg: self.platform.preagg_context(),
        };

        // Start pipeline. Connector-less narrative-wrapper agents
        // (`databases: []`, `context: []`, no `states:` overrides,
        // just `instructions:`) take the brief one-shot LLM path
        // instead of the full Clarifying → … → Interpreting FSM —
        // the FSM is overhead for "format these numbers" prompts and
        // can spawn an unbounded subrun chain when `instructions:` is
        // ever empty. Detection is on the parsed config, no flag in
        // the automation YAML required.
        let handle = if agentic_analytics::is_brief_agent(&params.config) {
            agentic_analytics::start_brief_pipeline(params)
                .await
                .map_err(|e| PipelineError::Build(format!("{e}")))?
        } else {
            agentic_analytics::start_pipeline(params)
                .await
                .map_err(|e| PipelineError::Build(format!("{e}")))?
        };

        Ok(StartedPipeline {
            run_id: run_id.to_string(),
            source_type: source_type.to_string(),
            inner: ErasedHandle::Analytics(handle),
        })
    }

    async fn resume_analytics(
        self,
        db: &DatabaseConnection,
        run_id: &str,
        agent_id: &str,
        base_dir: &std::path::Path,
        resume_data: agentic_core::human_input::SuspendedRunData,
        answer: String,
    ) -> Result<StartedPipeline, PipelineError> {
        // Defense: an empty `agent_id` would resolve `base_dir.join("")`
        // to the workspace root (a directory), and `from_file` would
        // then fail with the cryptic `IO error: Is a directory`.
        // The executor already falls back to `task_metadata.original_spec`
        // to recover the id; if it still can't find it, surface a
        // clear error instead of letting it propagate as a path error.
        if agent_id.is_empty() {
            return Err(PipelineError::Config(
                "resume: agent_id is empty (no metadata.agent_id and no \
                 task_metadata.original_spec.agent_id on the run row)"
                    .to_string(),
            ));
        }
        // Same compile-boundary read path as `start_analytics`. The
        // resume path needs to load the same agent definition the
        // original run did, so it routes through the same hook.
        let config = match self.platform.resolve_agent_yaml(agent_id).await {
            Some(yaml) => {
                AgentConfig::from_yaml(&yaml).map_err(|e| PipelineError::Config(format!("{e}")))?
            }
            None => {
                let config_path = base_dir.join(agent_id);
                let config_path = if config_path.exists() {
                    config_path
                } else {
                    let with_ext = base_dir.join(format!("{}.agentic.yml", agent_id));
                    if with_ext.exists() {
                        with_ext
                    } else {
                        config_path
                    }
                };
                AgentConfig::from_file(&config_path)
                    .map_err(|e| PipelineError::Config(format!("{e}")))?
            }
        };

        // Resolve project model + connectors via the platform port.
        let project_model = self
            .platform
            .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
            .await;

        // Resolve databases + connectors.
        //
        // Effective set = `agent.databases` ∪ databases referenced from the
        // agent's `context:` glob. No fallback to "everything in
        // config.yml" — an agent only gets connector access to what it
        // explicitly declares. The previous auto-fill (f7c474080) was
        // unwound because a misparsed agent (e.g. classic `.agent.yml`
        // shape silently dropped to defaults) would otherwise inherit
        // the whole workspace's database surface and start chasing SQL
        // it has no business running.
        let mut effective_databases: Vec<String> = config.databases.clone();
        if let Ok(resolved) = config.resolve_context(base_dir) {
            for db_name in resolved.referenced_databases {
                if !effective_databases.contains(&db_name) {
                    effective_databases.push(db_name);
                }
            }
        }
        let resolved = platform::resolve_connectors(&effective_databases, &*self.platform).await;
        let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
        connectors.extend(resolved.pre_built);

        // Automation runner.
        //
        // Only injected when the agent's `context:` glob actually
        // resolves to automation files. Without an explicit declaration
        // we don't hand the FSM a SubrunRunner at all — an empty list
        // would cause `OxyAutomationRunner::search()` to fall back to
        // `workspace.list_automation_files()` (every automation in the
        // project), and a misparsed/empty agent would then chain
        // `search_automations` → arbitrary automation → another agent
        // step → another empty agent → recursion. The previous
        // behavior was "always inject and let the runner fall back"
        // — unwound here so an agent only sees automations it asked for.
        let subrun_runner: Option<Arc<dyn agentic_core::subrun::SubrunRunner>> = config
            .resolve_context(base_dir)
            .ok()
            .map(|ctx| ctx.automation_files)
            .filter(|files| !files.is_empty())
            .map(|files| {
                let workspace: Arc<dyn agentic_automation::WorkspaceContext> =
                    self.platform.clone();
                let runner = agentic_automation::OxyAutomationRunner::new(workspace)
                    .with_automation_files(files);
                Arc::new(runner) as Arc<dyn agentic_core::subrun::SubrunRunner>
            });

        let (history, prior_spec_hint) = if let Some(tid) = self.thread_id {
            let turns = agentic_runtime::crud::get_thread_history(db, tid, 10)
                .await
                .unwrap_or_default();
            let history: Vec<agentic_analytics::ConversationTurn> = turns
                .into_iter()
                .map(|t| agentic_analytics::ConversationTurn {
                    question: t.question,
                    answer: t.answer,
                })
                .collect();
            (history, None)
        } else {
            (vec![], None)
        };

        let params = agentic_analytics::PipelineParams {
            config,
            base_dir: base_dir.to_path_buf(),
            agent_id: agent_id.to_string(),
            connectors,
            default_connector: effective_databases.first().cloned(),
            question: self.question,
            history,
            prior_spec_hint,
            schema_cache: self.schema_cache,
            project_model,
            timezone: self.platform.timezone(),
            use_extended_thinking: self.thinking_mode.is_extended(),
            subrun_runner,
            metric_tree_runner: self.platform.metric_tree_runner(),
            anomaly_store: self.platform.anomaly_store(),
            workspace_id: self.workspace_id,
            metric_sink: self.platform.metric_sink(),
            human_input: self.human_input.clone(),
            sql_generation_mode: self.analytics_sql_mode,
            // Forward the platform's preagg wiring so the analytics
            // Specifying stage can short-circuit to local Parquet.
            preagg: self.platform.preagg_context(),
        };

        let handle = agentic_analytics::resume_pipeline(params, resume_data, answer)
            .await
            .map_err(|e| PipelineError::Build(format!("{e}")))?;

        Ok(StartedPipeline {
            run_id: run_id.to_string(),
            source_type: "analytics".to_string(),
            inner: ErasedHandle::Analytics(handle),
        })
    }

    async fn resume_builder(
        mut self,
        db: &DatabaseConnection,
        run_id: &str,
        model: Option<String>,
        base_dir: &std::path::Path,
        resume_data: agentic_core::human_input::SuspendedRunData,
        answer: String,
    ) -> Result<StartedPipeline, PipelineError> {
        let bridges = self.builder_bridges.clone().ok_or_else(|| {
            PipelineError::Config(
                "builder bridges not provided — call .with_builder_bridges() first".into(),
            )
        })?;

        // Resolve model + API key, honouring an explicit override (onboarding
        // cold-resume rebuilds the override from persisted metadata before
        // calling .resume(), and we MUST use that here; falling through to
        // `build_builder_llm_client` would call the LLM with an empty key
        // because the onboarding model isn't yet in config.yml).
        let client = match self.builder_llm_override.take() {
            Some(c) => c,
            None => build_builder_llm_client(&*self.platform, model).await,
        };

        let history: Vec<agentic_builder::ConversationTurn> = if let Some(tid) = self.thread_id {
            agentic_runtime::crud::get_thread_history_with_events(db, tid, 10)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(q, a, exchanges)| agentic_builder::ConversationTurn {
                    question: q,
                    answer: a,
                    tool_exchanges: exchanges
                        .into_iter()
                        .map(|e| agentic_builder::ToolExchange {
                            name: e.name,
                            input: e.input,
                            output: e.output,
                        })
                        .collect(),
                })
                .collect()
        } else {
            vec![]
        };

        let handle = agentic_builder::resume_pipeline(
            agentic_builder::BuilderPipelineParams {
                client,
                project_root: base_dir.to_path_buf(),
                timezone: self.platform.timezone(),
                question: self.question,
                history,
                db_provider: Some(bridges.db_provider),
                project_validator: Some(bridges.project_validator),
                schema_provider: Some(bridges.schema_provider),
                semantic_compiler: Some(bridges.semantic_compiler),
                test_runner: self.builder_test_runner,
                app_runner: self.builder_app_runner,
                human_input: None,
                secrets_provider: bridges.secrets_provider,
                knowledge_cards: self.builder_knowledge_cards,
                skip_interpreting: self.builder_skip_interpreting,
                tool_allowlist: self.builder_tool_allowlist,
            },
            resume_data,
            answer,
        );

        Ok(StartedPipeline {
            run_id: run_id.to_string(),
            source_type: "builder".to_string(),
            inner: ErasedHandle::Builder(handle),
        })
    }

    async fn start_builder(
        mut self,
        db: &DatabaseConnection,
        run_id: &str,
        model: Option<String>,
        base_dir: &std::path::Path,
        skip_db_insert: bool,
    ) -> Result<StartedPipeline, PipelineError> {
        // Skip DB insert for delegation children — the coordinator already
        // created the run via insert_run_with_parent.
        let source_type = "builder";
        if !skip_db_insert {
            let knowledge_card_slugs: Vec<&str> = self
                .builder_knowledge_cards
                .iter()
                .map(|c| c.slug())
                .collect();
            let mut metadata = serde_json::json!({
                "agent_id": "__builder__",
                "model": model,
                "knowledge_cards": knowledge_card_slugs,
                "skip_interpreting": self.builder_skip_interpreting,
                "tool_allowlist": self.builder_tool_allowlist,
            });
            // Persist onboarding LLM metadata so cold-resume (server restart
            // mid-onboarding, before `config.yml` is written) can rebuild the
            // same LlmClient. Without these, a resumed run falls back to
            // `resolve_model` against a config.yml that doesn't yet contain
            // the onboarding-chosen model and ends up calling the LLM with
            // an empty API key.
            if let Some(meta) = &self.builder_llm_metadata {
                let m = metadata.as_object_mut().expect("json! produces an object");
                m.insert(
                    "onboarding_vendor".into(),
                    serde_json::Value::String(meta.vendor.clone()),
                );
                m.insert(
                    "onboarding_model_ref".into(),
                    serde_json::Value::String(meta.model_ref.clone()),
                );
                m.insert(
                    "onboarding_key_var".into(),
                    serde_json::Value::String(meta.key_var.clone()),
                );
            }
            agentic_runtime::crud::insert_run(
                db,
                run_id,
                &self.question,
                self.thread_id,
                source_type,
                Some(metadata),
                self.workspace_id,
            )
            .await?;
        }

        let bridges = self.builder_bridges.clone().ok_or_else(|| {
            PipelineError::Config(
                "builder bridges not provided — call .with_builder_bridges() first".into(),
            )
        })?;

        // Resolve model + API key, honouring an explicit override (onboarding flow).
        let client = match self.builder_llm_override.take() {
            Some(c) => c,
            None => build_builder_llm_client(&*self.platform, model).await,
        };

        let history: Vec<agentic_builder::ConversationTurn> = if let Some(tid) = self.thread_id {
            agentic_runtime::crud::get_thread_history_with_events(db, tid, 10)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(q, a, exchanges)| agentic_builder::ConversationTurn {
                    question: q,
                    answer: a,
                    tool_exchanges: exchanges
                        .into_iter()
                        .map(|e| agentic_builder::ToolExchange {
                            name: e.name,
                            input: e.input,
                            output: e.output,
                        })
                        .collect(),
                })
                .collect()
        } else {
            vec![]
        };

        let handle = agentic_builder::start_pipeline(agentic_builder::BuilderPipelineParams {
            client,
            project_root: base_dir.to_path_buf(),
            timezone: self.platform.timezone(),
            question: self.question,
            history,
            db_provider: Some(bridges.db_provider),
            project_validator: Some(bridges.project_validator),
            schema_provider: Some(bridges.schema_provider),
            semantic_compiler: Some(bridges.semantic_compiler),
            test_runner: self.builder_test_runner,
            app_runner: self.builder_app_runner,
            human_input: self.human_input,
            secrets_provider: bridges.secrets_provider,
            knowledge_cards: self.builder_knowledge_cards,
            skip_interpreting: self.builder_skip_interpreting,
            tool_allowlist: self.builder_tool_allowlist,
        });

        Ok(StartedPipeline {
            run_id: run_id.to_string(),
            source_type: source_type.to_string(),
            inner: ErasedHandle::Builder(handle),
        })
    }
}

/// Resolve the builder domain's LLM client via the platform port.
///
/// Tries the explicit model ref first, then the project's configured default.
/// Never falls back to a hardcoded provider.
async fn build_builder_llm_client(ctx: &dyn ProjectContext, model: Option<String>) -> LlmClient {
    // Try explicit model ref, then project default.
    let info = if let Some(ref name) = model {
        match ctx.resolve_model(Some(name), false).await {
            Some(info) => Some(info),
            None => ctx.resolve_model(None, false).await,
        }
    } else {
        ctx.resolve_model(None, false).await
    };
    let genai = agentic_llm::GenAiContext {
        agent_name: Some("builder".to_string()),
        workspace_id: Some(ctx.workspace_id())
            .filter(|id| !id.is_nil())
            .map(|id| id.to_string()),
        ..Default::default()
    };
    if let Some(info) = info {
        return platform::build_llm_client(&info).with_genai_context(genai);
    }
    tracing::warn!(
        model = ?model,
        "builder: no LLM model resolved from project config; LLM calls will fail"
    );
    // Return a placeholder — the LLM call will fail with a clear error.
    LlmClient::with_model("", model.unwrap_or_default())
}

/// A prepared one-shot completer: a resolved workspace model with its client
/// already built, reusable across many `system` + `user` completions (no tools,
/// no streaming). Resolve once via [`prepare_one_shot`], then `complete` per
/// call — cloning is cheap (the inner [`LlmClient`] is `Arc`-backed).
#[derive(Clone)]
pub struct OneShotCompleter {
    client: LlmClient,
}

impl OneShotCompleter {
    /// Run a single completion. Returns the model's text, or a message
    /// describing why the call failed.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        self.client
            .complete(system, user)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Resolve a workspace model (explicit `model_ref`, else the project default)
/// and build a reusable [`OneShotCompleter`]. This is how hosts (e.g. `oxy-app`'s
/// eval judge) reach the agentic LLM stack for a non-streaming completion instead
/// of building a client themselves; `agent_name` labels the genai/tracing spans.
///
/// An explicit-but-unknown model is an error, not a silent fallback to the
/// default — matching the pre-migration judge, so a misconfigured `judge_model`
/// (a typo, or an as-yet-unsupported vendor) fails loud rather than scoring on a
/// different model.
pub async fn prepare_one_shot(
    ctx: &dyn ProjectContext,
    model_ref: Option<&str>,
    agent_name: &str,
) -> Result<OneShotCompleter, String> {
    let info = match model_ref {
        Some(name) => ctx.resolve_model(Some(name), false).await.ok_or_else(|| {
            format!("LLM model '{name}' could not be resolved from project config")
        })?,
        None => ctx
            .resolve_model(None, false)
            .await
            .ok_or_else(|| "no default LLM model configured".to_string())?,
    };
    let genai = agentic_llm::GenAiContext {
        agent_name: Some(agent_name.to_string()),
        workspace_id: Some(ctx.workspace_id())
            .filter(|id| !id.is_nil())
            .map(|id| id.to_string()),
        ..Default::default()
    };
    Ok(OneShotCompleter {
        client: platform::build_llm_client(&info).with_genai_context(genai),
    })
}

// ── StartedPipeline (type-erased) ───────────────────────────────────────────

/// A started pipeline with type-erased domain events.
///
/// Call [`drive()`](StartedPipeline::drive) to run the full lifecycle
/// (bridge task + outcome loop + cleanup).
pub struct StartedPipeline {
    pub run_id: String,
    pub source_type: String,
    inner: ErasedHandle,
}

enum ErasedHandle {
    Analytics(agentic_runtime::handle::PipelineHandle<agentic_analytics::AnalyticsEvent>),
    Builder(agentic_runtime::handle::PipelineHandle<agentic_builder::BuilderEvent>),
}

impl StartedPipeline {
    /// Drive the pipeline through its full lifecycle using the runtime.
    ///
    /// Spawns the bridge task, processes outcomes, handles suspension/resume,
    /// Convert into an [`ExecutingTask`] for use with the coordinator-worker
    /// architecture.
    ///
    /// Spawns background tasks that:
    /// - Drain domain-typed events, serialize them to `(String, Value)`, and
    ///   forward to the `ExecutingTask::events` channel.
    /// - Map [`PipelineOutcome`] to [`TaskOutcome`] and send on the outcome
    ///   channel.
    pub fn into_executing_task(
        self,
    ) -> (
        agentic_runtime::worker::ExecutingTask,
        tokio::task::JoinHandle<()>,
    ) {
        use agentic_core::delegation::TaskOutcome;

        tracing::debug!(target: "worker", run_id = %self.run_id, source_type = %self.source_type, "converting StartedPipeline into ExecutingTask");
        let (event_tx, event_rx) = mpsc::channel::<(String, serde_json::Value)>(256);
        let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(4);
        let cancel = tokio_util::sync::CancellationToken::new();

        // HITL answers are routed via `RuntimeState::answer_txs` by the
        // coordinator, not through `ExecutingTask::answers`, so we pass `None`.
        let bridge_handle = match self.inner {
            ErasedHandle::Analytics(handle) => {
                spawn_bridge_tasks(handle, event_tx, outcome_tx, cancel.clone())
            }
            ErasedHandle::Builder(handle) => {
                spawn_bridge_tasks(handle, event_tx, outcome_tx, cancel.clone())
            }
        };

        (
            agentic_runtime::worker::ExecutingTask {
                events: event_rx,
                outcomes: outcome_rx,
                cancel,
                answers: None,
            },
            bridge_handle,
        )
    }
}

/// Spawn tasks that bridge a typed `PipelineHandle<Ev>` into the generic
/// `ExecutingTask` channels.
///
/// Returns a `JoinHandle` that completes once the event-draining and
/// outcome-forwarding bridge tasks have both finished. Callers that need to
/// know when no more events or outcomes will be forwarded (e.g. before
/// notifying SSE subscribers) can await it — bounded with a timeout in case a
/// producer keeps a sender open past the terminal outcome.
fn spawn_bridge_tasks<Ev: agentic_core::DomainEvents + 'static>(
    handle: PipelineHandle<Ev>,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<agentic_core::delegation::TaskOutcome>,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    use agentic_core::delegation::TaskOutcome;

    let pipeline_cancel = handle.cancel.clone();
    let mut events = handle.events;
    let mut outcomes = handle.outcomes;

    // Forward cancellation.
    tokio::spawn({
        let cancel = cancel.clone();
        async move {
            cancel.cancelled().await;
            pipeline_cancel.cancel();
        }
    });

    // Drain events and serialize.
    let events_task = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let (event_type, payload) = event.serialize();
            if event_tx.send((event_type, payload)).await.is_err() {
                break;
            }
        }
    });

    // Map PipelineOutcome → TaskOutcome. Forward ALL outcomes (pipeline
    // may produce Suspended then Done after resume).
    let outcomes_task = tokio::spawn(async move {
        // Track whether the pipeline produced any resolving outcome (a terminal
        // Done/Failed/Cancelled, or a Suspended). A `Suspended` is a legitimate
        // non-hang stopping point after which the driver normally drops the
        // sender, so that close must NOT be treated as a failure. Only a channel
        // close with no outcome at all means the driver task died (panic or
        // early drop) before reporting anything.
        let mut saw_any_outcome = false;
        while let Some(outcome) = outcomes.recv().await {
            saw_any_outcome = true;
            let is_terminal = matches!(
                outcome,
                PipelineOutcome::Done { .. }
                    | PipelineOutcome::Failed(_)
                    | PipelineOutcome::Cancelled
            );
            let task_outcome = match outcome {
                PipelineOutcome::Done { answer, metadata } => {
                    TaskOutcome::Done { answer, metadata }
                }
                PipelineOutcome::Suspended {
                    reason,
                    resume_data,
                    trace_id,
                } => TaskOutcome::Suspended {
                    reason,
                    resume_data,
                    trace_id,
                },
                PipelineOutcome::Failed(msg) => TaskOutcome::Failed(msg),
                PipelineOutcome::Cancelled => TaskOutcome::Cancelled,
            };
            if outcome_tx.send(task_outcome).await.is_err() {
                break;
            }
            if is_terminal {
                break;
            }
        }

        // The outcome channel closed without the driver ever reporting an
        // outcome — it died (panic or early drop). Synthesize a Failed so the
        // run transitions to failed and the SSE emits a terminal event instead
        // of hanging forever. If the receiver is already gone this send is a
        // harmless no-op.
        if !saw_any_outcome {
            tracing::error!("pipeline driver terminated without an outcome; synthesizing Failed");
            let _ = outcome_tx
                .send(TaskOutcome::Failed(
                    "driver terminated without an outcome (panic or early drop)".to_string(),
                ))
                .await;
        }
    });

    tokio::spawn(async move {
        let _ = tokio::join!(events_task, outcomes_task);
    })
}

// ── Coordinator-based drive ─────────────────────────────────────────────────

/// Drive a pipeline using the coordinator-worker architecture.
///
/// This is the new path that supports agent delegation and automation execution
/// as child tasks. It creates a [`LocalTransport`], [`Worker`], and
/// [`Coordinator`], wires them together, and runs the pipeline to completion.
///
/// Drop-in replacement for [`StartedPipeline::drive`] when delegation support
/// is needed.
pub async fn drive_with_coordinator(
    started: StartedPipeline,
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    answer_rx: mpsc::Receiver<String>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    platform: Arc<dyn PlatformContext>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
) {
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
    use agentic_runtime::coordinator::Coordinator;
    use agentic_runtime::transport::DurableTransport;
    use agentic_runtime::worker::Worker;

    let run_id = started.run_id.clone();
    let _source_type = started.source_type.clone();

    // Convert the already-started pipeline into an ExecutingTask.
    let (executing_task, bridge_handle) = started.into_executing_task();

    // Create the durable transport backed by the task queue table,
    // scoped to this run's task tree. Without scoping, this worker
    // will happily claim a queued root task that belongs to a
    // sibling run (e.g., an automation just queued by
    // `start_automation_run`) — the LISTEN/NOTIFY matcher widened a
    // pre-existing race window from ~1s to <10ms, making the
    // poaching almost certain. When that happens, the poached
    // task's events flow to *this* coordinator which doesn't know
    // about it (`event for unknown task` warn), the Done outcome's
    // policy-driven AutomationDecision chain never fires, and the
    // sibling run sits with a seeded `agentic_workflow_state` row
    // but no in-flight queue task driving it.
    //
    // The `with_router` scope filter is `task_id = $root OR
    // task_id LIKE '$root.%'`, which still reaches every child /
    // grandchild this coordinator legitimately owns.
    let transport = DurableTransport::with_router(db.clone(), router, Some(run_id.clone()));

    // Create the task executor for child tasks (delegation).
    let executor = Arc::new(executor::PipelineTaskExecutor {
        platform,
        builder_bridges,
        schema_cache,
        builder_test_runner,
        builder_app_runner,
        db: db.clone(),
        state: Some(state.clone()),
        custom_executors: None,
    });

    // Worker: handles task execution (including the initial root task).
    let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

    // Query the max existing event seq so the coordinator starts after it.
    // This is critical for cold resume — avoids seq conflicts with prior events.
    let root_next_seq = agentic_runtime::crud::get_max_seq(&db, &run_id)
        .await
        .unwrap_or(-1)
        + 1;

    // Coordinator: manages the task tree. Wire in the
    // automation-aware completion policy + delegation resolver —
    // analytics / builder runs can delegate to an automation as a
    // child task, so every coordinator could see a
    // `workflow_continue` outcome from a chained `Automation` spec
    // *and* needs the resolver to route loop bodies / single
    // steps / on-disk automations to the right TaskSpec variant.
    let mut coordinator = Coordinator::new(
        db,
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    )
    .with_completion_policy(Arc::new(agentic_automation::AutomationCompletionPolicy))
    .with_delegation_resolver(Arc::new(agentic_automation::AutomationDelegationResolver));
    coordinator.register_answer_channel(run_id.clone(), answer_rx);

    // For the root task, we already have an ExecutingTask from the started
    // pipeline. We need to feed its events and outcome into the coordinator
    // via the transport, then run the coordinator loop.
    //
    // Strategy: spawn a "virtual worker" that forwards the existing
    // ExecutingTask to the coordinator, then start the real worker for any
    // child tasks that may be spawned.
    let root_task_id = run_id.clone();

    // Forward cancellation from RuntimeState's cancel_tx to the coordinator
    // transport so the coordinator sees the root task *and every descendant*
    // as cancelled. `cancel_subtree` is required — if we only cancel the root
    // while the pipeline is suspended on delegation, the builder/analytics
    // child keeps running, eventually completes, and triggers a spurious
    // parent resume.
    let cancel_forwarder = {
        let transport_cancel = transport.clone();
        let cancel_task_id = root_task_id.clone();
        tokio::spawn(async move {
            // Wait for the cancel signal.
            while cancel_rx.changed().await.is_ok() {
                if *cancel_rx.borrow() {
                    tracing::info!(
                        target: "coordinator",
                        task_id = %cancel_task_id,
                        "cancel signal received, cancelling task subtree"
                    );
                    let _ = transport_cancel.cancel_subtree(&cancel_task_id).await;
                    break;
                }
            }
        })
    };

    let virtual_worker = {
        let transport_clone = transport.clone();
        let task_id = root_task_id.clone();
        tokio::spawn(async move {
            use agentic_core::transport::WorkerTransport;
            tracing::debug!(target: "worker", task_id = %task_id, "virtual worker started");

            // Forward cancellation from transport to the executing task,
            // mirroring what Worker::handle_task does for child tasks.
            let cancel_token = transport_clone.cancellation_token(&task_id);
            let task_cancel = executing_task.cancel.clone();
            let _cancel_fwd = tokio::spawn({
                let task_id = task_id.clone();
                async move {
                    cancel_token.cancelled().await;
                    tracing::debug!(target: "worker", task_id = %task_id, "cancellation forwarded to root task");
                    task_cancel.cancel();
                }
            });

            // Spawn heartbeat loop for the root task.
            let heartbeat_cancel = WorkerTransport::spawn_heartbeat(
                transport_clone.as_ref(),
                &task_id,
                agentic_runtime::orchestrator::worker::HEARTBEAT_INTERVAL,
            );

            let mut events = executing_task.events;
            let mut outcomes = executing_task.outcomes;

            // Forward events and outcomes concurrently.
            // Events and outcomes arrive on separate channels; we must
            // process both without blocking one on the other. The pipeline
            // may emit a Suspended outcome while the events channel is
            // still open (pipeline task holds the sender).
            //
            // `parked_suspended` tracks whether the LAST outcome forwarded was
            // a suspension, i.e. whether this driver is stopping while still
            // holding the queue claim. Every exit from the loop below consults
            // it before cancelling the heartbeat — see `Worker::handle_task`.
            let mut parked_suspended = false;
            loop {
                tokio::select! {
                    event = events.recv() => {
                        match event {
                            Some((event_type, payload)) => {
                                let _ = transport_clone
                                    .send(agentic_core::transport::WorkerMessage::Event {
                                        task_id: task_id.clone(),
                                        event_type,
                                        payload,
                                    })
                                    .await;
                            }
                            None => {
                                // Events channel closed — drain remaining outcomes.
                                while let Some(outcome) = outcomes.recv().await {
                                    let is_terminal = matches!(
                                        outcome,
                                        agentic_core::delegation::TaskOutcome::Done { .. }
                                            | agentic_core::delegation::TaskOutcome::Failed(_)
                                            | agentic_core::delegation::TaskOutcome::Cancelled
                                    );
                                    parked_suspended = matches!(
                                        outcome,
                                        agentic_core::delegation::TaskOutcome::Suspended { .. }
                                    );
                                    let _ = transport_clone
                                        .send(agentic_core::transport::WorkerMessage::Outcome {
                                            task_id: task_id.clone(),
                                            outcome,
                                        })
                                        .await;
                                    if is_terminal {
                                        heartbeat_cancel.cancel();
                                        return;
                                    }
                                }
                                // A suspension still holds the claim — keep
                                // beating. See `Worker::handle_task`'s cleanup
                                // block for why, and for why this is not a leak.
                                if !parked_suspended {
                                    heartbeat_cancel.cancel();
                                }
                                return;
                            }
                        }
                    }
                    outcome = outcomes.recv() => {
                        match outcome {
                            Some(outcome) => {
                                // Same translation the pooled worker does: a
                                // deferral is not an outcome, so hand the task
                                // back to the queue instead of reporting a
                                // result for something that never ran. This
                                // in-process driver is a second worker
                                // implementation and has to honour it too.
                                if let agentic_core::delegation::TaskOutcome::Deferred {
                                    delay_secs,
                                    max_wait_secs,
                                    reason,
                                } = outcome
                                {
                                    tracing::info!(target: "worker", task_id = %task_id, delay_secs, %reason, "virtual worker deferring task");
                                    let _ = transport
                                        .send(agentic_core::transport::WorkerMessage::Defer {
                                            task_id: task_id.clone(),
                                            delay_secs,
                                            max_wait_secs,
                                            reason,
                                        })
                                        .await;
                                    // Cancel like every other exit from this
                                    // loop; without it the ticker keeps
                                    // heartbeating a task we just handed back.
                                    heartbeat_cancel.cancel();
                                    break;
                                }
                                let outcome_type = match &outcome {
                                    agentic_core::delegation::TaskOutcome::Done { .. } => "Done",
                                    agentic_core::delegation::TaskOutcome::Suspended { .. } => "Suspended",
                                    agentic_core::delegation::TaskOutcome::Failed(_) => "Failed",
                                    agentic_core::delegation::TaskOutcome::Cancelled => "Cancelled",
                                    agentic_core::delegation::TaskOutcome::Deferred { .. } => unreachable!("handled above"),
                                };
                                tracing::debug!(target: "worker", task_id = %task_id, outcome_type, "virtual worker forwarding outcome");
                                let is_terminal = matches!(
                                    outcome,
                                    agentic_core::delegation::TaskOutcome::Done { .. }
                                        | agentic_core::delegation::TaskOutcome::Failed(_)
                                        | agentic_core::delegation::TaskOutcome::Cancelled
                                );
                                parked_suspended = matches!(
                                    outcome,
                                    agentic_core::delegation::TaskOutcome::Suspended { .. }
                                );
                                // Same rule as `Worker::handle_task`: a dropped
                                // `Suspended` would keep the ticker alive for an
                                // outcome no coordinator received, leaving the row
                                // claimed with every backstop disarmed. Let the
                                // claim go stale instead.
                                if let Err(e) = transport_clone
                                    .send(agentic_core::transport::WorkerMessage::Outcome {
                                        task_id: task_id.clone(),
                                        outcome,
                                    })
                                    .await
                                {
                                    tracing::error!(
                                        target: "worker",
                                        rule = "dropped-outcome",
                                        task_id = %task_id,
                                        outcome_type,
                                        error = %e,
                                        "failed to deliver outcome to the coordinator"
                                    );
                                    parked_suspended = false;
                                }
                                if is_terminal {
                                    // Drain remaining events before exiting.
                                    while let Ok(ev) = events.try_recv() {
                                        let _ = transport_clone
                                            .send(agentic_core::transport::WorkerMessage::Event {
                                                task_id: task_id.clone(),
                                                event_type: ev.0,
                                                payload: ev.1,
                                            })
                                            .await;
                                    }
                                    heartbeat_cancel.cancel();
                                    return;
                                }
                            }
                            None => {
                                // Outcome channel closed — drain remaining events
                                // so late-arriving events (e.g. awaiting_input
                                // emitted just before the Suspended outcome) are
                                // not lost.
                                while let Some(ev) = events.recv().await {
                                    let _ = transport_clone
                                        .send(agentic_core::transport::WorkerMessage::Event {
                                            task_id: task_id.clone(),
                                            event_type: ev.0,
                                            payload: ev.1,
                                        })
                                        .await;
                                }
                                // This is the ordinary way a suspended pipeline
                                // ends: it emits `Suspended`, then drops the
                                // outcome sender. The claim is still ours, so
                                // the ticker has to outlive this driver.
                                if !parked_suspended {
                                    heartbeat_cancel.cancel();
                                }
                                return;
                            }
                        }
                    }
                }
            }
        })
    };

    // Register the root task in the coordinator (already running via virtual worker).
    coordinator.register_root(run_id.clone(), root_next_seq);

    let child_worker = tokio::spawn(async move {
        worker.run().await;
    });

    // Run the coordinator (blocks until all tasks complete).
    tracing::debug!(target: "coordinator", run_id = %run_id, "drive_with_coordinator: starting coordinator loop");
    coordinator.run().await;
    tracing::debug!(target: "coordinator", run_id = %run_id, "drive_with_coordinator: coordinator loop finished");

    // Wait for the bridge task to flush remaining events (including `done`)
    // to the DB before notifying subscribers and deregistering. Bounded by a
    // timeout in case a producer keeps a sender open past the terminal
    // outcome.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), bridge_handle).await;
    state.notify(&run_id);

    // Coordinator has exited and bridge has flushed: abort background tasks
    // that would otherwise linger (cancel forwarder still watching cancel_rx,
    // virtual worker still waiting on a closed channel, child worker still
    // polling the transport).
    cancel_forwarder.abort();
    virtual_worker.abort();
    child_worker.abort();

    // Clean up.
    state.deregister(&run_id);
}

// ── Event registry construction ─────────────────────────────────────────────

/// Build an [`EventRegistry`] with all known domain handlers pre-registered.
///
/// Call this once at startup — both HTTP server and CLI use the same registry.
pub fn build_event_registry() -> EventRegistry {
    let mut registry = EventRegistry::new();
    registry.register("analytics", agentic_analytics::event_handler());
    registry.register("builder", agentic_builder::event_handler());
    registry.register(
        agentic_automation::SOURCE_TYPE,
        agentic_automation::event_handler(),
    );
    registry.register(AIRWAY_SOURCE_TYPE, airway_event_handler());
    registry
}

// ── Domain-specific CRUD facades ────────────────────────────────────────────

/// Update a completed run's answer + analytics spec_hint extension.
///
/// Wraps `runtime::crud::update_run_done` + analytics extension update.
pub async fn update_run_done(
    db: &DatabaseConnection,
    run_id: &str,
    answer: &str,
    spec_hint: Option<serde_json::Value>,
) -> Result<(), sea_orm::DbErr> {
    agentic_runtime::crud::update_run_done(db, run_id, answer, None).await?;
    if let Some(hint) = spec_hint {
        agentic_analytics::update_run_spec_hint(db, run_id, hint).await?;
    }
    Ok(())
}

/// Update thinking_mode on the analytics extension table.
pub async fn update_run_thinking_mode(
    db: &DatabaseConnection,
    run_id: &str,
    thinking_mode: Option<String>,
) -> Result<(), sea_orm::DbErr> {
    agentic_analytics::update_run_thinking_mode(db, run_id, thinking_mode).await
}

/// Insert a run record + analytics extension (for non-builder runs).
///
/// Wraps `runtime::crud::insert_run` + analytics extension insert.
pub async fn insert_run(
    db: &DatabaseConnection,
    run_id: &str,
    agent_id: &str,
    question: &str,
    thread_id: Option<Uuid>,
    thinking_mode: Option<String>,
    workspace_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    let source_type = if agent_id == "__builder__" {
        "builder"
    } else {
        "analytics"
    };
    let metadata = serde_json::json!({
        "agent_id": agent_id,
        "thinking_mode": thinking_mode,
    });
    agentic_runtime::crud::insert_run(
        db,
        run_id,
        question,
        thread_id,
        source_type,
        Some(metadata),
        workspace_id,
    )
    .await?;

    if agent_id != "__builder__" {
        agentic_analytics::insert_run_meta(db, run_id, agent_id, thinking_mode).await?;
    }
    Ok(())
}

/// Get analytics extensions for a list of run IDs (bulk fetch).
pub async fn get_analytics_extensions(
    db: &DatabaseConnection,
    run_ids: &[String],
) -> Result<Vec<AnalyticsRunMeta>, sea_orm::DbErr> {
    agentic_analytics::get_run_metas(db, run_ids).await
}

/// Get a single analytics extension by run ID.
pub async fn get_analytics_extension(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<AnalyticsRunMeta>, sea_orm::DbErr> {
    agentic_analytics::get_run_meta(db, run_id).await
}

/// Thread history turn with analytics-specific `spec_hint`.
pub struct ThreadHistoryTurn {
    pub question: String,
    pub answer: String,
    pub spec_hint: Option<serde_json::Value>,
}

/// Return completed runs for a thread with spec_hint (analytics-specific).
pub async fn get_thread_history(
    db: &DatabaseConnection,
    thread_id: Uuid,
    limit: u64,
) -> Result<Vec<ThreadHistoryTurn>, sea_orm::DbErr> {
    // Use runtime CRUD instead of querying the entity directly.
    let runs = agentic_runtime::crud::get_runs_by_thread(db, thread_id).await?;
    let completed: Vec<_> = runs
        .into_iter()
        .filter(|r| {
            matches!(
                r.task_status.as_deref(),
                Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
            )
        })
        .take(limit as usize)
        .collect();

    let run_ids: Vec<String> = completed.iter().map(|r| r.id.clone()).collect();
    let metas = agentic_analytics::get_run_metas(db, &run_ids).await?;
    let hint_map: std::collections::HashMap<String, serde_json::Value> = metas
        .into_iter()
        .filter_map(|m| m.spec_hint.map(|h| (m.run_id, h)))
        .collect();

    Ok(completed
        .into_iter()
        .filter_map(|r| {
            let spec_hint = hint_map.get(&r.id).cloned();
            let answer = render_history_answer(
                r.task_status.as_deref(),
                r.answer.as_deref(),
                r.error_message.as_deref(),
            )?;
            Some(ThreadHistoryTurn {
                question: r.question,
                answer,
                spec_hint,
            })
        })
        .collect())
}

fn render_history_answer(
    task_status: Option<&str>,
    answer: Option<&str>,
    error_message: Option<&str>,
) -> Option<String> {
    if let Some(ans) = answer {
        return Some(ans.to_string());
    }
    match task_status {
        Some("failed") | Some("timed_out") => {
            Some(format!("Error: {}", error_message.unwrap_or("run failed")))
        }
        Some("cancelled") => Some(
            error_message
                .map(|m| format!("Cancelled: {m}"))
                .unwrap_or_else(|| "Cancelled by user".to_string()),
        ),
        Some("done") => error_message.map(|e| format!("Error: {e}")),
        _ => None,
    }
}

// ── Headless eval entry-points ──────────────────────────────────────────────

/// What to do with each `Event<AnalyticsEvent>` the pipeline emits during a
/// headless run. `Drain` discards them (eval / fire-and-forget); `Forward`
/// forwards them to a caller-owned sink without ever blocking the solver.
enum EventDestination {
    Drain,
    Forward(
        tokio::sync::mpsc::Sender<agentic_core::events::Event<agentic_analytics::AnalyticsEvent>>,
    ),
}

/// Run an agentic analytics pipeline headlessly for evaluation purposes.
///
/// Returns the answer text, or a human-readable error string if the
/// pipeline suspends / fails. The caller is expected to lift the error
/// back into its own error type (`OxyError` for the eval runner).
pub async fn run_agentic_eval(
    platform: Arc<dyn PlatformContext>,
    config_path: &std::path::Path,
    prompt: String,
) -> Result<String, String> {
    run_agentic_headless(platform, config_path, prompt, EventDestination::Drain).await
}

/// Run ONE turn against an agent's instructions — no tools, no databases, no FSM.
///
/// # Why this exists beside [`run_agentic_eval`]
///
/// `run_agentic_eval` drives the analytics FSM, and that FSM is a warehouse
/// agent: `build_solver_with_context` returns `ConfigError::NoDatabases` before
/// it reads a word of the prompt if no connector was injected. That is correct
/// for an agent whose job is to write SQL, and it makes the same entry point
/// unusable for an agent whose job is to read what it was handed.
///
/// The document librarian behind `POST /api/documents/ask` is the first of
/// those. Its material arrives in the prompt, already filtered by the asker's
/// own read permissions, and giving it a warehouse connector to satisfy the
/// builder would hand a documents question the ability to run a query — the
/// opposite of what that route's design turns on.
///
/// So this is the narrow door: resolve the model the agent's `llm:` block
/// names, take `instructions:` as the system prompt, send one message, return
/// the text. Everything the FSM adds — states, tools, retrieval, subruns — is
/// absent by construction rather than by configuration.
///
/// It still reads the agent's YAML from disk, which is what keeps a caller of
/// this on the ide pod: the instructions are an operator-editable workspace
/// file, not a string compiled into the server.
pub async fn run_agentic_answer(
    platform: Arc<dyn PlatformContext>,
    config_path: &std::path::Path,
    prompt: String,
) -> Result<String, String> {
    /// A ceiling for an agent that names none. An answer over four documents
    /// is a few sentences; a run that produces more than this has misunderstood
    /// the job, and paying for it silently is how a per-question cost becomes a
    /// per-month surprise.
    const DEFAULT_MAX_TOKENS: u32 = 2048;

    let config = AgentConfig::from_file(config_path).map_err(|e| {
        format!(
            "failed to load agentic config at {}: {e}",
            config_path.display()
        )
    })?;

    let info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await
        .ok_or_else(|| {
            format!(
                "no model configured for agent {} — its `llm:` names {:?}, \
                 which does not resolve in this project's config.yml",
                config_path.display(),
                config.llm.model_ref
            )
        })?;

    let client = platform::build_llm_client(&info);
    // An agent with no `instructions:` gets an empty system prompt rather than
    // a default one. A librarian's whole behaviour is in that block, and
    // substituting house instructions for a missing one would answer in a
    // voice nobody wrote.
    let system = config.instructions.as_deref().unwrap_or("");
    client
        .complete_with_max_tokens(
            system,
            &prompt,
            config.llm.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        )
        .await
        .map_err(|e| format!("agent {} failed to answer: {e}", config_path.display()))
}

/// Variant of [`run_agentic_eval`] that forwards each
/// `Event<AnalyticsEvent>` to `event_sink` as it arrives, in addition to
/// returning the final answer text.
///
/// Used by surfaces that want to render intermediate events (Slack SQL
/// artifact capture, chart uploads, etc.) instead of only the final text.
/// Forwarding uses `try_send` so a closed / full caller sink never stalls
/// the solver — events are dropped (with a warn log) on full, which is the
/// right tradeoff for unfanout-able surfaces like Slack where chart render
/// latency can dwarf typical event burst rates.
pub async fn run_agentic_streaming(
    platform: Arc<dyn PlatformContext>,
    config_path: &std::path::Path,
    prompt: String,
    event_sink: tokio::sync::mpsc::Sender<
        agentic_core::events::Event<agentic_analytics::AnalyticsEvent>,
    >,
) -> Result<String, String> {
    run_agentic_headless(
        platform,
        config_path,
        prompt,
        EventDestination::Forward(event_sink),
    )
    .await
}

/// Shared body of [`run_agentic_eval`] and [`run_agentic_streaming`].
/// Loads the config, resolves connectors, builds the solver, spawns the
/// event drain matching `destination`, then runs the orchestrator and
/// maps `OrchestratorError` into a flat error string the caller can lift
/// back into their own error type.
async fn run_agentic_headless(
    platform: Arc<dyn PlatformContext>,
    config_path: &std::path::Path,
    prompt: String,
    destination: EventDestination,
) -> Result<String, String> {
    use agentic_analytics::{
        AnalyticsEvent, AnalyticsIntent, QuestionType, build_analytics_handlers,
        config::BuildContext,
    };
    use agentic_core::events::{Event, EventStream};
    use agentic_core::orchestrator::{Orchestrator, OrchestratorError};

    let ctx_root = platform.context_root().await;
    // See `start_analytics`: keep `ctx_root` alive across context resolution.
    let base_dir = ctx_root.path().to_path_buf();

    let config = AgentConfig::from_file(config_path).map_err(|e| {
        format!(
            "failed to load agentic config at {}: {e}",
            config_path.display()
        )
    })?;

    let mut ctx = BuildContext::default();
    ctx.project_model_info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await;
    ctx.timezone = platform.timezone();

    let mut effective_databases: Vec<String> = config.databases.clone();
    if let Ok(resolved) = config.resolve_context(&base_dir) {
        for db_name in resolved.referenced_databases {
            if !effective_databases.contains(&db_name) {
                effective_databases.push(db_name);
            }
        }
    }
    let resolved = platform::resolve_connectors(&effective_databases, &*platform).await;
    let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
    connectors.extend(resolved.pre_built);
    ctx.extra_default_connector = effective_databases
        .iter()
        .find(|name| connectors.contains_key(*name))
        .cloned();
    ctx.extra_connectors = connectors;

    let (solver, automation_files) = config
        .build_solver_with_context(&base_dir, ctx)
        .await
        .map_err(|e| format!("failed to build agentic solver: {e}"))?;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Event<AnalyticsEvent>>(256);
    let event_stream: EventStream<AnalyticsEvent> = event_tx;
    tokio::spawn(async move {
        match destination {
            EventDestination::Drain => while event_rx.recv().await.is_some() {},
            EventDestination::Forward(sink) => {
                while let Some(ev) = event_rx.recv().await {
                    // `try_send` so a slow / closed consumer never stalls
                    // the solver. Full / closed channels drop the event;
                    // a closed sink also breaks the loop so we don't burn
                    // CPU forwarding into the void.
                    match sink.try_send(ev) {
                        Ok(()) => {}
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            tracing::warn!("agentic event sink full; dropping event");
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
            }
        }
    });

    let solver = solver.with_events(event_stream.clone());
    // Same rule as the two PipelineBuilder sites: only inject a
    // automation runner when the agent's `context:` actually resolved
    // some files. An empty list would fall through to
    // `workspace.list_automation_files()` (full project scan), giving
    // the FSM access to every automation in the project — which has
    // caused a runaway subrun chain when a misparsed agent had no
    // concrete instructions.
    let solver = if automation_files.is_empty() {
        solver
    } else {
        let workspace: Arc<dyn agentic_automation::WorkspaceContext> = platform.clone();
        let runner = agentic_automation::OxyAutomationRunner::new(workspace)
            .with_automation_files(automation_files);
        solver.with_subrun_runner(std::sync::Arc::new(runner))
    };

    let mut orchestrator = Orchestrator::new(solver).with_handlers(build_analytics_handlers());

    let intent = AnalyticsIntent {
        raw_question: prompt,
        summary: String::new(),
        question_type: QuestionType::SingleValue,
        metrics: vec![],
        dimensions: vec![],
        filters: vec![],
        history: vec![],
        spec_hint: None,
        selected_automation: None,
        semantic_query: Default::default(),
        semantic_confidence: 0.0,
    };

    orchestrator
        .run(intent)
        .await
        .map(|answer| answer.text)
        .map_err(|e| match e {
            OrchestratorError::Suspended { reason, .. } => {
                let questions = match reason {
                    agentic_core::SuspendReason::HumanInput { questions } => questions,
                    _ => vec![],
                };
                let prompts: Vec<_> = questions.iter().map(|q| q.prompt.as_str()).collect();
                format!(
                    "agentic pipeline asked a clarifying question: {}",
                    prompts.join("; ")
                )
            }
            OrchestratorError::MaxIterationsExceeded => "max iterations exceeded".into(),
            OrchestratorError::ResumeNotSupported => "resume not supported".into(),
            OrchestratorError::Fatal(e) => format!("fatal: {e:?}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: every queue-driven domain must be in
    /// `build_event_registry`, or `stream_events` has no `RowProcessor`
    /// for its rows and the SSE stream silently drops them (the bug
    /// where the airway run page only saw the generic task row).
    #[test]
    fn build_event_registry_routes_airway_events() {
        let registry = build_event_registry();
        let mut proc = registry.stream_processor(AIRWAY_SOURCE_TYPE);
        let out = proc.process(
            "load_started",
            &serde_json::json!({
                "event_type": "load_started",
                "pipeline_name": "p",
                "load_id": "l"
            }),
        );
        assert!(
            out.iter().any(|(ty, _)| ty == "load_started"),
            "airway events must survive the registry, got {out:?}"
        );
    }
}
