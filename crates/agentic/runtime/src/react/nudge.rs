//! A one-time note the loop adds to a tool result when a run looks stuck.
//!
//! A prompt rule like "call the advisor when stuck" did not work on BIRD:
//! Sonnet 5.5 ran 14, 20 and 101 queries without an answer and never called
//! it. The loop counts calls itself, so the trigger does not depend on the
//! model's judgement.

use std::collections::HashSet;

use agentic_core::react::{Entry, Transcript};

/// Add `text` to a result of `tool` once that tool has run `after_calls`
/// times, or at its first failed call, whichever comes first. Fires once per
/// run, on the result that meets the trigger.
#[derive(Clone, Debug)]
pub struct Nudge {
    pub tool: String,
    pub after_calls: u32,
    pub text: String,
}

/// A [`Nudge`] and how far one run has got towards it.
///
/// The note is added to the result text before the result is recorded, so the
/// run's log holds it, and a resumed run rebuilds this state from the log
/// ([`NudgeState::resumed`]) instead of starting over.
#[derive(Debug)]
pub struct NudgeState {
    nudge: Nudge,
    calls: u32,
    fired: bool,
}

impl NudgeState {
    /// The state after `transcript`: the results of the tool it already
    /// holds (a call still waiting for its result is not counted yet), and
    /// whether one of them already carries the note.
    pub(super) fn resumed(nudge: Nudge, transcript: &Transcript) -> Self {
        let mut ours = HashSet::new();
        let (mut calls, mut fired) = (0, false);
        for entry in &transcript.entries {
            match entry {
                Entry::Assistant(turn) => ours.extend(
                    (turn.tool_calls.iter())
                        .filter(|c| c.name == nudge.tool)
                        .map(|c| c.id.as_str()),
                ),
                Entry::ToolResult { call_id, text, .. } if ours.contains(call_id.as_str()) => {
                    calls += 1;
                    fired |= text.contains(&nudge.text);
                }
                _ => {}
            }
        }
        Self {
            nudge,
            calls,
            fired,
        }
    }

    /// Counts one result of `tool` and returns the note to add to it, the
    /// first time the trigger holds: the tool's `after_calls`-th call, or its
    /// first failed one.
    pub(super) fn note(&mut self, tool: &str, is_error: bool) -> Option<String> {
        if self.fired || tool != self.nudge.tool {
            return None;
        }
        self.calls += 1;
        if !is_error && self.calls < self.nudge.after_calls {
            return None;
        }
        self.fired = true;
        Some(format!("\n\n{}", self.nudge.text))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::testkit::{self, call};
    use super::*;

    fn nudge() -> Nudge {
        Nudge {
            tool: "query".into(),
            after_calls: 3,
            text: "NUDGE".into(),
        }
    }

    fn notes(s: &mut NudgeState, results: &[(&str, bool)]) -> Vec<Option<String>> {
        results.iter().map(|(t, e)| s.note(t, *e)).collect()
    }

    #[test]
    fn fires_on_the_third_query_once_and_ignores_other_tools() {
        let mut s = NudgeState::resumed(nudge(), &Transcript::default());
        let got = notes(
            &mut s,
            &[("query", false), ("read", false), ("query", false)],
        );
        assert_eq!(got, [None, None, None]);
        assert_eq!(s.note("query", false).as_deref(), Some("\n\nNUDGE"));
        assert_eq!(s.note("query", true), None, "fires once");
    }

    #[test]
    fn a_failed_query_fires_it_early() {
        let mut s = NudgeState::resumed(nudge(), &Transcript::default());
        assert_eq!(s.note("query", true).as_deref(), Some("\n\nNUDGE"));
    }

    #[test]
    fn a_failed_other_tool_does_not_fire_it() {
        let mut s = NudgeState::resumed(nudge(), &Transcript::default());
        assert_eq!(
            notes(&mut s, &[("read", true), ("query", false)]),
            [None, None]
        );
    }

    fn turn(calls: &[(&str, &str)]) -> Entry {
        let calls = calls.iter().map(|(id, n)| call(id, n, json!({}))).collect();
        Entry::Assistant(testkit::turn("", calls))
    }

    fn result(id: &str, text: &str) -> Entry {
        Entry::ToolResult {
            call_id: id.into(),
            text: text.into(),
            is_error: false,
        }
    }

    #[test]
    fn a_resumed_run_counts_recorded_results_and_does_not_fire_twice() {
        let mut t = Transcript::default();
        let calls = [
            ("1", "query"),
            ("2", "read"),
            ("3", "query"),
            ("4", "query"),
        ];
        t.entries = vec![turn(&calls), result("1", "rows"), result("2", "rows")];
        // Calls 3 and 4 have no result yet: one recorded query so far.
        let mut s = NudgeState::resumed(nudge(), &t);
        assert_eq!(s.note("query", false), None);
        assert_eq!(s.note("query", false).as_deref(), Some("\n\nNUDGE"));
        t.entries.push(result("3", "rows"));
        t.entries.push(result("4", "rows\n\nNUDGE"));
        let mut s = NudgeState::resumed(nudge(), &t);
        assert_eq!(s.note("query", true), None, "already fired");
    }
}
