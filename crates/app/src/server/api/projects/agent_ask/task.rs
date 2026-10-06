//! What the queue carries for a custom-app ask, and what a claimed one is
//! driven with.
//!
//! An ask today is a spawn in the handler, on whichever serve replica took the
//! request: a deploy or a restart of that pod kills it, and the bundle's
//! stream waits for a terminal event that never comes. The queued form is one
//! `TaskSpec::Custom { kind: "custom_app_agent_ask" }` on a run the start has
//! already inserted, executed by [`super::executor::AgentAskExecutor`] on
//! whichever driver claims it.
//!
//! **Nothing enqueues this kind yet.** The executor is registered on every
//! driver one release ahead of the handler's enqueue, so that by the time a
//! serve replica queues an ask there is no pod left that would fail it as an
//! unknown kind (`internal-docs/worker-fleet.md` § "Agent asks").
//!
//! **The payload is empty, and that is the point.** Everything a driver needs
//! is on the run row the start inserted: the question, the thread, the
//! workspace, the agent (`metadata.agent_id`) and who asked
//! (`metadata.custom_app_caller`, [`super::caller::RunCaller`]). That last
//! record is the one `CallerRunResolver` reads to drive the ask's delegated
//! children and its resumes. Carrying a second copy of the caller in the
//! payload would let a run's root and its children disagree about who the run
//! is for; reading the one record cannot.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::entity::run;
use uuid::Uuid;

use super::caller::RunCaller;

/// `TaskSpec::Custom` discriminator of a queued ask.
///
/// Not the run's `source_type`: the start inserts the run as `analytics`,
/// which is what its stream, its transcript and a resume all key on. So the
/// placement gate (`drive_policy_for`, matched on `source_type`) sees an ask
/// as it sees a scheduled agent.
pub const AGENT_ASK_KIND: &str = "custom_app_agent_ask";

/// The spec a queued ask is enqueued with.
pub fn spec() -> TaskSpec {
    TaskSpec::Custom {
        kind: AGENT_ASK_KIND.to_string(),
        payload: serde_json::json!({}),
    }
}

/// Refuse any spec that is not a queued ask, rather than guess at it.
pub fn accept(spec: &TaskSpec) -> Result<(), String> {
    match spec {
        TaskSpec::Custom { kind, .. } if kind == AGENT_ASK_KIND => Ok(()),
        TaskSpec::Custom { kind, .. } => Err(format!("unknown agent-ask kind: {kind}")),
        other => Err(format!("unexpected spec for an agent ask: {other:?}")),
    }
}

/// What a claimed ask is driven with, read back from its run row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedAsk {
    pub run_id: String,
    /// The project (= workspace) the gate authorized the caller for.
    pub workspace_id: Uuid,
    pub agent_id: String,
    pub question: String,
    /// The conversation the ask belongs to. The start always provisions one;
    /// a follow-up reuses it, which is how the agent sees the earlier turns.
    pub thread_id: Option<Uuid>,
    /// Who asked. The run's context is built with this caller as its subject,
    /// at this pin — never with a driver's own subject-less context.
    pub caller: RunCaller,
}

impl QueuedAsk {
    /// Read the ask a run row describes.
    ///
    /// A row with no caller record is an error, not an ask to drive some other
    /// way: the only other context a driver has is its own, which carries no
    /// subject, and `airhouse_managed` mints a system Admin for that where the
    /// caller would have been a Reader.
    pub fn from_run(run: &run::Model) -> Result<Self, String> {
        let agent_id = run
            .metadata
            .as_ref()
            .and_then(|m| m.get("agent_id"))
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "the run names no agent".to_string())?;
        let caller = RunCaller::recorded(run.metadata.as_ref())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "the run records no caller, so there is nobody to run it as".to_string()
            })?;
        Ok(Self {
            run_id: run.id.clone(),
            workspace_id: run.workspace_id,
            agent_id: agent_id.to_string(),
            question: run.question.clone(),
            thread_id: run.thread_id,
            caller,
        })
    }
}

/// A `running` analytics root carrying `metadata`, for tests that read a run
/// row without a database.
#[cfg(test)]
pub(super) fn run_for_test(metadata: Option<serde_json::Value>) -> run::Model {
    let now = agentic_runtime::crud::now();
    run::Model {
        id: "run-1".into(),
        question: "how did the stores do?".into(),
        answer: None,
        error_message: None,
        thread_id: Some(Uuid::new_v4()),
        source_type: Some("analytics".into()),
        metadata,
        parent_run_id: None,
        schedule_id: None,
        task_status: Some("running".into()),
        task_metadata: None,
        attempt: 0,
        recovery_requested_at: None,
        driver_id: None,
        driver_heartbeat_at: None,
        cancel_requested_at: None,
        workspace_id: Uuid::new_v4(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::caller::RUN_CALLER_KEY;
    use super::*;

    fn caller() -> RunCaller {
        RunCaller {
            user_id: Uuid::new_v4(),
            staging_pin: None,
        }
    }

    /// What the start writes is what the driver reads: the question, the
    /// thread, the workspace, the agent and the caller, all off the one row.
    #[test]
    fn a_seeded_run_reads_back_as_the_ask_it_was_started_as() {
        let caller = caller();
        let run = run_for_test(Some(json!({
            "agent_id": "sales",
            "thinking_mode": null,
            RUN_CALLER_KEY: caller.to_metadata(),
        })));

        let ask = QueuedAsk::from_run(&run).expect("readable");

        assert_eq!(ask.run_id, run.id);
        assert_eq!(ask.workspace_id, run.workspace_id);
        assert_eq!(ask.agent_id, "sales");
        assert_eq!(ask.question, run.question);
        assert_eq!(ask.thread_id, run.thread_id, "a follow-up keeps its thread");
        assert_eq!(ask.caller, caller);
    }

    /// The identity rule: no record, no drive. Not "drive it as the tick".
    #[test]
    fn a_run_with_no_caller_record_is_refused() {
        for metadata in [None, Some(json!({ "agent_id": "sales" }))] {
            let err = QueuedAsk::from_run(&run_for_test(metadata)).unwrap_err();
            assert!(
                err.contains("no caller") || err.contains("no agent"),
                "{err}"
            );
        }
        let unreadable = run_for_test(Some(json!({
            "agent_id": "sales",
            RUN_CALLER_KEY: { "user_id": "not-a-uuid" },
        })));
        let err = QueuedAsk::from_run(&unreadable).unwrap_err();
        assert!(err.contains("unreadable"), "{err}");
    }

    #[test]
    fn a_run_that_names_no_agent_is_refused() {
        for agent in [json!(null), json!(""), json!(7)] {
            let run = run_for_test(Some(json!({
                "agent_id": agent,
                RUN_CALLER_KEY: caller().to_metadata(),
            })));
            let err = QueuedAsk::from_run(&run).unwrap_err();
            assert!(err.contains("no agent"), "{err}");
        }
    }

    #[test]
    fn only_a_queued_ask_is_accepted() {
        assert!(accept(&spec()).is_ok());
        let other_kind = TaskSpec::Custom {
            kind: "custom_app_procedure_run".into(),
            payload: json!({}),
        };
        assert!(
            accept(&other_kind)
                .unwrap_err()
                .contains("unknown agent-ask kind")
        );
        let agent = TaskSpec::Agent {
            agent_id: "sales".into(),
            question: "q".into(),
            extra: None,
        };
        assert!(accept(&agent).unwrap_err().contains("unexpected spec"));
    }
}
