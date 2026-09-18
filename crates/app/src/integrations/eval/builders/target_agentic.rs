//! Eval target on the agentic path — the "run the target agent" step of the
//! eval harness, with no old-executor scaffolding.
//!
//! Uses **none** of the old-executor's pipeline machinery — no `Executable`,
//! `execute_with_handler`, `OutputContainer` / `OutputGetter`, or
//! `EventHandler` / `writer`. See `internal-docs/old-executor-retirement.md`.
//!
//! Why the target step reduces to answer text + duration + token usage:
//!   1. The agent runs via `agentic_pipeline::run_agentic_streaming` directly.
//!   2. For a `.test.yml` agentic target `eval.rs` sets `task_ref: None`, and a
//!      single text answer carries no `relevant_contexts` / `references`.
//!   3. The correctness judge (`solver::run_solver` via `render_correctness_prompt`)
//!      reads only `TargetOutput.output` (+ `task_description` on the *expected*
//!      side) — it never touches `relevant_contexts` / `references`.
//!
//! `TargetOutput` is still imported from `oxy::exec_types`; relocating that
//! data type out of the old executor is a later (Tier-C) step.

use std::sync::Arc;

use agentic_analytics::AnalyticsEvent;
use agentic_core::events::{CoreEvent, Event as AgenticEvent};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::exec_types::TargetOutput;
use oxy_shared::errors::OxyError;
use tokio::sync::mpsc;

use super::types::AgenticInput;

/// Run one eval target agent and produce its [`TargetOutput`], on the agentic
/// path with zero old-executor scaffolding. Returns a single-element `Vec`
/// (one target run → one output) that `run_generator` pairs with its expected
/// answer.
pub(super) async fn run_target(
    workspace: &WorkspaceManager<oxy::config::WorkingCopy>,
    input: AgenticInput,
) -> Result<Vec<TargetOutput>, OxyError> {
    let start = std::time::Instant::now();

    // Resolve to absolute so the agentic loader reads from the right place
    // regardless of process CWD (same as target.rs).
    let resolved = workspace
        .config_manager
        .resolve_file(&input.config_path)
        .await
        .map_err(|e| {
            OxyError::ConfigurationError(format!(
                "Failed to resolve agentic config path '{}': {e}",
                input.config_path
            ))
        })?;

    let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
        workspace.clone(),
    ));
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx;

    // Capture token usage straight off the agentic event stream — no
    // `EventKind::Usage` round-trip, no `UsageAccumulatorHandler`, no writer.
    let (event_tx, mut event_rx) = mpsc::channel::<AgenticEvent<AnalyticsEvent>>(256);
    let usage_consumer = tokio::spawn(async move {
        let (mut input_tokens, mut output_tokens) = (0i32, 0i32);
        while let Some(ev) = event_rx.recv().await {
            let (di, dobj) = usage_delta(&ev);
            input_tokens = input_tokens.saturating_add(di);
            output_tokens = output_tokens.saturating_add(dobj);
        }
        (input_tokens, output_tokens)
    });

    let answer_text = agentic_pipeline::run_agentic_streaming(
        platform,
        std::path::Path::new(&resolved),
        input.prompt,
        event_tx,
    )
    .await
    .map_err(OxyError::RuntimeError)?;

    let (input_tokens, output_tokens) = usage_consumer
        .await
        .map_err(|e| OxyError::RuntimeError(format!("usage consumer join: {e}")))?;

    Ok(vec![build_target_output(
        answer_text,
        start.elapsed().as_secs_f64() * 1000.0,
        input_tokens,
        output_tokens,
    )])
}

/// Per-event token delta `(input, output)` — the same match arms target.rs uses,
/// extracted so it is unit-testable without a live pipeline.
fn usage_delta(ev: &AgenticEvent<AnalyticsEvent>) -> (i32, i32) {
    if let AgenticEvent::Core(core) = ev {
        match core {
            CoreEvent::LlmStart { prompt_tokens, .. } => (*prompt_tokens as i32, 0),
            CoreEvent::LlmEnd { output_tokens, .. } => (0, *output_tokens as i32),
            _ => (0, 0),
        }
    } else {
        (0, 0)
    }
}

/// Build the `TargetOutput` the correctness judge consumes. For the agentic
/// `.test.yml` path this is exactly what the old `OutputContainer::Single` arm
/// produced: the answer text, no task_description, and empty context/refs.
fn build_target_output(
    answer_text: String,
    duration_ms: f64,
    input_tokens: i32,
    output_tokens: i32,
) -> TargetOutput {
    TargetOutput {
        output: answer_text,
        task_description: None,
        relevant_contexts: vec![],
        references: vec![],
        duration_ms,
        input_tokens,
        output_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The judge (`CorrectnessSolverMapper`) reads `submission.output` as the
    /// actual answer and never reads `relevant_contexts` / `references`. Assert
    /// the spike's builder produces exactly that shape — the same one the old
    /// `OutputContainer::Single` → `TargetOutput` conversion yields.
    #[test]
    fn build_target_output_matches_single_container_shape() {
        let out = build_target_output("42 orders".to_string(), 12.5, 100, 20);
        assert_eq!(out.output, "42 orders");
        assert_eq!(out.task_description, None);
        assert!(out.relevant_contexts.is_empty());
        assert!(out.references.is_empty());
        assert_eq!(out.input_tokens, 100);
        assert_eq!(out.output_tokens, 20);
        assert_eq!(out.duration_ms, 12.5);
    }
}
