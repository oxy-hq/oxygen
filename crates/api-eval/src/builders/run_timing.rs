//! Folds the agentic event stream of one eval target run into a [`RunTiming`]:
//! where a question's wall time went (BIRD baseline — "the FSM is slow").
//!
//! Pure: the caller stamps each event with its receipt [`Instant`] and hands it
//! to [`TimingAccumulator::observe`], so the arithmetic is testable with no
//! clock and no pipeline. Durations reported by the emitters (`LlmEnd`,
//! `ToolResult`, `QueryExecuted`) are summed as-is; receipt instants are used
//! only for `startup_ms`, TTFT and per-state spans.

use std::collections::HashMap;
use std::time::Instant;

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use oxy::exec_types::RunTiming;

pub(super) struct TimingAccumulator {
    start: Instant,
    first_event: Option<Instant>,
    /// Open LLM rounds by `sub_spec_index`: `LlmStart` receipt, and the TTFT
    /// once the round's first token arrives.
    open_rounds: HashMap<Option<usize>, OpenRound>,
    /// Open states, by `(state, sub_spec_index)`.
    open_states: HashMap<(String, Option<usize>), Instant>,
    t: RunTiming,
}

struct OpenRound {
    started: Instant,
    ttft_ms: Option<u64>,
}

fn ms(from: Instant, to: Instant) -> u64 {
    to.saturating_duration_since(from).as_millis() as u64
}

impl TimingAccumulator {
    pub(super) fn new(start: Instant) -> Self {
        Self {
            start,
            first_event: None,
            open_rounds: HashMap::new(),
            open_states: HashMap::new(),
            t: RunTiming::default(),
        }
    }

    pub(super) fn observe(&mut self, at: Instant, ev: &AgenticEvent<AnalyticsEvent>) {
        self.first_event.get_or_insert(at);
        self.t.fanout |= implies_fanout(ev);
        match ev {
            AgenticEvent::Core(core) => self.observe_core(at, core),
            AgenticEvent::Domain(AnalyticsEvent::QueryExecuted {
                duration_ms,
                success,
                ..
            }) => {
                self.t.query_count += 1;
                self.t.query_ms += duration_ms;
                self.t.query_failures += u64::from(!success);
            }
            _ => {}
        }
    }

    fn observe_core(&mut self, at: Instant, ev: &CoreEvent) {
        match ev {
            CoreEvent::LlmStart { sub_spec_index, .. } => {
                let round = OpenRound {
                    started: at,
                    ttft_ms: None,
                };
                self.open_rounds.insert(*sub_spec_index, round);
            }
            CoreEvent::LlmToken { sub_spec_index, .. }
            | CoreEvent::ThinkingToken { sub_spec_index, .. } => {
                if let Some(round) = self.open_rounds.get_mut(sub_spec_index) {
                    round.ttft_ms.get_or_insert(ms(round.started, at));
                }
            }
            CoreEvent::LlmEnd { .. } => self.observe_llm_end(ev),
            CoreEvent::ToolResult {
                name,
                duration_ms,
                is_error,
                ..
            } => {
                self.t.tool_calls += 1;
                self.t.tool_ms += duration_ms;
                let tool = self.t.tools_by_name.entry(name.clone()).or_default();
                tool.calls += 1;
                tool.ms += duration_ms;
                tool.errors += u64::from(*is_error);
            }
            CoreEvent::StateEnter {
                state,
                sub_spec_index,
                ..
            } => self.enter_state(at, state, *sub_spec_index),
            CoreEvent::StateExit {
                state,
                sub_spec_index,
                ..
            } => self.exit_state(at, state, *sub_spec_index),
            _ => {}
        }
    }

    fn observe_llm_end(&mut self, ev: &CoreEvent) {
        let CoreEvent::LlmEnd {
            state,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            duration_ms,
            sub_spec_index,
            ..
        } = ev
        else {
            return;
        };
        // Only a round that streamed a token has a measured TTFT; the others
        // (structured response, tool calls only) count in neither TTFT nor
        // stream time.
        let measured = self.open_rounds.remove(sub_spec_index);
        if let Some(ttft) = measured.and_then(|r| r.ttft_ms) {
            self.t.llm_ttft_rounds += 1;
            self.t.llm_ttft_ms += ttft;
            self.t.llm_stream_ms += duration_ms.saturating_sub(ttft);
        }
        self.t.llm_calls += 1;
        self.t.llm_ms += duration_ms;
        self.t.output_tokens += *output_tokens as u64;
        self.t.cache_creation_input_tokens += *cache_creation_input_tokens as u64;
        self.t.cache_read_input_tokens += *cache_read_input_tokens as u64;
        let per_state = self.t.llm_by_state.entry(state.clone()).or_default();
        per_state.calls += 1;
        per_state.ms += duration_ms;
    }

    /// A second `StateEnter` on a key that is still open closes the first span
    /// at this instant, so a missed `StateExit` never loses time.
    fn enter_state(&mut self, at: Instant, state: &str, ssi: Option<usize>) {
        self.exit_state(at, state, ssi);
        self.open_states.insert((state.to_string(), ssi), at);
    }

    fn exit_state(&mut self, at: Instant, state: &str, ssi: Option<usize>) {
        if let Some(entered) = self.open_states.remove(&(state.to_string(), ssi)) {
            *self.t.states.entry(state.to_string()).or_default() += ms(entered, at);
        }
    }

    /// Close the run at `end` (the instant the agent run returned). States still
    /// open (error / suspend paths) are closed at `end`.
    pub(super) fn finish(mut self, end: Instant) -> RunTiming {
        let open: Vec<_> = self.open_states.drain().collect();
        for ((state, _), entered) in open {
            *self.t.states.entry(state).or_default() += ms(entered, end);
        }
        let mut t = self.t;
        t.wall_ms = ms(self.start, end);
        t.startup_ms = ms(self.start, self.first_event.unwrap_or(end).min(end));
        t.other_ms = other_ms(&t);
        t
    }
}

/// Fan-out evidence beyond `FanOut` itself: a `SubSpecStart/End`, or any event
/// tagged with a `sub_spec_index`. The headless pump drops events when the
/// sink is full (`try_send`), so one lost `FanOut` must not leave `fanout`
/// false and `other_ms` numeric over concurrent sums.
fn implies_fanout(ev: &AgenticEvent<AnalyticsEvent>) -> bool {
    let ssi = match ev {
        AgenticEvent::Domain(AnalyticsEvent::QueryExecuted { sub_spec_index, .. }) => {
            *sub_spec_index
        }
        AgenticEvent::Core(core) => match core {
            CoreEvent::FanOut { .. }
            | CoreEvent::SubSpecStart { .. }
            | CoreEvent::SubSpecEnd { .. } => return true,
            CoreEvent::StateEnter { sub_spec_index, .. }
            | CoreEvent::StateExit { sub_spec_index, .. }
            | CoreEvent::LlmStart { sub_spec_index, .. }
            | CoreEvent::LlmToken { sub_spec_index, .. }
            | CoreEvent::LlmEnd { sub_spec_index, .. }
            | CoreEvent::ThinkingStart { sub_spec_index, .. }
            | CoreEvent::ThinkingToken { sub_spec_index, .. }
            | CoreEvent::ThinkingEnd { sub_spec_index, .. }
            | CoreEvent::ToolCall { sub_spec_index, .. }
            | CoreEvent::ToolResult { sub_spec_index, .. } => *sub_spec_index,
            _ => None,
        },
        _ => None,
    };
    ssi.is_some()
}

/// Time not attributed to startup, LLM inference, tool execution or warehouse
/// queries: `wall − startup − llm − tool − query`, floored at 0; `None` under
/// fan-out, where concurrent sub-specs make the sums exceed wall time.
///
/// Subtracting all four is sound because, outside fan-out, the measured spans
/// are disjoint (verified at the emitters, branch `feat/bird-eval-harness`):
/// - `LlmEnd.duration_ms` is `round_start.elapsed()` taken right after the
///   provider stream ends (`crates/agentic/llm/src/client/tool_loop.rs:302`),
///   before any tool of that round runs.
/// - `ToolResult.duration_ms` times only `tool_executor(..)` (`tool_loop.rs:578-580`),
///   tools run sequentially, and no analytics tool executor calls an LLM or
///   emits `QueryExecuted` (`crates/agentic/analytics/src/tools/*`). The
///   synthetic `compile_semantic_query` results in
///   `crates/agentic/analytics/src/solver/specifying/mod.rs:763-957` time a
///   local airlayer compile done after the LLM round, outside any tool.
/// - `QueryExecuted` is emitted only by `execute_solution`
///   (`crates/agentic/analytics/src/solver/executing/mod.rs:244-404, 617-652`;
///   fan-out copy `solver/fanout_worker.rs:591-620`), called from the executing
///   state handler (`executing/mod.rs:987`) and `domain_solver.rs:238` — never
///   from inside a tool, so query time is not inside any `ToolResult`.
///
/// `startup_ms` and LLM time are disjoint only because the orchestrator's
/// `StateEnter` arrives before the first `LlmStart`: the eval target uses
/// `run_agentic_streaming_with_states` for that. On a stream without state
/// events the first event is an `LlmStart` whose round began at `round_start`
/// (`tool_loop.rs:103`, emitted at `:153`), so a sliver would be counted twice.
fn other_ms(t: &RunTiming) -> Option<u64> {
    if t.fanout {
        return None;
    }
    Some(
        t.wall_ms
            .saturating_sub(t.startup_ms)
            .saturating_sub(t.llm_ms)
            .saturating_sub(t.tool_ms)
            .saturating_sub(t.query_ms),
    )
}

#[cfg(test)]
#[path = "run_timing_tests.rs"]
mod tests;
