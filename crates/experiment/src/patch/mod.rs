//! Turning a qualifying slope into a driver-edge edit — and refusing, by name,
//! everything that does not qualify. See `internal-docs/experiments.md`
//! § "Write-back: the lever as an instrument".
//!
//! Three layers, each pure:
//! * [`plan`] — which edge, and the guards (no edge / non-linear / weak first stage …).
//! * [`text`] — a **line-level** edit of the `.view.yml` text. It rewrites only the
//!   lines of the one edge it edits; every comment, blank line, key order, quote
//!   style and line ending elsewhere is preserved byte for byte. A `serde_yaml`
//!   round-trip would have dropped every comment in the file.
//! * [`diff`] — the unified diff a reviewer reads.

pub mod diff;
pub mod plan;
pub mod text;

#[cfg(test)]
mod diff_tests;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;

pub use diff::unified_diff;
pub use plan::{
    CoefficientEdit, EdgeFacts, PatchRefusal, ReplacedPrior, ViewSource, WriteBackInputs,
    commit_message, edge_facts, plan_write_back,
};
pub use text::{RenderedPatch, apply_edit, render_patch};
