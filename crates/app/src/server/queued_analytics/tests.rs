//! What an attempt that does not start the pipeline hands back, without a
//! database.

use super::*;

/// A settled attempt is a task like any other to the worker that forwards it:
/// its event, then its outcome, then both channels closed — so the worker's
/// loops end and the claim is given up.
#[tokio::test]
async fn a_settled_attempt_reports_its_event_then_its_outcome_and_closes() {
    let mut task = Settled::failed("no model is configured".to_string()).into_task();

    let (event_type, payload) = task.events.recv().await.expect("the event");
    assert_eq!(event_type, "error");
    assert_eq!(payload["message"], "no model is configured");
    assert!(task.events.recv().await.is_none(), "one event, then closed");

    let outcome = task.outcomes.recv().await.expect("the outcome");
    assert!(matches!(&outcome, TaskOutcome::Failed(m) if m == "no model is configured"));
    assert!(
        task.outcomes.recv().await.is_none(),
        "one outcome, then closed"
    );
}

#[tokio::test]
async fn an_attempt_with_nothing_for_the_log_reports_only_its_outcome() {
    let mut task = Settled::only(TaskOutcome::Cancelled).into_task();

    assert!(task.events.recv().await.is_none());
    assert!(matches!(
        task.outcomes.recv().await,
        Some(TaskOutcome::Cancelled)
    ));
}

/// The bundle reads the run's event log through the analytics stream
/// processor and closes its stream on `error` (`agent_run_stream.rs`). The
/// event a failed attempt writes has to be one that processor recognises, or a
/// queued run that fails before its pipeline runs leaves the stream open.
#[test]
fn the_failure_event_is_one_the_bundles_stream_ends_on() {
    let settled = Settled::failed("the agent was interrupted".to_string());
    let (event_type, payload) = settled.event.expect("a failure is on the log");

    let ui = agentic_pipeline::build_event_registry()
        .stream_processor("analytics")
        .process(&event_type, &payload);

    assert_eq!(ui.len(), 1, "{ui:?}");
    assert_eq!(ui[0].0, "error");
    assert_eq!(ui[0].1["message"], "the agent was interrupted");
}
