//! The react loop: a domain-free driver over an
//! [`Engine`](agentic_core::react::Engine) and a
//! [`Toolset`](agentic_core::react::Toolset).

mod budget;
mod dispatch;
mod driver;
mod events;
mod results;
mod resume;
mod resume_guard;
mod store;
mod store_db;
#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

pub use budget::BudgetMeter;
pub use dispatch::DispatchCx as LoopCx;
pub use driver::{Budgets, LoopConfig, LoopOutcome, run_loop};
pub use events::ReactEvent;
pub use results::{Preview, ResultStore, StoredResult};
pub use resume::{ResumeInput, SuspensionInput, resume_loop, suspension_data};
pub use store::{InMemoryStore, Rebuilt, StoreError, TranscriptStore, rebuild};
pub use store_db::DbTranscriptStore;
