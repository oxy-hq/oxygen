//! What an attempt that does not start the pipeline hands back, without a
//! database. The cases that need one — a real run, a real stamp — are in
//! `tests/custom_apps/custom_app_agent_ask_queue.rs`.

use agentic_core::delegation::TaskSpec;

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
/// queued ask that fails before its pipeline runs leaves the stream open.
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

/// Refused before anything is read: the executor never treats another kind's
/// payload as an ask.
#[tokio::test]
async fn a_spec_of_another_kind_is_refused_before_any_read() {
    let executor = AgentAskExecutor {
        // Disconnected: a read would fail the test with a different error.
        db: DatabaseConnection::default(),
        schema_cache: None,
    };
    let refused = executor
        .execute(TaskAssignment {
            task_id: "run-1".into(),
            parent_task_id: None,
            run_id: "run-1".into(),
            spec: TaskSpec::Custom {
                kind: "custom_app_procedure_run".into(),
                payload: serde_json::json!({}),
            },
            policy: None,
        })
        .await;
    assert!(refused.is_err_and(|e| e.contains("unknown agent-ask kind")));
}

/// Nothing has started when the context is being built, so a failure that
/// may pass is tried again rather than ending the ask: a failed read of the
/// workspace row, and a context that answers "not compiled yet".
#[test]
fn a_context_that_may_build_a_minute_later_is_retried_not_failed() {
    for transient in [
        PrepareError::Context(CallerContextError::Lookup("connection reset".into())),
        PrepareError::Context(CallerContextError::Build(503)),
    ] {
        let reason = transient.to_string();
        let settled = unprepared("run-1", transient);
        assert!(settled.event.is_none(), "{reason}: nothing for the log");
        assert!(
            matches!(settled.outcome, TaskOutcome::Deferred { .. }),
            "{reason}: {:?}",
            settled.outcome
        );
    }
}

/// Everything else ends the ask, on its log: a run that is not an ask anyone
/// may drive, a workspace that is gone, and a context the builder refused in
/// a way it cannot tell from permanent.
#[test]
fn a_context_that_will_not_build_ends_the_ask() {
    for permanent in [
        PrepareError::NotAnAsk("the run records no caller".into()),
        PrepareError::Context(CallerContextError::Unreadable("missing field".into())),
        PrepareError::Context(CallerContextError::WorkspaceGone),
        PrepareError::Context(CallerContextError::Build(500)),
        PrepareError::Context(CallerContextError::Build(404)),
    ] {
        let reason = permanent.to_string();
        let settled = unprepared("run-1", permanent);
        let (event_type, payload) = settled.event.expect("a failure is on the log");
        assert_eq!(event_type, "error", "{reason}");
        assert_eq!(payload["message"], reason);
        assert!(
            matches!(&settled.outcome, TaskOutcome::Failed(m) if *m == reason),
            "{reason}: {:?}",
            settled.outcome
        );
    }
}
