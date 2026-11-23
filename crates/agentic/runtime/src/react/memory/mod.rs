//! Memory across runs: short notes on how to answer in a workspace, which a
//! run reads in its system prompt and writes with `remember` and `forget`.
//!
//! A note is a fact log, not a row to update: every write appends a
//! [`NoteFact`], and the notes are the latest fact per `note_id` that is not a
//! delete ([`latest`]). That is the shape an Airhouse table needs (DuckLake has
//! no keys), so the served store can keep it. Design:
//! `internal-docs/agent-engine-runtime.md`, "Memory per user".
//!
//! Only the eval harness uses memory today ([`FileMemoryStore`]); a served run
//! has none.

mod file;
mod reflect;
mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use file::FileMemoryStore;
pub(super) use reflect::reflect;
pub use reflect::{REFLECT_PROMPT, REFLECT_STATE, Reflect};
pub use tools::{MemoryGate, memory_tools};

/// The names of the tools [`memory_tools`] returns.
pub const MEMORY_TOOLS: [&str; 2] = ["remember", "forget"];

/// The longest note `remember` accepts, in characters.
pub const MAX_NOTE_CHARS: usize = 400;
/// The most notes the prompt lists, newest first.
pub const MAX_PROMPT_NOTES: usize = 40;
/// The most characters of notes the prompt lists.
pub const MAX_PROMPT_CHARS: usize = 4_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteOp {
    Put,
    Delete,
}

/// One write to the note log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteFact {
    pub note_id: String,
    pub op: NoteOp,
    /// Empty for a delete.
    #[serde(default)]
    pub text: String,
    pub run_id: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// A live note: the latest `put` for its id.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub note_id: String,
    pub text: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

#[derive(thiserror::Error, Debug)]
pub enum MemoryError {
    #[error("memory store: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory store: bad row: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Where notes live. Appends only: a correction or a delete is a new fact.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn facts(&self) -> Result<Vec<NoteFact>, MemoryError>;
    async fn append(&self, fact: NoteFact) -> Result<(), MemoryError>;
}

/// The live notes in `facts`, newest first: the last fact per id wins (the
/// log's order, then `recorded_at`), and a delete removes the note.
pub fn latest(facts: Vec<NoteFact>) -> Vec<Note> {
    let mut by_id: HashMap<String, (usize, NoteFact)> = HashMap::new();
    for (i, f) in facts.into_iter().enumerate() {
        by_id.insert(f.note_id.clone(), (i, f));
    }
    let mut live: Vec<(usize, NoteFact)> = (by_id.into_values())
        .filter(|(_, f)| f.op == NoteOp::Put)
        .collect();
    live.sort_by(|a, b| (b.1.recorded_at, b.0).cmp(&(a.1.recorded_at, a.0)));
    (live.into_iter())
        .map(|(_, f)| Note {
            note_id: f.note_id,
            text: f.text,
            recorded_at: f.recorded_at,
        })
        .collect()
}

/// The system prompt section for `notes` (newest first), capped at
/// [`MAX_PROMPT_NOTES`] and [`MAX_PROMPT_CHARS`]. `None` when there are none.
pub fn notes_section(notes: &[Note]) -> Option<String> {
    let mut out = String::from(
        "What you learned in earlier runs (how to answer here, not answers; check a note \
         against the files when it matters):\n",
    );
    let mut listed = 0;
    for n in notes.iter().take(MAX_PROMPT_NOTES) {
        let line = format!("- [{}] {}\n", n.note_id, n.text);
        if listed > 0 && out.len() + line.len() > MAX_PROMPT_CHARS {
            break;
        }
        out.push_str(&line);
        listed += 1;
    }
    (listed > 0).then_some(out)
}

/// One run's memory: the notes its prompt lists, and where its writes go.
/// `writes: None` is a read-only run: no memory tools, no reflection turn.
#[derive(Clone, Default)]
pub struct RunMemory {
    pub notes: Vec<Note>,
    pub writes: Option<Arc<dyn MemoryStore>>,
}

#[cfg(test)]
pub(crate) mod tests;
