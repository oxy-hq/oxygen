//! The advisory notes a model writes on a staging attempt's checks, once the
//! attempt has settled and where the app's org has the `promote_checks_gate`
//! flag on (`internal-docs/custom-app-sandboxes.md` section 5.9, "The
//! model's notes").
//!
//! **One queued task per staging attempt** ([`task`], kind
//! [`BUILD_CHECKS_AI_KIND`]), queued by `settled::on_staging_attempt_settled`
//! and run on the worker fleet ([`executor`]). Its run id is the attempt's, so
//! queueing it again finds the run it already wrote.
//!
//! **Which model.** The default model of the app's own workspace, reached
//! through `agentic-pipeline`'s prepared one-shot ([`model`]): its provider,
//! its key, nothing else's. With none resolvable the task writes its two rows
//! as `skipped`, saying so, and no note.
//!
//! **What it writes** ([`run`]), in this order, each capped, each `author:
//! "model"` with the model's name:
//!
//! | Where | What | From |
//! | --- | --- | --- |
//! | each check that ended `failed` or `error` | one note: why, and what to change | that check's summary and findings |
//! | the `changes` row, when screens differ | one note sorting them into "Look here first" and "Looks intended" | every changed screen's two pictures, its share of differing pixels and its two visible texts |
//! | a `design_audit` row (advisory; `passed`, its findings as its note, or `skipped`) | what departs from the design system | `css-digest.json` and the changed screens' staging pictures |
//! | the build's `summary` row | three or four lines: what the build changes against production, and where the risk is | the promote plan's own diff and the checks |
//!
//! **It only informs.** It never writes a check's status, summary or detail:
//! a note goes into `notes`, and the only rows it writes whole are the two
//! model kinds (`kind::MODEL_KINDS`), always `advisory`, so neither is part of
//! an attempt's state, of what a promote waits for, or of what stalls. A model
//! call that fails changes no status either: the note, or the row, says the
//! note could not be written.
//!
//! **Pictures** ([`visual`]) are the stored PNGs of every changed screen, read
//! from the checks' files and sent to the workspace's own provider and nowhere
//! else. The only bounds are that provider's documented ones: one request is
//! filled until it can carry no more, then the next is started, and the
//! answers are joined. Nothing is downscaled. Where the provider takes no
//! pictures, or refuses them, the note is written from the text inputs and
//! says the model did not see the pictures.
//!
//! **What a page shows is data, never an instruction** ([`prompts`]). A
//! screen's text, the CSS and a check's findings come from the app under
//! review: they reach the model inside marked blocks it is told to treat as
//! material, with every `<` and `>` replaced so nothing in them can close a
//! block. Whatever the model answers is stored as a note's text, capped, and
//! is never read back as a status, a finding or an action.
//!
//! **Nothing is logged.** No picture, no page text, no prompt and no answer
//! reaches a log line or an inference span (the calls are `private`); a failed
//! call is logged by its class (`auth`, `rate_limit`, ...).
//!
//! **Safe to run twice.** A note replaces the model's earlier note on the
//! same row, the two rows are written by key, and a task whose `summary` row
//! is already there does nothing.

pub mod executor;
pub mod inputs;
pub mod model;
pub mod notes;
pub mod prompts;
pub mod run;
pub mod screens;
pub mod summary;
pub mod task;
pub mod visual;

#[cfg(test)]
mod advisory_tests;

pub use task::BUILD_CHECKS_AI_KIND;
