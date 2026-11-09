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

/// What to do with each `Event<AnalyticsEvent>` the pipeline emits during a
/// headless run. `Drain` discards them (eval / fire-and-forget); `Forward`
/// forwards them to a caller-owned sink without ever blocking the solver.
enum EventDestination {
    Drain,
    Forward(tokio::sync::mpsc::Sender<Event<AnalyticsEvent>>),
}

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
    run_agentic_headless(platform, agent_ref, prompt, EventDestination::Drain).await
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
    )
    .await
    .map_err(|e| e.to_string())
}

/// Shared body of [`run_agentic_eval`] and [`run_agentic_streaming`].
/// Loads the agent, resolves connectors, builds the solver, spawns the event
/// pump matching `destination`, then runs the orchestrator.
async fn run_agentic_headless(
    platform: Arc<dyn PlatformContext>,
    agent_ref: &str,
    prompt: String,
    destination: EventDestination,
) -> Result<String, HeadlessError> {
    use agentic_analytics::build_analytics_handlers;
    use agentic_core::orchestrator::Orchestrator;

    let ctx_root = platform.context_root().await;
    // See `start_analytics`: keep `ctx_root` alive across context resolution.
    let base_dir = ctx_root.path().to_path_buf();

    let (config, domain_docs) = load_agent(&*platform, agent_ref, &base_dir).await?;
    let ctx = build_context(&*platform, &config, domain_docs, &base_dir).await;

    let (solver, automation_files) = config
        .build_solver_with_context(&base_dir, ctx)
        .await
        .map_err(|e| HeadlessError::Run(format!("failed to build agentic solver: {e}")))?;

    let solver = solver.with_events(spawn_event_pump(destination));
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
async fn build_context(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    domain_docs: Option<Vec<String>>,
    base_dir: &Path,
) -> BuildContext {
    let mut ctx = BuildContext {
        domain_docs,
        ..BuildContext::default()
    };
    ctx.project_model_info = platform
        .resolve_model(config.llm.model_ref.as_deref(), config.llm.model.is_some())
        .await;
    ctx.timezone = platform.timezone();

    let mut effective_databases: Vec<String> = config.databases.clone();
    if let Ok(resolved) = config.resolve_context(base_dir) {
        for db_name in resolved.referenced_databases {
            if !effective_databases.contains(&db_name) {
                effective_databases.push(db_name);
            }
        }
    }
    let resolved = platform::resolve_connectors(&effective_databases, platform).await;
    let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
    connectors.extend(resolved.pre_built);
    ctx.extra_default_connector = effective_databases
        .iter()
        .find(|name| connectors.contains_key(*name))
        .cloned();
    ctx.extra_connectors = connectors;
    ctx
}

/// The solver's event stream, pumped into `destination` on its own task.
fn spawn_event_pump(destination: EventDestination) -> EventStream<AnalyticsEvent> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Event<AnalyticsEvent>>(256);
    spawn_with_hub(async move {
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
            format!(
                "agentic pipeline asked a clarifying question: {}",
                prompts.join("; ")
            )
        }
        OrchestratorError::MaxIterationsExceeded => "max iterations exceeded".into(),
        OrchestratorError::ResumeNotSupported => "resume not supported".into(),
        OrchestratorError::Fatal(e) => format!("fatal: {e:?}"),
    }
}

#[cfg(test)]
#[path = "headless_tests.rs"]
mod tests;
