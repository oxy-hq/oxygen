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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::exec_types::{RunObserved, TargetOutput};
use oxy_shared::errors::OxyError;
use tokio::sync::mpsc;

use super::EvalRun;
use super::observed::{Observer, clarifying_question};
use super::reflect_phase::{self, ReflectPhase};
use super::run_timing::{TimingAccumulator, source_name};
use super::types::AgenticInput;

/// Run one eval target agent and produce its [`TargetOutput`], on the agentic
/// path with zero old-executor scaffolding. Returns a single-element `Vec`
/// (one target run → one output) that `run_generator` pairs with its expected
/// answer.
pub(super) async fn run_target(
    run: &EvalRun,
    input: AgenticInput,
) -> Result<Vec<TargetOutput>, OxyError> {
    let start = Instant::now();
    if run.reads_working_copy() {
        check_target_in_working_copy(&run.workspace, &input.config_path).await?;
    }
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = run.project_ctx();

    // Capture token usage straight off the agentic event stream — no
    // `EventKind::Usage` round-trip, no `UsageAccumulatorHandler`, no writer.
    // Each event is stamped with its receipt instant for the timing breakdown.
    let (event_tx, event_rx) = mpsc::channel::<AgenticEvent<AnalyticsEvent>>(256);
    let run_start = Instant::now();
    let usage_consumer = spawn_event_consumer(event_rx, run_start);

    let result = agentic_pipeline::run_agentic_streaming_with_states(
        platform,
        &input.config_path,
        input.prompt,
        event_tx,
        &target_memory(run.reads_working_copy(), |k| std::env::var_os(k)),
    )
    .await;
    let (answer_text, asked) = answer_or_question(result)?;
    let run_end = Instant::now();

    let (usage, timing, reflect, observer) = usage_consumer
        .await
        .map_err(|e| OxyError::RuntimeError(format!("usage consumer join: {e}")))?;
    // A memory reflection is not the answer's: its time comes off the run.
    let answer_end = reflect.as_ref().map_or(run_end, |r| r.start);
    let reflect = reflect.map(|r| r.finish(run_end));

    let mut out = build_target_output(
        answer_text,
        answer_end.duration_since(start).as_secs_f64() * 1000.0,
        usage.input_tokens,
        usage.output_tokens,
    );
    out.final_sql = usage.final_sql;
    out.llm_rounds = usage.llm_rounds;
    out.runtime = usage.runtime;
    out.model = usage.model;
    out.observed = Some(RunObserved {
        asked_user: asked,
        ..observer.finish(usage.answer_source.clone())
    });
    let mut timing = timing.finish(answer_end);
    timing.answer_source = usage.answer_source;
    timing.reflect = reflect;
    out.timing = Some(timing);
    Ok(vec![out])
}

/// The run's answer text, or the clarifying question it stopped on (as both
/// the text and the question): a suspended run is graded, not an error. Any
/// other failure is the run's error.
fn answer_or_question(
    result: Result<String, String>,
) -> Result<(String, Option<String>), OxyError> {
    match result {
        Ok(text) => Ok((text, None)),
        Err(e) => match clarifying_question(&e) {
            Some(question) => Ok((question.to_string(), Some(question.to_string()))),
            None => Err(OxyError::RuntimeError(e)),
        },
    }
}

/// [`eval_memory`] for `oxy test` (`working_copy`) only: an eval the server
/// runs reads a shared context and never gets memory, whatever its env says,
/// so no org's notes land in a local file.
pub(super) fn target_memory(
    working_copy: bool,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> agentic_pipeline::EvalMemory {
    if working_copy {
        eval_memory(var)
    } else {
        agentic_pipeline::EvalMemory::default()
    }
}

/// Notes file a react target reads (`OXY_EVAL_MEMORY_READ`) and appends to
/// (`OXY_EVAL_MEMORY_WRITE`): the memory eval's switch. A cold pass sets only
/// the write file, a warm pass only the read file. Unset or empty: no memory.
fn eval_memory(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> agentic_pipeline::EvalMemory {
    let path = |k| {
        var(k)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    };
    agentic_pipeline::EvalMemory {
        read: path("OXY_EVAL_MEMORY_READ"),
        write: path("OXY_EVAL_MEMORY_WRITE"),
    }
}

/// `oxy test` only: a target outside the workspace fails with this message.
/// The run itself takes the workspace-relative ref and reads it through
/// `ConfigManager::agent_by_ref`, like every other agent run, so the server
/// (which holds no working copy) skips this.
async fn check_target_in_working_copy(
    workspace: &WorkspaceManager<oxy::config::WorkingCopy>,
    config_path: &str,
) -> Result<(), OxyError> {
    workspace
        .config_manager
        .resolve_file(config_path)
        .await
        .map(|_| ())
        .map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to resolve agentic config path '{config_path}': {e}"
            ))
        })
}

type Consumed = (RunUsage, TimingAccumulator, Option<ReflectPhase>, Observer);

/// Drain the run's event stream, stamping each event with its receipt instant.
/// From a memory reflection's start on, events count to the [`ReflectPhase`],
/// except the answer's `AnswerResult`.
fn spawn_event_consumer(
    mut event_rx: mpsc::Receiver<AgenticEvent<AnalyticsEvent>>,
    run_start: Instant,
) -> tokio::task::JoinHandle<Consumed> {
    tokio::spawn(async move {
        let mut usage = RunUsage::default();
        let mut timing = TimingAccumulator::new(run_start);
        let mut reflect: Option<ReflectPhase> = None;
        let mut observer = Observer::default();
        while let Some(ev) = event_rx.recv().await {
            let now = Instant::now();
            if reflect.is_none() && reflect_phase::starts(&ev) {
                reflect = Some(ReflectPhase::new(now));
            }
            match reflect.as_mut() {
                Some(r) => {
                    r.observe(&ev);
                    usage.observe_runtime_and_answer(&ev);
                }
                None => {
                    timing.observe(now, &ev);
                    usage.observe(&ev);
                    observer.observe(&ev);
                }
            }
        }
        (usage, timing, reflect, observer)
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
    /// The source of the execution whose SQL is `final_sql`.
    answer_source: Option<String>,
    /// Source of each successful execution's SQL, so a react `AnswerResult`
    /// (which carries only the SQL) finds the source of the result it cites.
    sql_sources: HashMap<String, &'static str>,
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
            answer_source: None,
            sql_sources: HashMap::new(),
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
        self.observe_query_source(ev);
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
                self.answer_source = self.sql_sources.get(sql).map(|s| s.to_string());
            }
            _ => {}
        }
    }

    /// A successful execution records its SQL's source; until an `AnswerResult`
    /// pins `final_sql`, the latest one is the answer's source (as for `final_sql`).
    fn observe_query_source(&mut self, ev: &AgenticEvent<AnalyticsEvent>) {
        if let AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
            query,
            success: true,
            source,
            ..
        }) = ev
        {
            let name = source_name(*source);
            self.sql_sources.insert(query.clone(), name);
            if !self.sql_pinned {
                self.answer_source = Some(name.to_string());
            }
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
        observed: None,
        expect: None,
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

    fn executed_from(
        query: &str,
        success: bool,
        source: agentic_analytics::QuerySource,
    ) -> AgenticEvent<AnalyticsEvent> {
        let mut ev = executed(query, success);
        if let AgenticEvent::Domain(AnalyticsEvent::QueryExecuted { source: s, .. }) = &mut ev {
            *s = source;
        }
        ev
    }

    /// The answer's source is the source of the result it cites, not of the
    /// last execution: here the semantic `a` is cited after a raw `b` ran.
    #[test]
    fn answer_source_follows_the_cited_result() {
        use agentic_analytics::QuerySource::{Llm, Semantic};
        let usage = observe_all(&[
            selected(),
            executed_from("a", true, Semantic),
            executed_from("b", true, Llm),
            answer("a"),
            executed_from("c", true, Llm),
        ]);
        assert_eq!(usage.answer_source.as_deref(), Some("semantic"));

        let usage = observe_all(&[selected(), executed_from("a", true, Semantic), answer("z")]);
        assert_eq!(
            usage.answer_source, None,
            "an uncited SQL has no known source"
        );
    }

    /// Without an `AnswerResult` (the FSM), the last successful execution decides;
    /// a failed one does not.
    #[test]
    fn answer_source_without_answer_result_is_last_success() {
        use agentic_analytics::QuerySource::{Llm, Semantic};
        let usage = observe_all(&[
            executed_from("a", true, Llm),
            executed_from("b", true, Semantic),
            executed_from("c", false, Llm),
        ]);
        assert_eq!(usage.final_sql.as_deref(), Some("b"));
        assert_eq!(usage.answer_source.as_deref(), Some("semantic"));
        assert_eq!(RunUsage::default().answer_source, None);
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
        let (usage, _, _, _) = consumer.await.unwrap();
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
                advisor_calls: 0,
                advisor_input_tokens: 0,
                advisor_output_tokens: 0,
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
                advisor_calls: 0,
                advisor_input_tokens: 0,
                advisor_output_tokens: 0,
            }),
        ]);
        assert_eq!((usage.input_tokens, usage.output_tokens), (500, 7));
        assert_eq!(usage.model.as_deref(), Some("m-0"));
    }

    #[test]
    fn a_suspended_run_answers_with_its_question() {
        let asked = Err(format!(
            "{}Which region?",
            agentic_pipeline::CLARIFYING_QUESTION_PREFIX
        ));
        let (text, question) = answer_or_question(asked).unwrap();
        assert_eq!(text, "Which region?");
        assert_eq!(question.as_deref(), Some("Which region?"));
        let (text, question) = answer_or_question(Ok("42".into())).unwrap();
        assert_eq!((text.as_str(), question), ("42", None));
        let err = answer_or_question(Err("max iterations exceeded".into())).unwrap_err();
        assert_eq!(err.to_string(), "max iterations exceeded");
    }

    /// The observer sees the answer's events and not the reflection's.
    #[tokio::test]
    async fn the_observer_reads_the_answer_events_only() {
        let (tx, rx) = mpsc::channel(32);
        let consumer = spawn_event_consumer(rx, Instant::now());
        let call = |name: &str| {
            AgenticEvent::Core(CoreEvent::ToolCall {
                name: name.into(),
                input: "{}".into(),
                llm_duration_ms: 0,
                sub_spec_index: None,
            })
        };
        let enter = AgenticEvent::Core(CoreEvent::StateEnter {
            state: agentic_runtime::react::REFLECT_STATE.into(),
            revision: 0,
            trace_id: "t".into(),
            sub_spec_index: None,
        });
        for ev in [selected(), call("query"), enter, call("remember")] {
            tx.send(ev).await.unwrap();
        }
        drop(tx);
        let (_, _, _, observer) = consumer.await.unwrap();
        assert_eq!(observer.finish(None).tools, vec!["query"]);
    }

    #[test]
    fn eval_memory_reads_its_two_variables_and_treats_empty_as_unset() {
        use std::ffi::OsString;
        let off = eval_memory(|_| None);
        assert!(off.is_off());
        let cold = eval_memory(|k| match k {
            "OXY_EVAL_MEMORY_WRITE" => Some(OsString::from("/tmp/n.jsonl")),
            "OXY_EVAL_MEMORY_READ" => Some(OsString::new()),
            _ => None,
        });
        assert_eq!(cold.read, None);
        assert_eq!(
            cold.write.as_deref(),
            Some(std::path::Path::new("/tmp/n.jsonl"))
        );
    }

    fn llm_end(state: &str, input: usize, output: usize) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Core(CoreEvent::LlmEnd {
            state: state.into(),
            output_tokens: output,
            input_tokens: input,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            duration_ms: 1,
            model: "m".into(),
            sub_spec_index: None,
            advisor_calls: 0,
            advisor_input_tokens: 0,
            advisor_output_tokens: 0,
        })
    }

    fn llm_start() -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Core(CoreEvent::LlmStart {
            state: "react".into(),
            prompt_tokens: 0,
            sub_spec_index: None,
        })
    }

    #[tokio::test]
    async fn the_reflection_turn_is_counted_apart_and_the_answer_sql_still_pins() {
        let (tx, rx) = mpsc::channel(32);
        let consumer = spawn_event_consumer(rx, Instant::now());
        let enter = AgenticEvent::Core(CoreEvent::StateEnter {
            state: agentic_runtime::react::REFLECT_STATE.into(),
            revision: 0,
            trace_id: "t".into(),
            sub_spec_index: None,
        });
        for ev in [
            selected(),
            llm_start(),
            llm_end("react", 10, 5),
            enter,
            llm_start(),
            llm_end(agentic_runtime::react::REFLECT_STATE, 100, 50),
            answer("select 5"),
        ] {
            tx.send(ev).await.unwrap();
        }
        drop(tx);
        let (usage, timing, reflect, _) = consumer.await.unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (10, 5));
        assert_eq!(usage.llm_rounds, 1);
        assert_eq!(usage.final_sql.as_deref(), Some("select 5"));
        let r = reflect.unwrap().finish(Instant::now());
        assert_eq!((r.llm_calls, r.input_tokens, r.output_tokens), (1, 100, 50));
        assert_eq!(timing.finish(Instant::now()).llm_calls, 1);
    }
}
