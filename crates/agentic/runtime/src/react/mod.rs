//! The react loop: a domain-free driver over an
//! [`Engine`](agentic_core::react::Engine) and a
//! [`Toolset`](agentic_core::react::Toolset).

mod ask_user;
mod budget;
mod dispatch;
mod driver;
mod events;
mod memory;
mod nudge;
mod results;
mod resume;
mod resume_guard;
mod store;
mod store_db;
#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

pub use ask_user::AskUser;
pub use budget::BudgetMeter;
pub use dispatch::DispatchCx as LoopCx;
pub use driver::{Budgets, LoopConfig, LoopOutcome, run_loop};
pub use events::ReactEvent;
pub use memory::{
    DbMemoryStore, FileMemoryStore, MAX_NOTE_CHARS, MEMORY_TOOLS, MemoryError, MemoryGate,
    MemoryStore, MergeError, MergeReport, Note, NoteFact, NoteOp, NoteOwner, REFLECT_PROMPT,
    REFLECT_STATE, Reflect, RunMemory, latest, memory_tools, merge_notes, notes_section,
};
pub use nudge::Nudge;
pub use results::{DEFAULT_PREVIEW_ROWS, Preview, ResultStore, StoredResult};
pub use resume::{ResumeInput, SuspensionInput, resume_loop, suspension_data};
pub use resume_guard::already_terminal;
pub use store::{InMemoryStore, Rebuilt, StoreError, TranscriptStore, rebuild};
pub use store_db::DbTranscriptStore;
