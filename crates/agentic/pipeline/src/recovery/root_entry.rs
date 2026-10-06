//! A root that has reached a suspension, and the queue entry it still has.
//!
//! A suspended task keeps its claim, so the same `task_id` can resume in
//! place. When the driver holding that claim dies, the reaper (or a graceful
//! release) puts the row back to `queued` — carrying the spec it was claimed
//! with. Recovery then has two descriptions of how the root continues: the
//! suspension it is parked at, and a `queued` entry its own worker is about to
//! claim. Acting on both is how a run got resumed from its checkpoint *and*
//! started again from the top.
//!
//! [`reconcile`] makes them agree before any worker exists, and tells the tree
//! walk which one is the root's continuation.

use agentic_core::delegation::TaskSpec;
use agentic_core::human_input::SuspendedRunData;
use agentic_runtime::entity::task_queue;
use agentic_runtime::transport::DurableTransport;
use sea_orm::DatabaseConnection;

/// How the root's own queue entry relates to the suspension the root is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RootEntry {
    /// Nothing to reconcile: the root never suspended, its entry is not
    /// `queued`, or the entry belongs to a domain that re-drives itself (an
    /// automation, whose decider is stateless and whose duplicate delegations
    /// are refused). The walk proceeds as it always has.
    Unchanged,
    /// `queued`, and it *is* the continuation: the `Resume` the coordinator
    /// assigned from the checkpoint the root is at. The worker runs it, with
    /// the answer it carries; the walk must not resume the root a second time.
    Continues,
    /// `queued`, with a spec from before the suspension the root is at: the
    /// one the run was started with, or a `Resume` from an earlier checkpoint.
    /// Running it would repeat work the run has already done, beside the root
    /// this walk resumes or leaves parked. Taken out of `queued` instead.
    Stale,
}

/// Read the root's entry against its suspension. Pure, so the mapping — which
/// is the whole of "is this run started twice" — is assertable without a
/// database.
pub(super) fn classify(
    suspension: Option<&SuspendedRunData>,
    entry: Option<&task_queue::Model>,
) -> RootEntry {
    let (Some(suspension), Some(entry)) = (suspension, entry) else {
        return RootEntry::Unchanged;
    };
    if entry.queue_status != "queued" {
        return RootEntry::Unchanged;
    }
    // An entry this binary cannot read is the worker's to fail, as it already
    // does at claim time.
    let Ok(spec) = serde_json::from_value::<TaskSpec>(entry.spec.clone()) else {
        return RootEntry::Unchanged;
    };
    match spec {
        TaskSpec::Automation { .. }
        | TaskSpec::AutomationStep { .. }
        | TaskSpec::AutomationDecision { .. } => RootEntry::Unchanged,
        TaskSpec::Resume { resume_data, .. } if same_checkpoint(&resume_data, suspension) => {
            RootEntry::Continues
        }
        _ => RootEntry::Stale,
    }
}

/// Is `queued` the checkpoint the root is suspended at? Compared as stored:
/// both sides were written from the same `SuspendedRunData` and read back
/// through the same JSON column type.
fn same_checkpoint(queued: &SuspendedRunData, current: &SuspendedRunData) -> bool {
    match (serde_json::to_value(queued), serde_json::to_value(current)) {
        (Ok(queued), Ok(current)) => queued == current,
        _ => false,
    }
}

/// Make the root's queue entry agree with its suspension, and report which of
/// the two the root continues from. Call it once, before the walk and before
/// the worker is spawned: a [`RootEntry::Stale`] entry is taken as this
/// driver's claim here, so no worker is ever handed it.
///
/// An error is a failed recovery, not a reason to carry on: continuing without
/// knowing would be the double start this exists to prevent.
pub(super) async fn reconcile(
    db: &DatabaseConnection,
    transport: &DurableTransport,
    root_id: &str,
) -> Result<RootEntry, String> {
    let suspension = agentic_runtime::crud::get_suspension(db, root_id)
        .await
        .map_err(|e| format!("failed to read the root's suspension: {e}"))?;
    if suspension.is_none() {
        return Ok(RootEntry::Unchanged);
    }
    let entry = agentic_runtime::crud::get_queue_entry(db, root_id)
        .await
        .map_err(|e| format!("failed to read the root's queue entry: {e}"))?;
    let verdict = classify(suspension.as_ref(), entry.as_ref());
    if verdict == RootEntry::Stale {
        let adopted = transport
            .adopt_queued_claim(root_id)
            .await
            .map_err(|e| format!("failed to hold the suspended root's queue entry: {e}"))?;
        tracing::info!(
            target: "recovery",
            run_id = %root_id,
            adopted,
            "root is at a suspension and its queue entry predates it; holding the \
             entry so no worker starts the run again"
        );
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn checkpoint(stage: &str) -> SuspendedRunData {
        SuspendedRunData {
            from_state: stage.into(),
            original_input: "how did the stores do?".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({ "stage": stage }),
            question: "which store?".into(),
            suggestions: vec!["store 7".into()],
        }
    }

    fn entry(status: &str, spec: serde_json::Value) -> task_queue::Model {
        let now = agentic_runtime::crud::now();
        task_queue::Model {
            task_id: "run-1".into(),
            run_id: "run-1".into(),
            parent_task_id: None,
            queue_status: status.into(),
            spec,
            policy: None,
            worker_id: None,
            last_heartbeat: None,
            claimed_at: None,
            visibility_timeout_secs: 60,
            claim_count: 1,
            max_claims: 3,
            scope_owned: false,
            available_at: now,
            first_deferred_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn queued(spec: &TaskSpec) -> task_queue::Model {
        entry("queued", serde_json::to_value(spec).unwrap())
    }

    fn agent() -> TaskSpec {
        TaskSpec::Agent {
            agent_id: "sales".into(),
            question: "how did the stores do?".into(),
            extra: None,
        }
    }

    fn resume_from(stage: &str) -> TaskSpec {
        TaskSpec::Resume {
            run_id: "run-1".into(),
            resume_data: checkpoint(stage),
            answer: "store 7".into(),
        }
    }

    /// The reported case: the entry still carries the spec the run was
    /// started with, whatever kind that was.
    #[test]
    fn a_queued_start_spec_under_a_suspended_root_is_stale() {
        let at = checkpoint("clarifying");
        for start in [
            agent(),
            TaskSpec::Custom {
                kind: "custom_app_agent_ask".into(),
                payload: json!({}),
            },
        ] {
            assert_eq!(
                classify(Some(&at), Some(&queued(&start))),
                RootEntry::Stale,
                "{start:?}"
            );
        }
    }

    /// A run that suspended twice still holds the claim it resumed on the
    /// first time. Requeued, that resume would replay everything between the
    /// two suspensions.
    #[test]
    fn a_queued_resume_from_an_earlier_checkpoint_is_stale() {
        let at = checkpoint("interpreting");
        assert_eq!(
            classify(Some(&at), Some(&queued(&resume_from("clarifying")))),
            RootEntry::Stale
        );
    }

    #[test]
    fn a_queued_resume_from_the_current_checkpoint_is_the_continuation() {
        let at = checkpoint("clarifying");
        assert_eq!(
            classify(Some(&at), Some(&queued(&resume_from("clarifying")))),
            RootEntry::Continues
        );
    }

    /// Without a suspension the entry is the only description of the run, and
    /// it stays the worker's: a freshly seeded run is exactly this.
    #[test]
    fn a_root_that_never_suspended_is_left_alone() {
        assert_eq!(
            classify(None, Some(&queued(&agent()))),
            RootEntry::Unchanged
        );
        assert_eq!(classify(None, None), RootEntry::Unchanged);
    }

    /// Only a `queued` entry can be handed to a worker.
    #[test]
    fn an_entry_no_worker_can_claim_is_left_alone() {
        let at = checkpoint("clarifying");
        assert_eq!(classify(Some(&at), None), RootEntry::Unchanged);
        let start = serde_json::to_value(agent()).unwrap();
        for status in ["claimed", "completed", "failed", "cancelled", "dead"] {
            assert_eq!(
                classify(Some(&at), Some(&entry(status, start.clone()))),
                RootEntry::Unchanged,
                "{status}"
            );
        }
    }

    /// An automation re-drives itself through its queue entry: the decider is
    /// stateless and a duplicate delegation is refused. Holding that entry
    /// back would park the run with nothing left to move it.
    #[test]
    fn an_automation_entry_is_left_to_its_own_domain() {
        let at = checkpoint("workflow_decision");
        for spec in [
            TaskSpec::Automation {
                workflow_ref: "weekly.automation.yml".into(),
                variables: None,
                retry_from_run_id: None,
                cache_enabled: false,
                body: None,
                initial_render_context: None,
            },
            TaskSpec::AutomationDecision {
                run_id: "run-1".into(),
                pending_child_answer: None,
            },
            TaskSpec::AutomationStep {
                step_config: json!({}),
                render_context: json!({}),
                workflow_context: json!({}),
            },
        ] {
            assert_eq!(
                classify(Some(&at), Some(&queued(&spec))),
                RootEntry::Unchanged,
                "{spec:?}"
            );
        }
    }

    #[test]
    fn an_entry_this_binary_cannot_read_is_left_for_the_worker_to_fail() {
        let at = checkpoint("clarifying");
        let unreadable = entry("queued", json!({ "type": "from_a_newer_binary" }));
        assert_eq!(classify(Some(&at), Some(&unreadable)), RootEntry::Unchanged);
    }
}
