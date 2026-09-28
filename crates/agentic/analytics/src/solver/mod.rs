//! Analytics domain solver — split across per-state submodules.
//!
//! Each pipeline state has its own module:
//! - [`clarifying`]  — Clarify (triage + ground)
//! - [`specifying`]  — Specify (hybrid semantic-model + LLM, fan-out)
//! - [`solving`]     — Solve (SQL generation)
//! - [`executing`]   — Execute (connector dispatch, path-aware diagnosis)
//! - [`interpreting`]— Interpret (LLM narrative, multi-result merge)
//! - [`diagnosing`]  — Diagnose (error → recovery routing table)
//! - [`resuming`]    — HITL (ask_user, suspend/resume)
//! - [`prompts`]     — Shared prompt constants and formatting helpers

pub(crate) mod clarifying;
pub(crate) mod diagnosing;
pub(crate) mod executing;
pub(crate) mod freshness_hint;
pub(crate) mod interpreting;
pub(crate) mod prompts;
pub(crate) mod resuming;
pub(crate) mod solving;
pub(crate) mod specifying;

mod helpers;
pub(super) use helpers::{
    emit_core, emit_domain, fmt_result_shape, infer_result_shape, is_retryable_compile_error,
    lookup_connector, resolve_solution_connector, strip_json_fences,
};

mod fanout_worker;
mod solver;
pub use solver::AnalyticsSolver;

use std::collections::HashMap;

use agentic_core::orchestrator::StateHandler;

use crate::AnalyticsDomain;
use crate::events::AnalyticsEvent;

mod builder;

mod domain_solver;

// Table-driven handlers

/// Build the analytics-specific state handler table.
///
/// Each handler overrides the generic default with analytics-aware logic:
/// - **clarifying** — delegates to `clarify_impl`; short-circuits `GeneralInquiry`.
/// - **specifying** — hybrid: semantic model → LLM fallback; fan-out on multiple specs.
/// - **solving** — delegates to `solve_impl`; propagates `solution_source`.
/// - **executing** — path-aware diagnosis: `SemanticLayer` → Specify, `LlmWithSemanticContext` → Solve.
/// - **interpreting** — delegates to `interpret_impl`.
pub fn build_analytics_handlers()
-> HashMap<&'static str, StateHandler<AnalyticsDomain, AnalyticsSolver, AnalyticsEvent>> {
    let mut map = HashMap::new();
    map.insert("clarifying", clarifying::build_clarifying_handler());
    map.insert("specifying", specifying::build_specifying_handler());
    // Solving is absorbed into the specifying handler — no separate handler.
    map.insert("executing", executing::build_executing_handler());
    map.insert("interpreting", interpreting::build_interpreting_handler());
    map
}

// Tests (lifted into sibling file `solver/tests.rs`)

#[cfg(test)]
mod tests;

#[cfg(test)]
mod preagg_liveness_tests;
