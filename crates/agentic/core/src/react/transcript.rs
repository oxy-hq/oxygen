//! The conversation the loop feeds an [`Engine`](super::Engine).

use serde::{Deserialize, Serialize};

use super::engine::{Turn, TurnUsage};

/// One message in the transcript.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Entry {
    User(String),
    Assistant(Turn),
    ToolResult {
        call_id: String,
        text: String,
        is_error: bool,
    },
}

/// A finished earlier turn of the conversation, used to seed a new run.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PriorTurn {
    pub question: String,
    pub answer: String,
    pub results_summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub system: String,
    pub entries: Vec<Entry>,
}

impl Transcript {
    /// A transcript of `prior` turns (a user entry and an assistant reply
    /// each; the reply text carries `results_summary`) followed by `question`.
    pub fn seeded(system: String, prior: &[PriorTurn], question: String) -> Self {
        let mut entries = Vec::with_capacity(prior.len() * 2 + 1);
        for p in prior {
            entries.push(Entry::User(p.question.clone()));
            let text = if p.results_summary.is_empty() {
                p.answer.clone()
            } else {
                format!("{}\n\n{}", p.answer, p.results_summary)
            };
            entries.push(Entry::Assistant(Turn {
                text,
                tool_calls: Vec::new(),
                native: serde_json::Value::Null,
                usage: TurnUsage::default(),
            }));
        }
        entries.push(Entry::User(question));
        Self { system, entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::react::engine::{Turn, TurnUsage};
    use serde_json::json;

    #[test]
    fn seeded_transcript_puts_prior_turns_before_the_question() {
        let prior = vec![
            PriorTurn {
                question: "q1".into(),
                answer: "a1".into(),
                results_summary: "s1".into(),
            },
            PriorTurn {
                question: "q2".into(),
                answer: "a2".into(),
                results_summary: "s2".into(),
            },
        ];
        let t = Transcript::seeded("sys".into(), &prior, "q3".into());
        assert_eq!(t.system, "sys");
        assert_eq!(t.entries.len(), 5);
        assert_eq!(t.entries[0], Entry::User("q1".into()));
        for (i, (a, s)) in [(1, ("a1", "s1")), (3, ("a2", "s2"))] {
            match &t.entries[i] {
                Entry::Assistant(turn) => {
                    assert!(turn.text.contains(a) && turn.text.contains(s));
                    assert!(turn.tool_calls.is_empty());
                }
                other => panic!("expected assistant, got {other:?}"),
            }
        }
        assert_eq!(t.entries[2], Entry::User("q2".into()));
        assert_eq!(t.entries[4], Entry::User("q3".into()));
    }

    #[test]
    fn entry_round_trips_through_json() {
        let turn = Turn {
            text: "hi".into(),
            tool_calls: vec![],
            native: json!([{"type": "thinking", "thinking": "hmm", "signature": "sig"}]),
            usage: TurnUsage {
                model: "m".into(),
                ..Default::default()
            },
        };
        for e in [
            Entry::Assistant(turn),
            Entry::ToolResult {
                call_id: "c1".into(),
                text: "boom".into(),
                is_error: true,
            },
        ] {
            let back: Entry = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
            assert_eq!(back, e);
        }
    }
}
