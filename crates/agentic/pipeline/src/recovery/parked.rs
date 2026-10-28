//! A root parked on a person, which the startup pass leaves where it is.
//!
//! [`super::park`] ends a parked root's claim (its entry is `completed`) and
//! releases its driver lease, leaving it `awaiting_input` with its checkpoint on
//! record. An unleased active root is exactly what
//! `get_resumable_root_runs` selects, so without this check every pod's
//! startup pass (twice per boot) would rebuild every parked chat and ask ever
//! created, log a `recovery_resumed` over it (the web app marks its open step
//! interrupted on that event), and park it again.
//!
//! Nothing about a parked root is in flight. An answer continues it from its
//! checkpoint through the queue (`queued_answer`), which gives it a `queued`
//! entry again and makes it a continuation the pass does drive; a cancel closes
//! it (`close_if_undriven`). A root that is `awaiting_input` but still has a
//! live entry (a dead process's claim the reaper just requeued) is not parked
//! yet, and is recovered as before: that recovery is what parks it.

use agentic_runtime::entity::run;
use sea_orm::DatabaseConnection;

/// Queue states that leave a root with no entry a worker can claim.
const ENDED_ENTRY: [&str; 4] = ["completed", "failed", "cancelled", "dead"];

/// Is this root parked: a root recovery parks at a person, waiting on one, with
/// its checkpoint on record and no entry left to run? Pure, so the rule is
/// assertable without a database.
fn is_parked(root: &run::Model, has_checkpoint: bool, entry_status: Option<&str>) -> bool {
    super::parks_at_a_person(root)
        && root.task_status.as_deref() == Some("awaiting_input")
        && has_checkpoint
        && entry_status.is_none_or(|status| ENDED_ENTRY.contains(&status))
}

async fn read_is_parked(db: &DatabaseConnection, root: &run::Model) -> Result<bool, String> {
    if !super::parks_at_a_person(root) || root.task_status.as_deref() != Some("awaiting_input") {
        return Ok(false);
    }
    let has_checkpoint = agentic_runtime::crud::get_suspension(db, &root.id)
        .await
        .map_err(|e| format!("failed to read the root's suspension: {e}"))?
        .is_some();
    let entry = agentic_runtime::crud::get_queue_entry(db, &root.id)
        .await
        .map_err(|e| format!("failed to read the root's queue entry: {e}"))?;
    Ok(is_parked(
        root,
        has_checkpoint,
        entry.as_ref().map(|e| e.queue_status.as_str()),
    ))
}

/// Drop every parked root from the startup pass's selection. Fails open per
/// root: one that cannot be read is kept, which is the pass's behaviour before
/// this check.
pub(super) async fn leave_parked_roots(
    db: &DatabaseConnection,
    roots: Vec<run::Model>,
) -> Vec<run::Model> {
    let before = roots.len();
    let mut kept = Vec::with_capacity(before);
    for root in roots {
        match read_is_parked(db, &root).await {
            Ok(true) => {}
            Ok(false) => kept.push(root),
            Err(e) => {
                tracing::warn!(
                    target: "recovery",
                    run_id = %root.id,
                    error = %e,
                    "startup pass: could not tell whether the root is parked; recovering it"
                );
                kept.push(root);
            }
        }
    }
    if kept.len() < before {
        tracing::info!(
            target: "recovery",
            count = before - kept.len(),
            "startup pass: leaving roots parked on a person as they are"
        );
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(source_type: &str, task_status: &str) -> run::Model {
        let now = agentic_runtime::crud::now();
        run::Model {
            id: "run-1".into(),
            question: "how did the stores do?".into(),
            answer: None,
            error_message: None,
            thread_id: None,
            source_type: Some(source_type.to_string()),
            metadata: None,
            parent_run_id: None,
            schedule_id: None,
            task_status: Some(task_status.to_string()),
            task_metadata: None,
            attempt: 0,
            recovery_requested_at: None,
            driver_id: None,
            driver_heartbeat_at: None,
            cancel_requested_at: None,
            workspace_id: uuid::Uuid::nil(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn an_analytics_root_waiting_with_its_entry_ended_is_parked() {
        let r = root("analytics", "awaiting_input");
        assert!(is_parked(&r, true, Some("completed")));
        assert!(is_parked(&r, true, None));
    }

    #[test]
    fn a_root_with_a_live_entry_is_not_parked_yet() {
        let r = root("analytics", "awaiting_input");
        assert!(!is_parked(&r, true, Some("queued")));
        assert!(!is_parked(&r, true, Some("claimed")));
    }

    #[test]
    fn a_root_without_its_checkpoint_is_recovered() {
        assert!(!is_parked(
            &root("analytics", "awaiting_input"),
            false,
            None
        ));
    }

    #[test]
    fn only_an_analytics_root_waiting_on_a_person_is_parked() {
        assert!(!is_parked(&root("analytics", "running"), true, None));
        assert!(!is_parked(&root("builder", "awaiting_input"), true, None));
        assert!(!is_parked(&root("workflow", "awaiting_input"), true, None));
    }
}
