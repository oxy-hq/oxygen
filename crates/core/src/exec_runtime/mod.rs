//! Execution runtime: the live utilities the pre-agentic executor left behind
//! (its dead pipeline was removed across old-executor-retirement Phases 4a/4b).
//! The type vocabulary lives in the sibling `crate::exec_types`.
//!
//! `ExecutionContext` (the pre-agentic tool-execution hub) was retired once its
//! last consumers went away — the MCP tool server and Looker tool were removed,
//! and the eval harness moved to its own `EvalRun`; SQL now runs through
//! `agentic-connector`. What remains here is the shared IO: the renderer, the
//! event writer, and the formatters, used by airway / Data Apps / CLI.
pub mod formatters;
pub mod renderer;
pub mod writer;
