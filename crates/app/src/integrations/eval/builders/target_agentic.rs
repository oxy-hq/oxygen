//! Eval target on the agentic path — the "run the target agent" step of the
//! eval harness, with no old-executor scaffolding.
//!
//! Uses **none** of the old-executor's pipeline machinery — no `Executable`,
//! `execute_with_handler`, `OutputContainer` / `OutputGetter`, or
//! `EventHandler` / `writer`. See `internal-docs/old-executor-retirement.md`.
//!
//! Why the target step reduces to answer text + duration + token usage:
//!   1. The agent runs via `agentic_pipeline::run_agentic_streaming` directly.
//!   2. For a `.test.yml` agentic target `eval.rs` sets `task_ref: None`, and a
//!      single text answer carries no `relevant_contexts` / `references`.
//!   3. The correctness judge (`solver::run_solver` via `render_correctness_prompt`)
//!      reads only `TargetOutput.output` (+ `task_description` on the *expected*
//!      side) — it never touches `relevant_contexts` / `references`.
//!
//! `TargetOutput` is still imported from `oxy::exec_types`; relocating that
//! data type out of the old executor is a later (Tier-C) step.

use std::sync::Arc;
use std::time::Instant;

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::exec_types::TargetOutput;
use oxy_shared::errors::OxyError;
use tokio::sync::mpsc;

use super::run_timing::TimingAccumulator;
use super::types::AgenticInput;

/// Run one eval target agent and produce its [`TargetOutput`], on the agentic
/// path with zero old-executor scaffolding. Returns a single-element `Vec`
/// (one target run → one output) that `run_generator` pairs with its expected
/// answer.
pub(super) async fn run_target(
    workspace: &WorkspaceManager<oxy::config::WorkingCopy>,
    input: AgenticInput,
) -> Result<Vec<TargetOutput>, OxyError> {
    let start = Instant::now();

    // Checked here only so a target outside the workspace fails with this
    // message. The run itself takes the workspace-relative ref and reads it
    // through `ConfigManager::agent_by_ref`, like every other agent run (the
    // `.test.yml` itself stays a working-copy read: evals are CLI-only).
    workspace
        .config_manager
        .resolve_file(&input.config_path)
        .await
        .map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to resolve agentic config path '{}': {e}",
                input.config_path
            ))
        })?;

    let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
        workspace.clone(),
    ));
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx;

    // Capture token usage straight off the agentic event stream — no
    // `EventKind::Usage` round-trip, no `UsageAccumulatorHandler`, no writer.
    // Each event is stamped with its receipt instant for the timing breakdown.
    let (event_tx, event_rx) = mpsc::channel::<AgenticEvent<AnalyticsEvent>>(256);
    let run_start = Instant::now();
    let usage_consumer = spawn_event_consumer(event_rx, run_start);

    let answer_text = agentic_pipeline::run_agentic_streaming_with_states(
        platform,
        &input.config_path,
        input.prompt,
        event_tx,
    )
    .await
    .map_err(OxyError::RuntimeError)?;
    let run_end = Instant::now();

    let (usage, timing) = usage_consumer
        .await
        .map_err(|e| OxyError::RuntimeError(format!("usage consumer join: {e}")))?;

    let mut out = build_target_output(
        answer_text,
        start.elapsed().as_secs_f64() * 1000.0,
        usage.input_tokens,
        usage.output_tokens,
    );
    out.final_sql = usage.final_sql;
    out.llm_rounds = usage.llm_rounds;
    out.runtime = usage.runtime;
    out.model = usage.model;
    out.timing = Some(timing.finish(run_end));
    Ok(vec![out])
}

/// Drain the run's event stream, stamping each event with its receipt instant.
fn spawn_event_consumer(
    mut event_rx: mpsc::Receiver<AgenticEvent<AnalyticsEvent>>,
    run_start: Instant,
) -> tokio::task::JoinHandle<(RunUsage, TimingAccumulator)> {
    tokio::spawn(async move {
        let mut usage = RunUsage::default();
        let mut timing = TimingAccumulator::new(run_start);
        while let Some(ev) = event_rx.recv().await {
            timing.observe(Instant::now(), &ev);
            usage.observe(&ev);
        }
        (usage, timing)
    })
}

/// What the target step reads off the event stream besides timing.
struct RunUsage {
    input_tokens: i32,
    output_tokens: i32,
    final_sql: Option<String>,
    llm_rounds: u32,
    /// `react` once a `RuntimeSelected` event is seen; every other run is the FSM.
    runtime: Option<String>,
    /// The model of the first `LlmEnd` that names one.
    model: Option<String>,
    /// An `AnswerResult` fixed `final_sql`; later queries no longer replace it.
    sql_pinned: bool,
}

impl Default for RunUsage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            final_sql: None,
            llm_rounds: 0,
            runtime: Some("fsm".to_string()),
            model: None,
            sql_pinned: false,
        }
    }
}

impl RunUsage {
    fn observe(&mut self, ev: &AgenticEvent<AnalyticsEvent>) {
        let react = self.runtime.as_deref() == Some("react");
        let (di, dobj) = usage_delta(ev, react);
        if let AgenticEvent::Core(CoreEvent::LlmEnd { model, .. }) = ev
            && self.model.is_none()
            && !model.is_empty()
        {
            self.model = Some(model.clone());
        }
        self.input_tokens = self.input_tokens.saturating_add(di);
        self.output_tokens = self.output_tokens.saturating_add(dobj);
        self.observe_runtime_and_answer(ev);
        if !self.sql_pinned {
            last_successful_sql(ev, &mut self.final_sql);
        }
        self.llm_rounds += count_llm_round(ev);
    }

    /// `RuntimeSelected` names the runtime; `AnswerResult` (which follows the core
    /// `Done`) is the react run's own answer SQL and pins `final_sql`.
    fn observe_runtime_and_answer(&mut self, ev: &AgenticEvent<AnalyticsEvent>) {
        match ev {
            AgenticEvent::Domain(AnalyticsEvent::RuntimeSelected { runtime }) => {
                self.runtime = Some(runtime.clone());
            }
            AgenticEvent::Domain(AnalyticsEvent::AnswerResult { sql, .. }) => {
                self.final_sql = Some(sql.clone());
                self.sql_pinned = true;
            }
            _ => {}
        }
    }
}

/// Per-event token delta `(input, output)` — the same match arms target.rs uses,
/// extracted so it is unit-testable without a live pipeline.
///
/// The FSM reports input on `LlmStart.prompt_tokens` (its `LlmEnd.input_tokens` is 0).
/// A react run reports the provider's uncached input on `LlmEnd.input_tokens`; its
/// `LlmStart` size is a characters/4 estimate and is not counted.
fn usage_delta(ev: &AgenticEvent<AnalyticsEvent>, react: bool) -> (i32, i32) {
    if let AgenticEvent::Core(core) = ev {
        match core {
            CoreEvent::LlmStart { prompt_tokens, .. } if !react => (*prompt_tokens as i32, 0),
            CoreEvent::LlmEnd {
                input_tokens,
                output_tokens,
                ..
            } => (*input_tokens as i32, *output_tokens as i32),
            _ => (0, 0),
        }
    } else {
        (0, 0)
    }
}

/// Remember the SQL of the latest successful `QueryExecuted`; failures leave the slot alone.
fn last_successful_sql(ev: &AgenticEvent<AnalyticsEvent>, slot: &mut Option<String>) {
    if let AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
        query,
        success: true,
        ..
    }) = ev
    {
        *slot = Some(query.clone());
    }
}

/// 1 for an `LlmStart` event, else 0.
fn count_llm_round(ev: &AgenticEvent<AnalyticsEvent>) -> u32 {
    matches!(ev, AgenticEvent::Core(CoreEvent::LlmStart { .. })) as u32
}

/// Build the `TargetOutput` the correctness judge consumes. For the agentic
/// `.test.yml` path this is exactly what the old `OutputContainer::Single` arm
/// produced: the answer text, no task_description, and empty context/refs.
fn build_target_output(
    answer_text: String,
    duration_ms: f64,
    input_tokens: i32,
    output_tokens: i32,
) -> TargetOutput {
    TargetOutput {
        output: answer_text,
        task_description: None,
        relevant_contexts: vec![],
        references: vec![],
        duration_ms,
        input_tokens,
        output_tokens,
        final_sql: None,
        llm_rounds: 0,
        timing: None,
        runtime: None,
        model: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The judge (`CorrectnessSolverMapper`) reads `submission.output` as the
    /// actual answer and never reads `relevant_contexts` / `references`. Assert
    /// the spike's builder produces exactly that shape — the same one the old
    /// `OutputContainer::Single` → `TargetOutput` conversion yields.
    #[test]
    fn build_target_output_matches_single_container_shape() {
        let out = build_target_output("42 orders".to_string(), 12.5, 100, 20);
        assert_eq!(out.output, "42 orders");
        assert_eq!(out.task_description, None);
        assert!(out.relevant_contexts.is_empty());
        assert!(out.references.is_empty());
        assert_eq!(out.input_tokens, 100);
        assert_eq!(out.output_tokens, 20);
        assert_eq!(out.duration_ms, 12.5);
    }

    fn executed(query: &str, success: bool) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
            query: query.to_string(),
            row_count: 0,
            duration_ms: 0,
            success,
            error: None,
            columns: vec![],
            rows: vec![],
            source: agentic_analytics::QuerySource::Llm,
            is_preagg: false,
            sub_spec_index: None,
            semantic_query: None,
        })
    }

    #[test]
    fn captures_last_successful_query_only() {
        let mut slot = None;
        for (q, ok) in [("A", true), ("B", false), ("C", true)] {
            last_successful_sql(&executed(q, ok), &mut slot);
        }
        assert_eq!(slot.as_deref(), Some("C"));

        let mut slot = None;
        last_successful_sql(&executed("X", false), &mut slot);
        assert_eq!(slot, None);

        let start = || {
            AgenticEvent::Core(CoreEvent::LlmStart {
                state: "s".into(),
                prompt_tokens: 1,
                sub_spec_index: None,
            })
        };
        let rounds: u32 = [start(), start(), start(), executed("A", true)]
            .iter()
            .map(count_llm_round)
            .sum();
        assert_eq!(rounds, 3);
    }

    fn selected() -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Domain(AnalyticsEvent::RuntimeSelected {
            runtime: "react".into(),
        })
    }

    fn answer(sql: &str) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Domain(AnalyticsEvent::AnswerResult {
            result_id: "r2".into(),
            sql: sql.into(),
        })
    }

    fn done() -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Core(CoreEvent::Done {
            trace_id: "t".into(),
        })
    }

    fn observe_all(events: &[AgenticEvent<AnalyticsEvent>]) -> RunUsage {
        let mut usage = RunUsage::default();
        events.iter().for_each(|e| usage.observe(e));
        usage
    }

    #[test]
    fn answer_result_pins_final_sql() {
        let usage = observe_all(&[
            selected(),
            executed("a", true),
            answer("b"),
            executed("c", true),
        ]);
        assert_eq!(usage.final_sql.as_deref(), Some("b"));
        assert_eq!(usage.runtime.as_deref(), Some("react"));
    }

    /// `AnswerResult` follows the core `Done`; the consumer must keep reading.
    #[tokio::test]
    async fn answer_result_after_done_still_pins_final_sql() {
        let (tx, rx) = mpsc::channel(8);
        let consumer = spawn_event_consumer(rx, Instant::now());
        for ev in [selected(), executed("a", true), done(), answer("b")] {
            tx.send(ev).await.unwrap();
        }
        drop(tx);
        let (usage, _) = consumer.await.unwrap();
        assert_eq!(usage.final_sql.as_deref(), Some("b"));
        assert_eq!(usage.runtime.as_deref(), Some("react"));
    }

    #[test]
    fn fsm_run_keeps_last_successful_sql() {
        let usage = observe_all(&[
            executed("a", true),
            executed("b", false),
            executed("c", true),
        ]);
        assert_eq!(usage.final_sql.as_deref(), Some("c"));
        assert_eq!(usage.runtime.as_deref(), Some("fsm"));
    }

    #[test]
    fn react_turn_records_provider_usage_not_the_estimate() {
        let usage = observe_all(&[
            selected(),
            AgenticEvent::Core(CoreEvent::LlmStart {
                state: "react".into(),
                prompt_tokens: 99_999,
                sub_spec_index: None,
            }),
            AgenticEvent::Core(CoreEvent::LlmEnd {
                state: "react".into(),
                output_tokens: 22,
                input_tokens: 111,
                cache_creation_input_tokens: 44,
                cache_read_input_tokens: 3333,
                duration_ms: 5,
                model: "m-1".into(),
                sub_spec_index: None,
            }),
        ]);
        assert_eq!((usage.input_tokens, usage.output_tokens), (111, 22));
        assert_eq!(usage.model.as_deref(), Some("m-1"));
        assert_eq!(usage.llm_rounds, 1);
    }

    #[test]
    fn fsm_input_still_comes_from_llm_start() {
        let usage = observe_all(&[
            AgenticEvent::Core(CoreEvent::LlmStart {
                state: "s".into(),
                prompt_tokens: 500,
                sub_spec_index: None,
            }),
            AgenticEvent::Core(CoreEvent::LlmEnd {
                state: "s".into(),
                output_tokens: 7,
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                duration_ms: 5,
                model: "m-0".into(),
                sub_spec_index: None,
            }),
        ]);
        assert_eq!((usage.input_tokens, usage.output_tokens), (500, 7));
        assert_eq!(usage.model.as_deref(), Some("m-0"));
    }
}
