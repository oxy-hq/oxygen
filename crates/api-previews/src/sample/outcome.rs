//! Between the Airway worker and the runtime: a sample's deadline, and what its
//! outcome records.
//!
//! [`watch`] forwards the worker's events and, when the sample's deadline
//! passes first, cancels the engine (its `CancellationToken`, the one the
//! runtime cancels through too). A sample cut off that way ends `Done` and
//! reads `partial`: the tables it loaded before the cut are real, and are
//! recorded like any others. A load the engine itself failed stays `Failed`.
//!
//! A sample's tables are recorded for the TTL drop as its load starts; a
//! `Done` sample's are recorded again from its stored schema, and its metadata
//! carries its compare with live under `sample` ([`super::record`]).

use std::time::Duration;

use agentic_core::delegation::TaskOutcome;
use agentic_core::hub_task::spawn_with_hub;
use agentic_runtime::worker::ExecutingTask;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::Deadline;
use super::record::{Recorded, record_sample, record_tables};

/// Everything [`watch`] needs to record the sample it watches.
pub(super) struct Watched {
    pub db: DatabaseConnection,
    pub workspace_id: uuid::Uuid,
    pub preview_key: String,
    pub run_id: String,
    pub pipeline: String,
    pub dataset: Option<String>,
    pub deadline: Deadline,
}

impl Watched {
    fn at(&self) -> Recorded<'_> {
        Recorded {
            workspace_id: self.workspace_id,
            preview_key: &self.preview_key,
            run_id: &self.run_id,
            pipeline: &self.pipeline,
            dataset: self.dataset.as_deref(),
        }
    }
}

/// The task the runtime drives in place of the worker's (module doc).
pub(super) fn watch(inner: ExecutingTask, watched: Watched) -> ExecutingTask {
    let (event_tx, events) = mpsc::channel(64);
    let (outcome_tx, outcomes) = mpsc::channel(4);
    let cancel = inner.cancel.clone();
    spawn_with_hub(async move {
        let (outcome, cut) = forward(inner, &watched, event_tx).await;
        let settled = settle(&watched, outcome, cut).await;
        let _ = outcome_tx.send(settled).await;
    });
    ExecutingTask {
        events,
        outcomes,
        cancel,
        answers: None,
    }
}

/// Forward events until the worker's outcome, cancelling the engine once if
/// the deadline passes first. The outcome, and whether the deadline cut it.
async fn forward(
    mut inner: ExecutingTask,
    watched: &Watched,
    events: mpsc::Sender<(String, Value)>,
) -> (Option<TaskOutcome>, bool) {
    let deadline = tokio::time::sleep(watched.deadline.after);
    tokio::pin!(deadline);
    let (mut cut, mut open) = (false, true);
    let outcome = loop {
        tokio::select! {
            event = inner.events.recv(), if open => match event {
                Some(event) => {
                    note(watched, &event).await;
                    let _ = events.send(event).await;
                }
                None => open = false,
            },
            outcome = inner.outcomes.recv() => break outcome,
            () = &mut deadline, if !cut => {
                tracing::info!(target: "preview", run_id = %watched.run_id,
                    reason = watched.deadline.reason, "cutting an Airway sample off");
                cut = true;
                inner.cancel.cancel();
            }
        }
    };
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(100), inner.events.recv()).await
    {
        note(watched, &event).await;
        let _ = events.send(event).await;
    }
    (outcome, cut)
}

/// A load about to start names its tables: record them before they exist.
async fn note(watched: &Watched, (event_type, payload): &(String, Value)) {
    if event_type != "destination_load_started" {
        return;
    }
    let tables: Vec<String> = payload["tables"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Err(e) = record_tables(&watched.db, &watched.at(), &tables).await {
        tracing::warn!(target: "preview", run_id = %watched.run_id, error = %e,
            "could not record a sample's tables as it loaded them");
    }
}

/// The outcome the runtime records (module doc).
async fn settle(watched: &Watched, outcome: Option<TaskOutcome>, cut: bool) -> TaskOutcome {
    // Only a sample that ends `Done` (cut off at its deadline included) is
    // compared and recorded from its stored schema; a failed one keeps only
    // the tables recorded as its load started ([`note`]).
    let cut_by = match (outcome, cut) {
        (Some(TaskOutcome::Done { metadata, .. }), _) => Ok((metadata, None)),
        (Some(TaskOutcome::Failed(_) | TaskOutcome::Cancelled), true) => {
            Ok((None, Some(watched.deadline.reason)))
        }
        (Some(other), _) => Err(other),
        (None, _) => Err(TaskOutcome::Failed(
            "the Airway sample ended without an outcome".into(),
        )),
    };
    match cut_by {
        Ok((metadata, cut_by)) => {
            let recorded = record_sample(&watched.db, &watched.at()).await;
            done(watched, metadata, recorded, cut_by)
        }
        Err(outcome) => outcome,
    }
}

/// `Done`, with the engine's metadata and the sample's own under `sample`.
fn done(
    watched: &Watched,
    engine: Option<Value>,
    recorded: Result<super::SampleReport, String>,
    cut_by: Option<&str>,
) -> TaskOutcome {
    let mut sample = match &recorded {
        Ok(report) => serde_json::to_value(report).unwrap_or(Value::Null),
        Err(e) => json!({ "pipeline": watched.pipeline, "record_error": e }),
    };
    sample["partial"] = json!(cut_by.is_some());
    sample["partial_reason"] = json!(cut_by.map(|why| format!("cut off at {why}")));
    let mut metadata = engine.unwrap_or_else(|| json!({}));
    metadata["sample"] = sample;
    let answer = match (&recorded, cut_by) {
        (_, Some(why)) => format!(
            "partial Airway sample of {}: cut off at {why}",
            watched.pipeline
        ),
        (Ok(r), None) => format!(
            "Airway sample of {}: {} table(s), {} finding(s) against live",
            watched.pipeline,
            r.tables.len(),
            r.findings.len()
        ),
        (Err(_), None) => format!("Airway sample of {}", watched.pipeline),
    };
    TaskOutcome::Done {
        answer,
        metadata: Some(metadata),
    }
}
