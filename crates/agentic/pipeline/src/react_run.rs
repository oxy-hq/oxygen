//! Analytics on the react runtime: the one place a react run is assembled
//! ([`build_react_run`], shared by the headless and served paths), the run
//! itself ([`run_react`]), and the headless branch that turns its outcome
//! into the FSM's answer-or-error contract.
//!
//! A react run takes its model, thinking and date hint from the same
//! `AgentConfig` readers the FSM's solver uses, so a comparison of the two
//! runtimes runs on one model by construction.

use std::path::Path;
use std::sync::Arc;

use agentic_analytics::AnalyticsEvent;
use agentic_analytics::SemanticCatalog;
use agentic_analytics::airlayer_compat::build_dialect_map;
use agentic_analytics::config::{AgentConfig, AgentRuntime, BuildContext, ConfigError};
use agentic_analytics::react::run::{ReactAnalytics, ReactInputs, emit_runtime_selected, finish};
use agentic_analytics::react::{WorkspaceFiles, date_hint};
use agentic_core::events::{CoreEvent, Event};
use agentic_core::hub_task::spawn_with_hub;
use agentic_core::react::{Engine, EventSink, PriorTurn, Transcript};
use agentic_llm::LlmEngine;
use agentic_llm::engine::EngineConfig;
use agentic_runtime::react::{LoopConfig, LoopOutcome, ResultStore, run_loop};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::headless::{EventDestination, HeadlessError};
use crate::platform::PlatformContext;

/// Everything one react analytics question runs on.
pub(crate) struct ReactRun {
    /// `LlmEngine` in production; tests substitute a scripted engine.
    pub engine: Box<dyn Engine>,
    pub analytics: ReactAnalytics,
    /// Shared by the loop's tools and [`finish`], which reads the cited SQL.
    pub results: Arc<ResultStore>,
    pub system: String,
}

/// Whether `config` opts into the react runtime. An agent with no
/// `runtime:` key stays on the FSM.
fn runs_react(config: &AgentConfig) -> bool {
    config.runtime == AgentRuntime::React
}

/// Whether a headless entry point may honor an agent's `runtime: react`.
/// Each entry states its own; there is no default, so a new caller has to
/// choose. The pilot is internal and benchmark only: until PR B's per-org
/// `react_runtime` flag exists, only the eval harness entry allows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReactPolicy {
    /// Honor the `.agentic.yml` opt-in (the eval harness, `oxy test`).
    Allow,
    /// Run the FSM whatever the agent asks for (Slack, the health probe).
    Never,
}

/// [`crate::run_agentic_eval`]'s policy: the Workspace Health probe answers on
/// the FSM until the pilot flag lands.
pub(crate) const EVAL_ENTRY_REACT: ReactPolicy = ReactPolicy::Never;
/// [`crate::run_agentic_streaming`]'s policy: Slack answers on the FSM.
pub(crate) const STREAMING_ENTRY_REACT: ReactPolicy = ReactPolicy::Never;
/// [`crate::run_agentic_streaming_with_states`]'s policy: the eval harness is
/// the one entry that runs the react pilot.
pub(crate) const HARNESS_ENTRY_REACT: ReactPolicy = ReactPolicy::Allow;

/// Whether this run goes to the react runtime: the agent opts in and the
/// entry's `policy` allows it. An opt-in the policy refuses is logged, so a
/// react agent answering on the FSM is visible rather than silent.
pub(crate) fn picks_react(config: &AgentConfig, policy: ReactPolicy, agent_ref: &str) -> bool {
    if !runs_react(config) {
        return false;
    }
    match policy {
        ReactPolicy::Allow => true,
        ReactPolicy::Never => {
            tracing::info!(
                agent_ref,
                "agent opts into runtime: react; ignored on this entry point, which runs \
                 the fsm until the react_runtime flag lands"
            );
            false
        }
    }
}

/// Assembles a react run from the same inputs the FSM's solver build reads:
/// the context globs, the injected connectors, the semantic files, and the
/// agent's model, thinking and timezone.
pub(crate) async fn build_react_run(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    mut ctx: BuildContext,
    base_dir: &Path,
) -> Result<ReactRun, String> {
    let fail = |e: ConfigError| format!("failed to build agentic solver: {e}");
    if ctx.extra_connectors.is_empty() {
        return Err(fail(ConfigError::NoDatabases));
    }
    if ctx.genai.workspace_id.is_none() {
        let id = platform.workspace_id();
        ctx.genai.workspace_id = Some(id).filter(|id| !id.is_nil()).map(|id| id.to_string());
    }
    let catalog = load_catalog(config, &ctx, base_dir).map_err(fail)?;
    let files = WorkspaceFiles::load(base_dir, &config.context, ctx.domain_docs.as_deref())
        .map_err(|e| format!("failed to read the agent's context files: {e}"))?;
    let tz = config.effective_timezone(ctx.timezone).map_err(fail)?;
    let engine = LlmEngine::new(
        config.solving_llm_client(&ctx),
        EngineConfig {
            thinking: config.solving_thinking(&ctx),
            system_date_hint: Some(date_hint(tz)),
            ..EngineConfig::default()
        },
    );
    let analytics = ReactAnalytics::new(ReactInputs {
        instructions: config.instructions.clone().unwrap_or_default(),
        databases: ctx.extra_connectors,
        catalog,
        files: Arc::new(files),
    });
    Ok(ReactRun {
        engine: Box::new(engine),
        system: analytics.toolset.system_prompt.clone(),
        analytics,
        results: Arc::new(ResultStore::new()),
    })
}

/// The semantic catalog over the agent's `context:` semantic files, compiled
/// with the dialects of the injected connectors (the FSM's default-connector
/// rule). `None` when the globs reach no semantic file.
fn load_catalog(
    config: &AgentConfig,
    ctx: &BuildContext,
    base_dir: &Path,
) -> Result<Option<Arc<SemanticCatalog>>, ConfigError> {
    let resolved = config.resolve_context(base_dir)?;
    if resolved.semantic_files.is_empty() {
        return Ok(None);
    }
    let connectors = &ctx.extra_connectors;
    let default = ctx
        .extra_default_connector
        .clone()
        .or_else(|| connectors.keys().min().cloned())
        .unwrap_or_default();
    let dialects = build_dialect_map(connectors, &default);
    let catalog = SemanticCatalog::load_files(&resolved.semantic_files, dialects)
        .map_err(ConfigError::SemanticError)?;
    Ok(Some(Arc::new(catalog)))
}

/// Runs one question: `RuntimeSelected` first, then the loop, then
/// [`finish`]. On `Done` the answer text comes back with its citation
/// markers stripped.
pub(crate) async fn run_react(
    run: ReactRun,
    run_id: String,
    prior: Vec<PriorTurn>,
    question: String,
    ev: EventSink,
    cancel: CancellationToken,
) -> LoopOutcome {
    emit_runtime_selected(&ev).await;
    let transcript = Transcript::seeded(run.system, &prior, question);
    let cfg = LoopConfig {
        budgets: run.analytics.budgets.clone(),
        run_id,
        results: run.results.clone(),
    };
    let toolset = &run.analytics.toolset;
    let mut outcome = run_loop(&*run.engine, toolset, transcript, &cfg, &ev, &cancel).await;
    if let Some(text) = finish(&outcome, &*run.results, &ev).await
        && let LoopOutcome::Done { answer } = &mut outcome
    {
        answer.text = text;
    }
    outcome
}

/// A runtime row as the event the FSM's consumers read: the payload re-tagged
/// with its type, tried as a [`CoreEvent`] and then as an
/// [`AnalyticsEvent`]. The loop's own `react_*` rows have no FSM shape.
pub(crate) fn row_to_event(event_type: &str, payload: &Value) -> Option<Event<AnalyticsEvent>> {
    if event_type.starts_with("react_") {
        return None;
    }
    let mut tagged = payload.as_object().cloned().unwrap_or_default();
    tagged.insert("event_type".into(), Value::String(event_type.to_string()));
    let tagged = Value::Object(tagged);
    if let Ok(core) = serde_json::from_value::<CoreEvent>(tagged.clone()) {
        return Some(Event::Core(core));
    }
    match serde_json::from_value::<AnalyticsEvent>(tagged) {
        Ok(domain) => Some(Event::Domain(domain)),
        Err(e) => {
            // A runtime row with no FSM shape: payload drift, made visible.
            tracing::warn!(event_type, error = %e, "react row matches no FSM event; dropped");
            None
        }
    }
}

/// The headless branch: build the run, then [`run_react_headless`].
pub(crate) async fn run_headless(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    mut ctx: BuildContext,
    run: (&str, &Path),
    prompt: String,
    destination: EventDestination,
) -> Result<String, HeadlessError> {
    let (agent_ref, base_dir) = run;
    ctx.genai.agent_name = Some(agent_ref.to_string());
    let run = build_react_run(platform, config, ctx, base_dir)
        .await
        .map_err(HeadlessError::Run)?;
    run_react_headless(run, prompt, destination).await
}

/// Runs `run` with no prior turns, pumping its rows into `destination`, and
/// maps the outcome to the FSM's headless contract.
pub(crate) async fn run_react_headless(
    run: ReactRun,
    prompt: String,
    destination: EventDestination,
) -> Result<String, HeadlessError> {
    let (tx, rx) = tokio::sync::mpsc::channel::<(String, Value)>(256);
    let pump = spawn_with_hub(pump_rows(rx, destination));
    let run_id = uuid::Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    let outcome = run_react(run, run_id, Vec::new(), prompt, tx, cancel).await;
    // Every sender is gone once `run_react` returns, so the pump drains
    // what is left and ends: the caller's sink holds every row on return.
    if let Err(e) = pump.await {
        tracing::warn!(error = %e, "react event pump ended abnormally");
    }
    headless_result(outcome)
}

pub(crate) fn headless_result(outcome: LoopOutcome) -> Result<String, HeadlessError> {
    match outcome {
        LoopOutcome::Done { answer } => Ok(answer.text),
        LoopOutcome::Suspended { question, .. } => Err(HeadlessError::Run(format!(
            "agentic pipeline asked a clarifying question: {question}"
        ))),
        LoopOutcome::Failed { message } => Err(HeadlessError::Run(message)),
        // Unreachable today: the headless token is never cancelled.
        LoopOutcome::Cancelled => Err(HeadlessError::Run("cancelled".into())),
        LoopOutcome::Refused { reason } => Err(HeadlessError::Run(reason)),
    }
}

/// Always drains `rx` (the loop awaits each send), forwarding what maps to an
/// FSM event. Forwarding is `try_send`, as for the FSM's pump: a full or
/// closed sink never stalls the run.
async fn pump_rows(
    mut rx: tokio::sync::mpsc::Receiver<(String, Value)>,
    destination: EventDestination,
) {
    let sink = match destination {
        EventDestination::Drain => None,
        EventDestination::Forward(s) | EventDestination::ForwardWithStates(s) => Some(s),
    };
    while let Some((event_type, payload)) = rx.recv().await {
        let Some(sink) = &sink else { continue };
        let Some(ev) = row_to_event(&event_type, &payload) else {
            continue;
        };
        if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = sink.try_send(ev) {
            tracing::warn!("agentic event sink full; dropping event");
        }
    }
}

#[cfg(test)]
#[path = "react_run_tests.rs"]
mod tests;
