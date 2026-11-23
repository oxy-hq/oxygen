//! The memory reflection turn's share of a react eval run, kept apart from
//! the answer's. The runtime wraps that turn in a `StateEnter` / `StateExit`
//! named `REFLECT_STATE`. Every event from the `StateEnter` on belongs to the
//! reflection, except `AnswerResult` (the done hook, which runs after it).
//! So the answer's tokens, rounds, timing and `duration_ms` end at the
//! reflection's start.

use std::time::Instant;

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use agentic_runtime::react::{MEMORY_TOOLS, REFLECT_STATE};
use oxy::exec_types::{MAX_TOOL_ERROR_CHARS, ReflectTiming};

/// Whether `ev` starts the reflection turn.
pub(super) fn starts(ev: &AgenticEvent<AnalyticsEvent>) -> bool {
    matches!(ev, AgenticEvent::Core(CoreEvent::StateEnter { state, .. }) if state == REFLECT_STATE)
}

pub(super) struct ReflectPhase {
    pub(super) start: Instant,
    t: ReflectTiming,
}

impl ReflectPhase {
    pub(super) fn new(start: Instant) -> Self {
        Self {
            start,
            t: ReflectTiming::default(),
        }
    }

    pub(super) fn observe(&mut self, ev: &AgenticEvent<AnalyticsEvent>) {
        let AgenticEvent::Core(core) = ev else { return };
        match core {
            CoreEvent::LlmStart { .. } => self.t.llm_calls += 1,
            CoreEvent::LlmEnd {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                advisor_calls,
                advisor_input_tokens,
                advisor_output_tokens,
                ..
            } => {
                self.t.advisor_calls += *advisor_calls as u64;
                self.t.advisor_input_tokens += *advisor_input_tokens as u64;
                self.t.advisor_output_tokens += *advisor_output_tokens as u64;
                self.t.input_tokens += *input_tokens as u64;
                self.t.output_tokens += *output_tokens as u64;
                self.t.cache_read_input_tokens += *cache_read_input_tokens as u64;
                self.t.cache_creation_input_tokens += *cache_creation_input_tokens as u64;
            }
            CoreEvent::ToolResult {
                name,
                output,
                is_error,
                ..
            } => {
                self.t.memory_writes += (!is_error && MEMORY_TOOLS.contains(&name.as_str())) as u64;
                if *is_error {
                    let msg: String = output.chars().take(MAX_TOOL_ERROR_CHARS).collect();
                    self.t.errors.push(format!("{name}: {msg}"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn finish(mut self, end: Instant) -> ReflectTiming {
        self.t.ms = end.saturating_duration_since(self.start).as_millis() as u64;
        self.t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(e: CoreEvent) -> AgenticEvent<AnalyticsEvent> {
        AgenticEvent::Core(e)
    }

    #[test]
    fn counts_the_reflection_round_tokens_and_writes() {
        let enter = core(CoreEvent::StateEnter {
            state: REFLECT_STATE.into(),
            revision: 0,
            trace_id: "t".into(),
            sub_spec_index: None,
        });
        assert!(starts(&enter));
        let start = Instant::now();
        let mut p = ReflectPhase::new(start);
        p.observe(&core(CoreEvent::LlmStart {
            state: "react".into(),
            prompt_tokens: 900,
            sub_spec_index: None,
        }));
        p.observe(&core(CoreEvent::LlmEnd {
            state: REFLECT_STATE.into(),
            output_tokens: 40,
            input_tokens: 12,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 800,
            duration_ms: 5,
            model: "m".into(),
            sub_spec_index: None,
            advisor_calls: 1,
            advisor_input_tokens: 70,
            advisor_output_tokens: 9,
        }));
        for (name, is_error) in [("remember", false), ("remember", true), ("query", false)] {
            p.observe(&core(CoreEvent::ToolResult {
                name: name.into(),
                output: String::new(),
                duration_ms: 1,
                is_error,
                sub_spec_index: None,
            }));
        }
        let t = p.finish(start);
        assert_eq!((t.llm_calls, t.input_tokens, t.output_tokens), (1, 12, 40));
        assert_eq!(
            (t.cache_read_input_tokens, t.cache_creation_input_tokens),
            (800, 3)
        );
        assert_eq!(
            (
                t.advisor_calls,
                t.advisor_input_tokens,
                t.advisor_output_tokens
            ),
            (1, 70, 9)
        );
        assert_eq!(t.memory_writes, 1);
        assert_eq!(t.errors, vec!["remember: ".to_string()]);
    }
}
