//! What an ask's attempt decides before its pipeline, without a database. The
//! cases that need one — a real run, a real stamp — are in
//! `oxy-api-custom-apps`'s `custom_app_agent_ask_queue.rs`.

use agentic_core::delegation::{TaskOutcome, TaskSpec};

use super::*;

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
            claim_epoch: agentic_core::NO_CLAIM,
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
