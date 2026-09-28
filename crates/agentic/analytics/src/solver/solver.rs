use std::collections::HashMap;
use std::sync::Arc;

use chrono_tz::Tz;

use agentic_connector::DatabaseConnector;
use agentic_core::{
    events::EventStream,
    human_input::{DeferredInputProvider, HumanInputHandle, ResumeInput, SuspendedRunData},
};

use crate::config::StateConfig;
use crate::engine::SemanticEngine;
use crate::events::AnalyticsEvent;
use crate::llm::{LlmClient, ThinkingConfig};
use crate::metric_sink::SharedMetricSink;
use crate::metric_tree_runner::MetricTreeRunner;
use crate::semantic::SemanticCatalog;
use crate::tools::{SchemaCache, new_schema_cache};
use crate::validation::Validator;
use agentic_core::subrun::SubrunRunner;

use super::prompts::QUESTION_TYPE_DEFS;

// AnalyticsSolver

/// Solver for the analytics domain.
///
/// Pipeline stages are split across per-state submodules; this struct holds
/// all shared state and wires together the [`DomainSolver`] trait impl.
pub struct AnalyticsSolver {
    pub(crate) client: LlmClient,
    pub(crate) catalog: Arc<SemanticCatalog>,
    /// All configured connectors, keyed by logical name.
    pub(crate) connectors: HashMap<String, Arc<dyn DatabaseConnector>>,
    /// Logical name of the default connector.
    pub(crate) default_connector: String,
    /// Optional event stream for emitting LLM and domain events.
    pub(crate) event_tx: Option<EventStream<AnalyticsEvent>>,
    /// Global instructions injected into every LLM call.
    pub(crate) global_instructions: Option<String>,
    /// Timezone used to format the "Today is …" date hint and to resolve
    /// relative dates ("yesterday", "last week"). Sourced from `timezone:`
    /// in the agent YAML. `None` falls back to UTC (legacy behavior).
    pub(crate) timezone: Option<Tz>,
    /// Precomputed ambient data-freshness hint (one line, e.g. "Data
    /// coverage: sales_daily through 2026-07-17 (several times daily); ...").
    /// Computed once per pipeline build from live MAX(watermark) probes
    /// (TTL-cached process-wide); appended to the uncached dynamic suffix
    /// beside the date hint. `None` when no view has a freshness target.
    pub(crate) freshness_hint: Option<String>,
    /// SQL example files appended to the Solving prompt.
    pub(crate) sql_examples: Vec<String>,
    /// Markdown domain docs injected into Clarifying and Interpreting prompts.
    pub(crate) domain_docs: Vec<String>,
    /// Per-state config overrides (thinking config, max_retries, instructions).
    pub(crate) state_configs: HashMap<String, StateConfig>,
    /// Per-state LLM clients built from `states.<name>.model` overrides.
    ///
    /// Only populated for states that specify an explicit `model:` in their
    /// `StateConfig`.  Falls back to `self.client` for all other states.
    pub(crate) state_clients: HashMap<String, LlmClient>,
    /// Global thinking config that overrides the per-state default when set.
    pub(crate) global_thinking: Option<ThinkingConfig>,
    /// When true, `global_thinking` takes absolute precedence over per-state
    /// thinking configs.  Set by extended thinking mode to ensure the runtime override
    /// wins everywhere.
    pub(crate) extended_thinking_active: bool,
    /// Human input provider for `ask_user` tool calls.
    ///
    /// Defaults to [`DeferredInputProvider`] (always suspends).
    pub(crate) human_input: HumanInputHandle,
    /// Suspension data stored by the `ask_user` tool handler.
    pub(crate) suspension_data: Option<SuspendedRunData>,
    /// Resume input injected before re-entering the pipeline.
    pub(crate) resume_data: Option<ResumeInput>,
    /// Optional external automation runner.
    ///
    /// When set, the executing stage delegates `SolutionSource::Automation`
    /// solutions to this runner instead of the SQL connector.
    pub(crate) subrun_runner: Option<Arc<dyn SubrunRunner>>,
    /// Optional metric-tree runner for `explain` / `opportunity` /
    /// `sensitivity` / `predict` agent tools. `None` disables those tools
    /// (CLI, tests, and contexts without a semantic model).
    pub(crate) metric_tree_runner: Option<Arc<dyn MetricTreeRunner>>,
    /// Optional anomaly store for `list_anomalies` / `detect_anomalies` /
    /// `explain_anomaly` tools inside the `RootCause` inquiry loop.
    /// `None` disables those tools.
    pub(crate) anomaly_store: Option<Arc<dyn crate::anomaly_store::AnomalyStore>>,
    /// Workspace that owns this run. Used to scope anomaly store queries.
    pub(crate) workspace_id: uuid::Uuid,
    /// Configured validator for all three pipeline stages.
    pub(crate) validator: Validator,
    /// Initial max output tokens per LLM call, sourced from `llm.max_tokens` in config.
    ///
    /// When `Some`, used as `max_tokens_override` in every [`ToolLoopConfig`].
    /// Resume continuations take precedence and may double this value.
    pub(crate) max_tokens: Option<u32>,
    /// Optional vendor semantic engine for the VendorEngine execution path.
    ///
    /// When `Some`, the Specifying handler attempts to translate the intent via
    /// the engine before falling back to `try_compile` / LLM.  The Executing
    /// handler dispatches `SolutionPayload::Vendor` queries here.
    pub(crate) engine: Option<Arc<dyn SemanticEngine>>,
    /// Lazy schema cache for database lookup tools (`list_tables`, `describe_table`).
    ///
    /// Populated on first tool call per connector, then reused for the session.
    pub(crate) schema_cache: SchemaCache,
    /// Identifier of the agentic config driving this run. Used as
    /// `source_ref` when writing metric-usage records to the Metrics tab.
    pub(crate) agent_id: String,
    /// Original user question. Captured on the metric-usage context JSON
    /// so the Metrics detail view can show it.
    pub(crate) question: String,
    /// Optional port for recording Tier 1 metric usage (measures +
    /// dimensions) to whatever backend the host has wired up. `None`
    /// disables metric recording — the pipeline still runs normally.
    pub(crate) metric_sink: Option<SharedMetricSink>,
    /// SQL-generation mode flag. When `true`, the executing stage
    /// terminates the pipeline with the candidate SQL as the answer
    /// instead of materialising rows or running interpreting.
    /// Pre-validated solution paths (semantic-model, verified `.sql`,
    /// vendor engine) skip execution entirely; LLM-generated SQL
    /// runs a `LIMIT 0` smoke check first. Automation delegation is
    /// rejected with an explicit error in this mode.
    pub(crate) sql_generation_mode: bool,
    /// Root directory of the workspace / project.
    ///
    /// Used to resolve workspace-relative paths returned by `search_automations`
    /// (e.g. `example_sql/total_number_of_store.sql`) before reading the file.
    pub(crate) workspace_path: Option<std::path::PathBuf>,
    /// The local-rollup short-circuit: cache key, Layer-1 refresh-key cache,
    /// renewal threshold, and the cross-node Parquet read-through.
    ///
    /// When set, the Specifying stage consults the local Parquet manifest
    /// after a successful airlayer compile and swaps `SolutionPayload::Sql`
    /// for `SolutionPayload::Preaggregation` whenever a covering rollup
    /// exists. `None` (CLI, tests, runs with no preagg worker) always routes
    /// to the warehouse.
    ///
    /// One field rather than the three it replaced (cache, threshold, scan
    /// path) — they were never independently meaningful, and the scan path
    /// among them was a cache key that a caller could and did set to a
    /// directory nothing had built.
    pub(crate) preagg: Option<agentic_semantic::compile::PreaggContext>,
}

impl AnalyticsSolver {
    /// Create a solver with a single connector registered as `"default"`.
    pub fn new(
        client: LlmClient,
        catalog: SemanticCatalog,
        connector: Box<dyn DatabaseConnector>,
    ) -> Self {
        let mut connectors: HashMap<String, Arc<dyn DatabaseConnector>> = HashMap::new();
        connectors.insert("default".to_string(), Arc::from(connector));
        Self {
            client,
            catalog: Arc::new(catalog),
            connectors,
            default_connector: "default".to_string(),
            event_tx: None,
            global_instructions: None,
            timezone: None,
            freshness_hint: None,
            sql_examples: vec![],
            domain_docs: vec![],
            state_configs: HashMap::new(),
            state_clients: HashMap::new(),
            global_thinking: None,
            extended_thinking_active: false,
            human_input: Arc::new(DeferredInputProvider),
            suspension_data: None,
            resume_data: None,
            subrun_runner: None,
            metric_tree_runner: None,
            anomaly_store: None,
            workspace_id: uuid::Uuid::nil(),
            validator: Validator::default_validator(),
            max_tokens: None,
            engine: None,
            schema_cache: new_schema_cache(),
            agent_id: String::new(),
            question: String::new(),
            metric_sink: None,
            sql_generation_mode: false,
            preagg: None,
            workspace_path: None,
        }
    }

    /// Create a solver with multiple named database connectors.
    pub fn new_multi(
        client: LlmClient,
        catalog: SemanticCatalog,
        connectors: HashMap<String, Arc<dyn DatabaseConnector>>,
        default_connector: String,
    ) -> Self {
        Self {
            client,
            catalog: Arc::new(catalog),
            connectors,
            default_connector,
            event_tx: None,
            global_instructions: None,
            timezone: None,
            freshness_hint: None,
            sql_examples: vec![],
            domain_docs: vec![],
            state_configs: HashMap::new(),
            state_clients: HashMap::new(),
            global_thinking: None,
            extended_thinking_active: false,
            human_input: Arc::new(DeferredInputProvider),
            suspension_data: None,
            resume_data: None,
            subrun_runner: None,
            metric_tree_runner: None,
            anomaly_store: None,
            workspace_id: uuid::Uuid::nil(),
            validator: Validator::default_validator(),
            max_tokens: None,
            engine: None,
            schema_cache: new_schema_cache(),
            agent_id: String::new(),
            question: String::new(),
            metric_sink: None,
            sql_generation_mode: false,
            preagg: None,
            workspace_path: None,
        }
    }

    /// Set the human input provider for `ask_user` tool calls.
    pub fn with_human_input(mut self, provider: HumanInputHandle) -> Self {
        self.human_input = provider;
        self
    }

    pub fn with_events(mut self, tx: EventStream<AnalyticsEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Record the agentic config id and user question driving this run.
    /// Both feed the metric-usage records emitted during the Executing stage
    /// so the Metrics observability tab can attribute analytics activity.
    pub fn with_source_attribution(
        mut self,
        agent_id: impl Into<String>,
        question: impl Into<String>,
    ) -> Self {
        self.agent_id = agent_id.into();
        self.question = question.into();
        self
    }

    /// Enable SQL-generation mode. See the field doc on
    /// [`AnalyticsSolver::sql_generation_mode`] for semantics.
    pub fn with_sql_generation_mode(mut self, enabled: bool) -> Self {
        self.sql_generation_mode = enabled;
        self
    }

    /// Attach a [`SharedMetricSink`] so the Executing stage can record
    /// the measures and dimensions touched by each query. When
    /// unset, metric recording is a silent no-op.
    pub fn with_metric_sink(mut self, sink: Option<SharedMetricSink>) -> Self {
        self.metric_sink = sink;
        self
    }

    /// Set global instructions injected into every LLM call.
    pub fn with_global_instructions(mut self, instructions: Option<String>) -> Self {
        self.global_instructions = instructions;
        self
    }

    /// Set the timezone used to resolve the "Today is …" date hint.
    ///
    /// `None` falls back to UTC (legacy behavior).
    pub fn with_timezone(mut self, tz: Option<Tz>) -> Self {
        self.timezone = tz;
        self
    }

    /// Set SQL example snippets appended to the Solving system prompt.
    pub fn with_sql_examples(mut self, examples: Vec<String>) -> Self {
        self.sql_examples = examples;
        self
    }

    /// Set markdown domain docs injected into Clarifying and Interpreting prompts.
    pub fn with_domain_docs(mut self, docs: Vec<String>) -> Self {
        self.domain_docs = docs;
        self
    }

    /// Set per-state config overrides.
    pub fn with_state_configs(mut self, configs: HashMap<String, StateConfig>) -> Self {
        self.state_configs = configs;
        self
    }

    /// Set per-state LLM client overrides built from `states.<name>.model`.
    pub fn with_state_clients(mut self, clients: HashMap<String, LlmClient>) -> Self {
        self.state_clients = clients;
        self
    }

    /// Return the LLM client for `state`, falling back to the global client.
    pub(crate) fn client_for_state(&self, state: &str) -> &LlmClient {
        self.state_clients.get(state).unwrap_or(&self.client)
    }

    /// Set a global thinking config applied to every pipeline state.
    pub fn with_global_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.global_thinking = Some(thinking);
        self
    }

    /// Override the global LLM client and clear per-state model overrides.
    ///
    /// Used for runtime preset overrides (e.g. "extended thinking" mode from the UI).
    /// Per-state **model** clients are cleared so all states use the
    /// overridden client.  Sets `extended_thinking_active` so that the extended thinking
    /// config also takes precedence over per-state overrides.
    pub fn with_client_override(mut self, client: LlmClient) -> Self {
        self.client = client;
        self.state_clients.clear();
        self.extended_thinking_active = true;
        self
    }

    /// Attach an external automation runner for `SolutionSource::Automation` solutions.
    /// Attach an external subrun runner for `SolutionSource::Automation` solutions.
    pub fn with_subrun_runner(mut self, runner: Arc<dyn SubrunRunner>) -> Self {
        self.subrun_runner = Some(runner);
        self
    }

    /// Attach a metric-tree runner for the `explain` / `opportunity` /
    /// `sensitivity` / `predict` agent tools. Without one, those tools are
    /// excluded from the tool list exposed to the LLM.
    pub fn with_metric_tree_runner(mut self, runner: Arc<dyn MetricTreeRunner>) -> Self {
        self.metric_tree_runner = Some(runner);
        self
    }

    pub fn with_anomaly_store(
        mut self,
        store: Arc<dyn crate::anomaly_store::AnomalyStore>,
    ) -> Self {
        self.anomaly_store = Some(store);
        self
    }

    pub fn with_workspace_id(mut self, workspace_id: uuid::Uuid) -> Self {
        self.workspace_id = workspace_id;
        self
    }

    /// Replace the semantic catalog at runtime.
    ///
    /// Used by the coordinator-worker resume path to reload the catalog
    /// after a builder delegation has modified `.view.yml` files on disk.
    pub fn replace_catalog(&mut self, catalog: Arc<SemanticCatalog>) {
        self.catalog = catalog;
    }

    /// Override the default validator with a custom-configured one.
    pub fn with_validator(mut self, validator: Validator) -> Self {
        self.validator = validator;
        self
    }

    /// Set the initial max output tokens applied to every LLM call.
    pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Attach a vendor semantic engine for the VendorEngine execution path.
    pub fn with_engine(mut self, engine: Arc<dyn SemanticEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Wire the local-rollup short-circuit. `None` reverts the solver to
    /// warehouse-only execution, which is what the CLI, tests, and any run
    /// without a rebuild worker want: without one there is no guarantee a
    /// local Parquet is current.
    pub fn with_preagg(mut self, preagg: Option<agentic_semantic::compile::PreaggContext>) -> Self {
        self.preagg = preagg;
        self
    }

    pub fn with_workspace_path(mut self, path: std::path::PathBuf) -> Self {
        self.workspace_path = Some(path);
        self
    }

    /// Wrap compiled warehouse SQL into the appropriate `SolutionPayload`:
    /// `LocalParquet` when a fresh local rollup covers the request, otherwise
    /// raw `Sql`. Single source of truth — every pipeline stage that builds a
    /// semantic-model solution payload must go through this method so the
    /// preagg short-circuit is applied consistently.
    pub(crate) fn build_semantic_payload(
        &self,
        sql: String,
        request: &oxy_airlayer_compat::engine::query::QueryRequest,
    ) -> crate::types::SolutionPayload {
        use crate::types::SolutionPayload;
        match self.preagg.as_ref() {
            Some(preagg) => {
                // The live set comes from the CATALOG, which is an airlayer
                // engine and always carries the view list. This used to pass
                // `None` — "don't check liveness" — on the reasoning that the
                // solver's `engine` is the vendor abstraction (cube / looker)
                // and has no airlayer views, so an empty set would decline
                // every rollup and silently switch pre-aggregation off for the
                // analytics agent. The premise was about the wrong field:
                // `self.engine` never reaches this method. Both callers
                // (`specifying`, `clarifying`) compiled `sql` through
                // `self.catalog.engine()` immediately above, so the schema
                // that produced the query is right here, and `None` was giving
                // up a check it could afford.
                //
                // It is not a missed optimisation. `covers()` matches member
                // NAMES, so an edit that renames nothing — `expr: amount` to
                // `expr: amount - refunds`, `type: sum` to `type: avg`, a new
                // measure `filters:` entry — moves the rollup's hash and
                // leaves the old manifest row matching. Under `None` that row
                // answered, and the pre-edit numbers went out under the
                // Pre-aggregated badge with nothing to say which definition
                // they came from. Declining sends the query to the warehouse,
                // which is slower and right.
                //
                // How long "slower" lasts is not this call's to promise. The
                // heartbeat rebuilds a rollup only when it has a refresh key
                // to judge staleness by (`preagg_executor`, which skips the
                // rest as `RollupSkippedNoRefreshKey`), so a keyless rollup
                // edited in place stays declined until someone forces a
                // rebuild. That is the pre-existing shape of the rebuild
                // cycle, not something this check introduces — `compile_against`
                // and the metric-tree runner have always declined the same
                // rows — and the status surface now reads "not built" rather
                // than "cached", which points at the button that fixes it.
                match agentic_semantic::compile::try_resolve_preagg(
                    preagg,
                    request,
                    &sql,
                    "",
                    Some(self.catalog.live_rollups()),
                ) {
                    Some(agentic_semantic::compile::CompiledQuery::Preaggregation {
                        preagg_sql,
                        source,
                        warehouse_sql,
                        ..
                    }) => SolutionPayload::Preaggregation {
                        preagg_sql,
                        source,
                        warehouse_sql,
                    },
                    _ => SolutionPayload::Sql(sql),
                }
            }
            _ => SolutionPayload::Sql(sql),
        }
    }

    /// Day-only date hint that is appended to the system prompt as a
    /// separate, uncached content block (Anthropic) or concatenated
    /// (other providers).  Kept out of the cached prefix so cross-thread
    /// runs can share the system+tools cache.
    ///
    /// Formatted in `tz` when set, otherwise UTC. Using UTC unconditionally
    /// (the historical behavior) makes the hint a calendar day ahead of the
    /// user's local date during the evening — e.g. 9pm Pacific is already
    /// "tomorrow" in UTC — so relative dates like "yesterday" resolve to the
    /// wrong day. Set `timezone:` in the agent YAML to pin it to the
    /// operating timezone.
    pub(crate) fn current_date_hint(tz: Option<Tz>) -> String {
        Self::format_date_hint(chrono::Utc::now(), tz)
    }

    /// Pure formatting core of [`current_date_hint`](Self::current_date_hint),
    /// split out so the timezone handling can be exercised against a fixed
    /// instant in tests.
    pub(crate) fn format_date_hint(
        now_utc: chrono::DateTime<chrono::Utc>,
        tz: Option<Tz>,
    ) -> String {
        let date = match tz {
            Some(tz) => now_utc.with_timezone(&tz).format("%Y-%m-%d").to_string(),
            None => now_utc.format("%Y-%m-%d").to_string(),
        };
        format!("Today is {date}.")
    }

    /// The full uncached dynamic system suffix for this run: the date hint,
    /// plus the ambient data-freshness hint when one was computed. Kept out
    /// of the cached system prefix (same rationale as the date hint - the
    /// coverage dates advance daily and would otherwise invalidate every
    /// prompt-cache hit).
    pub(crate) fn system_hint(&self) -> String {
        let date = Self::current_date_hint(self.timezone);
        match &self.freshness_hint {
            Some(f) => format!("{date}\n{f}"),
            None => date,
        }
    }

    /// Build a composite system prompt for a given state.
    ///
    /// Composition order:
    /// 1. Hardcoded `base` system prompt.
    /// 2. `QUESTION_TYPE_DEFS` for clarifying/specifying states.
    /// 3. `global_instructions` from config.
    /// 4. Per-state `instructions` from `state_configs`.
    /// 5. `domain_docs` (clarifying + interpreting).
    /// 6. SQL dialect + `sql_examples` (specifying + solving).
    pub(crate) fn build_system_prompt(
        &self,
        state: &str,
        base: &str,
        dialect: Option<&str>,
    ) -> String {
        let mut parts = vec![base.to_string()];

        match state {
            "clarifying" | "specifying" => {
                parts.push(QUESTION_TYPE_DEFS.to_string());
            }
            _ => {}
        }

        if let Some(global) = &self.global_instructions
            && !global.trim().is_empty()
        {
            parts.push(format!(
                "<global_instructions>\n{}\n</global_instructions>",
                global.trim()
            ));
        }

        if let Some(state_cfg) = self.state_configs.get(state)
            && let Some(state_instr) = &state_cfg.instructions
            && !state_instr.trim().is_empty()
        {
            parts.push(format!(
                "<state_instructions>\n{}\n</state_instructions>",
                state_instr.trim()
            ));
        }

        match state {
            "clarifying" | "interpreting" => {
                for doc in &self.domain_docs {
                    if !doc.trim().is_empty() {
                        parts.push(format!(
                            "<domain_context>\n{}\n</domain_context>",
                            doc.trim()
                        ));
                    }
                }
            }
            "specifying" | "solving" => {
                if let Some(d) = dialect {
                    parts.push(format!(
                        "<sql_dialect>{d} — use {d}-compatible syntax, \
                         including its identifier quoting and built-in functions.</sql_dialect>"
                    ));
                }
                if state == "solving" && !self.sql_examples.is_empty() {
                    let examples: Vec<String> = self
                        .sql_examples
                        .iter()
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| format!("<sql_example>\n{}\n</sql_example>", s.trim()))
                        .collect();
                    if !examples.is_empty() {
                        parts.push(format!(
                            "<sql_examples>\n{}\n</sql_examples>",
                            examples.join("\n")
                        ));
                    }
                }
            }
            _ => {}
        }

        parts.join("\n\n")
    }

    /// Return the [`ThinkingConfig`] for a state.
    ///
    /// When extended thinking mode is active the global thinking override wins
    /// unconditionally.  Otherwise the priority is:
    /// per-state config > global_thinking override > `default`.
    pub(crate) fn thinking_for_state(
        &self,
        state: &str,
        default: ThinkingConfig,
    ) -> ThinkingConfig {
        // Extended thinking override takes absolute precedence.
        if self.extended_thinking_active
            && let Some(global) = &self.global_thinking
        {
            return global.clone();
        }
        if let Some(t) = self
            .state_configs
            .get(state)
            .and_then(|c| c.thinking.as_ref())
            .map(|t| t.to_thinking_config())
        {
            return t;
        }
        if let Some(global) = &self.global_thinking {
            return global.clone();
        }
        default
    }

    /// Return the max tool rounds for a state, falling back to `default`.
    pub(crate) fn max_tool_rounds_for_state(&self, state: &str, default: u32) -> u32 {
        self.state_configs
            .get(state)
            .and_then(|c| c.max_retries)
            .unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    /// 2026-06-16T04:28:00Z is 2026-06-15 21:28 in America/Los_Angeles (PDT,
    /// UTC-7): the UTC calendar date (the 16th) is already a day ahead of the
    /// Pacific date (the 15th). This is exactly the evening window where the
    /// old UTC-only hint told the model "today is the 16th", so "yesterday"
    /// resolved to the 15th — the user's actual today.
    fn evening_instant() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 16, 4, 28, 0).unwrap()
    }

    #[test]
    fn date_hint_without_timezone_uses_utc() {
        assert_eq!(
            AnalyticsSolver::format_date_hint(evening_instant(), None),
            "Today is 2026-06-16.",
        );
    }

    #[test]
    fn date_hint_with_timezone_uses_local_calendar_day() {
        // Pacific is still the 15th when UTC has already rolled to the 16th —
        // this is the regression the timezone config fixes.
        assert_eq!(
            AnalyticsSolver::format_date_hint(
                evening_instant(),
                Some(chrono_tz::America::Los_Angeles),
            ),
            "Today is 2026-06-15.",
        );
    }

    #[test]
    fn date_hint_with_timezone_agrees_with_utc_before_rollover() {
        // 2026-06-15T18:00:00Z is 11:00 Pacific the same day: no date skew.
        let midday = Utc.with_ymd_and_hms(2026, 6, 15, 18, 0, 0).unwrap();
        assert_eq!(
            AnalyticsSolver::format_date_hint(midday, Some(chrono_tz::America::Los_Angeles)),
            "Today is 2026-06-15.",
        );
    }
}
