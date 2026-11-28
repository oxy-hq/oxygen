//! Headless entry points: run an analytics agent outside an HTTP request and
//! hand back the answer text — the eval runner, Slack, the Workspace Health
//! agent probe, and the document librarian.
//!
//! No run row, no SSE stream: the caller owns what happens to the answer.
//!
//! Every entry point takes an agent **ref**, never a path on this node's disk,
//! and reads it through [`agent_definition::read_analytics_agent`], the reader
//! an Ask run uses. On a host with a compile boundary that is the revision's
//! row and nothing else, so a headless run works on a pod with no files.

use std::path::Path;
use std::sync::Arc;

use agentic_analytics::config::{AgentConfig, BuildContext};
use agentic_analytics::{AnalyticsDomain, AnalyticsEvent, AnalyticsIntent, QuestionType};
use agentic_core::events::{Event, EventStream};
use agentic_core::hub_task::spawn_with_hub;
use agentic_core::orchestrator::OrchestratorError;

use crate::agent_definition::{self, AgentDefinitionError};
use crate::context_documents;
use crate::platform::{self, PlatformContext};
pub use crate::react_run::memory::EvalMemory;
use crate::react_run::{
    self, EVAL_ENTRY_REACT, HARNESS_ENTRY_REACT, ReactPolicy, STREAMING_ENTRY_REACT,
};
pub use memory_merge::merge_eval_notes;

/// What to do with each `Event<AnalyticsEvent>` the pipeline emits during a
/// headless run. `Drain` discards them (eval / fire-and-forget); `Forward`
/// forwards them to a caller-owned sink without ever blocking the solver.
/// `ForwardWithStates` forwards the same way and also wires the orchestrator's
/// own events (`StateEnter` / `StateExit` / `FanOut` / `SubSpec*`), which
/// `Forward` never carries on an FSM run — the eval harness's timing breakdown
/// needs them. On a react run, which only the harness entry starts (see
/// [`HARNESS_ENTRY_REACT`]), every row that maps to an event is forwarded,
/// core `done` / `error` included.
pub(crate) enum EventDestination {
    Drain,
    Forward(tokio::sync::mpsc::Sender<Event<AnalyticsEvent>>),
    ForwardWithStates(tokio::sync::mpsc::Sender<Event<AnalyticsEvent>>),
}

/// How a headless run's error text starts when the run stopped on a
/// clarifying question (either runtime); the question follows it. The eval
/// harness reads it to grade a run that asked rather than answered.
pub const CLARIFYING_QUESTION_PREFIX: &str = "agentic pipeline asked a clarifying question: ";

/// Why a headless run produced no answer, split where its callers need it: an
/// agent that could not be read is a run that never started; anything after
/// that is the agent failing.
#[derive(Debug)]
pub enum HeadlessError {
    /// The agent's definition or its context documents could not be read —
    /// not compiled into the revision yet, no such agent, a ref outside the
    /// workspace. The run never had an agent to start.
    Load(String),
    /// A definition was found and does not parse. The agent is broken, the
    /// same as one that loads and then fails.
    Invalid(String),
    /// The definition loaded and the run failed: no model or key, no
    /// connector, a clarifying question, an LLM or query error.
    Run(String),
}

impl std::fmt::Display for HeadlessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(msg) | Self::Invalid(msg) | Self::Run(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for HeadlessError {}

/// Run an agentic analytics agent headlessly and return its answer text.
///
/// `agent_ref` is a workspace ref (`_analytics.agentic.yml`,
/// `agents/foo.agentic.yml`, or a bare stem), resolved like an Ask run's agent:
/// the host's compile boundary answers, and only a host with none (the CLI,
/// tests) reads its working copy. The Workspace Health agent probe calls this
/// from fleet pods that have no files to read.
pub async fn run_agentic_eval(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
) -> Result<String, HeadlessError> {
    let destination = EventDestination::Drain;
    run_agentic_headless(
        platform,
        agent_ref,
        prompt,
        destination,
        (EVAL_ENTRY_REACT, None),
    )
    .await
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
/// The agent is named by a workspace ref and read the way every other run
/// reads one ([`agent_definition::read_analytics_agent`]): from the compiled
/// revision on a host that has one, so a caller of this is not tied to a pod
/// holding a working copy.
pub async fn run_agentic_answer(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
) -> Result<String, String> {
    /// A ceiling for an agent that names none. An answer over four documents
    /// is a few sentences; a run that produces more than this has misunderstood
    /// the job, and paying for it silently is how a per-question cost becomes a
    /// per-month surprise.
    const DEFAULT_MAX_TOKENS: u32 = 2048;

    let config = {
        let ctx_root = platform.context_root().await;
        agent_definition::read_analytics_agent(&*platform, agent_ref, ctx_root.path())
            .await
            .map_err(|e| format!("failed to load agentic config {agent_ref}: {e}"))?
    };

    let info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await
        .ok_or_else(|| {
            format!(
                "no model configured for agent {agent_ref} — its `llm:` names {:?}, \
                 which does not resolve in this project's config.yml",
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
        .map_err(|e| format!("agent {agent_ref} failed to answer: {e}"))
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
///
/// `agent_ref` is read like [`run_agentic_eval`]'s: Slack passes the path its
/// picker or a saved preference holds, and on a fleet pod that resolves
/// against the compiled revision, never a file.
pub async fn run_agentic_streaming(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
    event_sink: tokio::sync::mpsc::Sender<Event<AnalyticsEvent>>,
) -> Result<String, String> {
    run_agentic_headless(
        platform,
        agent_ref,
        prompt,
        EventDestination::Forward(event_sink),
        (STREAMING_ENTRY_REACT, None),
    )
    .await
    .map_err(|e| e.to_string())
}

/// [`run_agentic_streaming`] that also forwards the orchestrator's state and
/// fan-out events (`StateEnter`, `StateExit`, `FanOut`, `SubSpecStart/End`,
/// `Done`, ...). Opt-in, so existing streaming surfaces (Slack, documents, the
/// health probe) keep receiving exactly what they did. Used by the eval
/// harness to time each pipeline state.
/// It is the one headless entry that honors `runtime: react`
/// ([`HARNESS_ENTRY_REACT`]); the others run such an agent on the FSM.
/// `memory` is the eval's notes ([`EvalMemory`]): only a react run reads or
/// writes them, and an FSM run ignores them.
pub async fn run_agentic_streaming_with_states(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
    event_sink: tokio::sync::mpsc::Sender<Event<AnalyticsEvent>>,
    memory: &EvalMemory,
) -> Result<String, String> {
    run_agentic_headless(
        platform,
        agent_ref,
        prompt,
        EventDestination::ForwardWithStates(event_sink),
        (HARNESS_ENTRY_REACT, Some(memory)),
    )
    .await
    .map_err(|e| e.to_string())
}

/// Shared body of [`run_agentic_eval`] and the streaming variants.
/// Loads the agent, resolves connectors, builds the solver, spawns the event
/// pump matching `destination`, then runs the orchestrator.
async fn run_agentic_headless(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
    destination: EventDestination,
    (react, memory): (ReactPolicy, Option<&EvalMemory>),
) -> Result<String, HeadlessError> {
    use agentic_analytics::build_analytics_handlers;
    use agentic_core::orchestrator::Orchestrator;

    let ctx_root = platform.context_root().await;
    // See `start_analytics`: keep `ctx_root` alive across context resolution.
    let base_dir = ctx_root.path().to_path_buf();

    let (config, domain_docs) = load_agent(&*platform, agent_ref, &base_dir).await?;
    let runs_react = react_run::picks_react(&config, react, agent_ref);
    let ctx = build_context(&*platform, &config, domain_docs, &base_dir, runs_react).await;
    if runs_react {
        let memory = memory.cloned().unwrap_or_default();
        let run = (agent_ref, base_dir.as_path(), &memory);
        return react_run::run_headless(&*platform, &config, ctx, run, prompt, destination).await;
    }

    let (solver, automation_files) = config
        .build_solver_with_context(&base_dir, ctx)
        .await
        .map_err(|e| HeadlessError::Run(format!("failed to build agentic solver: {e}")))?;

    let wire_orchestrator = matches!(destination, EventDestination::ForwardWithStates(_));
    let events = spawn_event_pump(destination);
    let solver = FsmTools::of(&*platform).equip(solver.with_events(events.clone()));
    // Same rule as the two PipelineBuilder sites: only inject a
    // automation runner when the agent's `context:` actually resolved
    // some files. An empty list would fall through to
    // `workspace.list_automation_files()` (full project scan), giving
    // the FSM access to every automation in the project — which has
    // caused a runaway subrun chain when a misparsed agent had no
    // concrete instructions.
    // Never on a workspace-preview platform — see the gate.
    let solver = match platform::preview::automation_subrun_runner(&platform, automation_files) {
        Some(runner) => solver.with_subrun_runner(runner),
        None => solver,
    };

    let mut orchestrator = Orchestrator::new(solver).with_handlers(build_analytics_handlers());
    if wire_orchestrator {
        orchestrator = orchestrator.with_events(events);
    }
    orchestrator
        .run(headless_intent(prompt))
        .await
        .map(|answer| answer.text)
        .map_err(|e| HeadlessError::Run(orchestrator_error_text(e)))
}

/// The agent's definition and the context documents it names.
///
/// A ref is read by [`agent_definition::read_analytics_agent`], the reader an
/// Ask run's loader uses, so a headless run and a chat run of the same agent
/// read the same definition from the same place. A definition that does not
/// parse is [`HeadlessError::Invalid`]; every other failure here is
/// [`HeadlessError::Load`]. Both are flattened to strings like the rest of this
/// module: its callers hand them to a person (an eval, a health probe, a Slack
/// reply) and have no retry to give "retryable" to.
async fn load_agent(
    platform: &dyn PlatformContext,
    agent_ref: &str,
    base_dir: &Path,
) -> Result<(AgentConfig, Option<Vec<String>>), HeadlessError> {
    let read = agent_definition::read_analytics_agent(platform, agent_ref, base_dir).await;
    let config = read.map_err(|e| match e {
        e @ (AgentDefinitionError::Missing(_)
        | AgentDefinitionError::Unavailable(_)
        | AgentDefinitionError::OutsideWorkspace(_)) => {
            HeadlessError::Load(format!("failed to load agentic config {agent_ref}: {e}"))
        }
        AgentDefinitionError::Invalid(e) => {
            HeadlessError::Invalid(format!("agentic config {agent_ref} is not valid: {e}"))
        }
    })?;
    let docs = context_documents::resolve(platform, &config)
        .await
        .map_err(|e| {
            HeadlessError::Load(format!("failed to read the agent's context documents: {e}"))
        })?;
    Ok((config, docs))
}

/// The solver's build context: model, timezone, and the connectors for the
/// agent's databases plus any its `context:` references.
/// The run's context. A react run gets the platform's read-only connector
/// where it has one, as a served react chat does
/// ([`platform::react_connectors`]); without it, an `airhouse_managed`
/// database has only the role connection, which react refuses.
async fn build_context(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    domain_docs: Option<Vec<String>>,
    base_dir: &Path,
    runs_react: bool,
) -> BuildContext {
    let mut ctx = BuildContext {
        domain_docs,
        ..BuildContext::default()
    };
    ctx.project_model_info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await;
    ctx.timezone = platform.timezone();
    // The platform's rollups, as a served chat forwards them (`start_analytics`),
    // so an eval can be answered from a pre-aggregation.
    ctx.preagg = platform.preagg_context();

    let (effective_databases, connectors) = if runs_react {
        platform::react_connectors(platform, config, base_dir).await
    } else {
        platform::fsm_connectors(platform, config, base_dir).await
    };
    ctx.extra_default_connector = effective_databases
        .iter()
        .find(|name| connectors.contains_key(*name))
        .cloned();
    ctx.extra_connectors = connectors;
    ctx
}

/// The tools a served FSM chat puts on its solver beyond the build context
/// (`agentic_analytics::start_pipeline`), so a headless run of the same agent
/// can call them too: the metric-tree tools (`explain`, `opportunity`,
/// `sensitivity`, `predict`) and the anomaly tools (`list_anomalies`,
/// `detect_anomalies`, `explain_anomaly`), the latter scoped to the platform's
/// workspace. Each is present only when the platform provides it: an eval run
/// gets the runner because it carries the caller's subject and role, and a
/// Workspace Health probe, which carries no subject, does not.
///
/// FSM only. The react runtime has no metric-tree or anomaly tool
/// (`agentic_analytics::react::ReactInputs`), so a react run is given none.
///
/// The served chat's metric sink is not here: it records the measures a run
/// touched for the agent's Metrics tab, keyed by the run's `analytics.run`
/// trace, which a headless run does not open. Forwarded, it would either skip
/// every record or count an eval's or a probe's questions as the agent's use.
struct FsmTools {
    metric_tree_runner: Option<Arc<dyn agentic_analytics::MetricTreeRunner>>,
    anomaly_store: Option<Arc<dyn agentic_analytics::anomaly_store::AnomalyStore>>,
    workspace_id: uuid::Uuid,
}

impl FsmTools {
    fn of(platform: &dyn PlatformContext) -> Self {
        Self {
            metric_tree_runner: platform.metric_tree_runner(),
            anomaly_store: platform.anomaly_store(),
            workspace_id: platform.workspace_id(),
        }
    }

    /// Hands the tools to the solver. A tool the platform does not provide
    /// stays off the solver, so the model is not offered it.
    fn equip(
        self,
        solver: agentic_analytics::AnalyticsSolver,
    ) -> agentic_analytics::AnalyticsSolver {
        let solver = solver.with_workspace_id(self.workspace_id);
        let solver = match self.metric_tree_runner {
            Some(runner) => solver.with_metric_tree_runner(runner),
            None => solver,
        };
        match self.anomaly_store {
            Some(store) => solver.with_anomaly_store(store),
            None => solver,
        }
    }
}

/// The solver's event stream, pumped into `destination` on its own task.
fn spawn_event_pump(destination: EventDestination) -> EventStream<AnalyticsEvent> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Event<AnalyticsEvent>>(256);
    spawn_with_hub(async move {
        match destination {
            EventDestination::Drain => while event_rx.recv().await.is_some() {},
            EventDestination::Forward(sink) | EventDestination::ForwardWithStates(sink) => {
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
    event_tx
}

fn headless_intent(prompt: String) -> AnalyticsIntent {
    AnalyticsIntent {
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
    }
}

fn orchestrator_error_text(e: OrchestratorError<AnalyticsDomain>) -> String {
    match e {
        OrchestratorError::Suspended { reason, .. } => {
            let questions = match reason {
                agentic_core::SuspendReason::HumanInput { questions } => questions,
                _ => vec![],
            };
            let prompts: Vec<_> = questions.iter().map(|q| q.prompt.as_str()).collect();
            format!("{CLARIFYING_QUESTION_PREFIX}{}", prompts.join("; "))
        }
        OrchestratorError::MaxIterationsExceeded => "max iterations exceeded".into(),
        OrchestratorError::ResumeNotSupported => "resume not supported".into(),
        OrchestratorError::Fatal(e) => format!("fatal: {e:?}"),
    }
}

#[cfg(test)]
#[path = "headless_tests.rs"]
mod tests;

#[path = "react_memory_merge.rs"]
pub(crate) mod memory_merge;
