//! An `airway` step inside a workspace-preview run: held, never run.
//!
//! A pipeline run is a write the preview platform cannot see into — it lands
//! rows through its own destination connection, advances a cursor, takes the
//! single-flight lease and may rotate a token. Phase 2a therefore holds every
//! airway step: the step succeeds, so the procedure goes on, and its result
//! records the pipeline it would have run, in the same `preview` note a held
//! `execute_sql` step carries.

use agentic_core::delegation::TaskOutcome;
use agentic_runtime::worker::ExecutingTask;
use serde_json::{Value, json};

use crate::platform::{AirwayStepMode, PreviewScope};

/// The `preview` note of a held airway step. `pipeline_ref` is reported
/// unscoped: the scoped form is an internal fence, not something a person
/// reading the run should have to decode.
pub(super) fn held_airway_note(scope: &PreviewScope, pipeline_ref: &str) -> Value {
    let pipeline_ref = agentic_automation::preview_names::unscope(pipeline_ref, &scope.run_id)
        .unwrap_or(pipeline_ref);
    let AirwayStepMode::Hold = scope.airway_steps;
    json!({
        "held": true,
        "reason": "Airway pipelines are not run in a workspace preview; the step records what it would have run.",
        "verb": "AIRWAY RUN",
        "targets": [pipeline_ref],
        "pipeline_ref": pipeline_ref,
    })
}

/// A task that is already `Done`, carrying the hold both where the automation
/// folds a child's answer into the step result (`answer`) and where a reader of
/// the outcome looks for it (`metadata.preview`).
pub(super) fn held_airway_task(scope: &PreviewScope, pipeline_ref: &str) -> ExecutingTask {
    let note = held_airway_note(scope, pipeline_ref);
    tracing::info!(
        target: "preview",
        run_id = %scope.run_id,
        pipeline_ref = %note["pipeline_ref"],
        "preview_airway_held"
    );
    // No run event: the hold is the step's result, which the run viewer
    // already shows, and an event type no domain registered has no reader.
    let (events_tx, events) = tokio::sync::mpsc::channel(1);
    drop(events_tx);
    let (outcomes_tx, outcomes) = tokio::sync::mpsc::channel(1);
    // Capacity 1 and a single send, so this cannot block.
    let _ = outcomes_tx.try_send(TaskOutcome::Done {
        answer: json!({ "preview": note.clone() }).to_string(),
        metadata: Some(json!({ "preview": note })),
    });
    ExecutingTask {
        events,
        outcomes,
        cancel: tokio_util::sync::CancellationToken::new(),
        answers: None,
    }
}
