//! Coordinator event emitters and task-status persistence.

use agentic_core::delegation::{DelegationTarget, FanoutFailurePolicy, TaskSpec};
use serde_json::{Value, json};

use crate::crud;

use super::Coordinator;

impl Coordinator {
    /// Emit a `task_failed` event capturing the raw worker failure on
    /// the parent's stream — every worker `Failed` outcome generates one.
    ///
    /// Admins use this to attribute a run's failure to the specific
    /// `TaskSpec` that errored: the `delegation_completed { success: false }`
    /// that follows for a child carries only an opaque error string.
    pub(super) async fn emit_task_failed(&mut self, task_id: &str, error: &str) {
        let (parent_id, spec_kind, step_name) = {
            let Some(node) = self.tasks.get(task_id) else {
                return;
            };
            (
                node.parent_task_id.clone(),
                node.original_spec.as_ref().map(super::source_type_for_spec),
                node.original_spec.as_ref().and_then(automation_step_name),
            )
        };

        let target_id = parent_id.clone().unwrap_or_else(|| task_id.to_string());
        let Some(target_node) = self.tasks.get_mut(&target_id) else {
            return;
        };
        let seq = target_node.next_seq;
        target_node.next_seq += 1;
        let run_id = target_node.run_id.clone();
        let payload = task_failed_payload(task_id, spec_kind, step_name, error);
        if let Err(e) = crud::insert_event(
            &self.db,
            &run_id,
            seq,
            "task_failed",
            &payload,
            self.attempt,
        )
        .await
        {
            tracing::error!(
                target: "coordinator",
                task_id,
                run_id = %run_id,
                error = %e,
                "failed to persist task_failed event"
            );
        }
        self.state.notify(&run_id);
    }

    /// Emit a `waiting_on_children` event when a parent task transitions
    /// to `TaskStatus::WaitingOnChildren` after fanning out one or more
    /// children. Fires once per transition (so a parallel-delegation
    /// step emits one event for the whole fan-out, not N).
    ///
    /// Pairs with the per-child `delegation_started` events: those tell
    /// you the individual edges (parent→child), this one tells you the
    /// parent's overall state with the full child list and the failure
    /// policy that will decide whether one child's failure aborts the
    /// fan-out or just gets recorded. Useful for an admin tree view
    /// that wants to render a single "waiting on children" boundary
    /// instead of inferring it from the cluster of delegation_started
    /// events.
    pub(super) async fn emit_waiting_on_children(
        &mut self,
        parent_id: &str,
        child_task_ids: &[String],
        failure_policy: &FanoutFailurePolicy,
    ) {
        if let Some(node) = self.tasks.get_mut(parent_id) {
            let seq = node.next_seq;
            node.next_seq += 1;
            let run_id = node.run_id.clone();
            let payload = json!({
                "parent_task_id": parent_id,
                "child_task_ids": child_task_ids,
                "failure_policy": serde_json::to_value(failure_policy).unwrap_or(Value::Null),
            });
            if let Err(e) = crud::insert_event(
                &self.db,
                &run_id,
                seq,
                "waiting_on_children",
                &payload,
                self.attempt,
            )
            .await
            {
                tracing::error!(
                    target: "coordinator",
                    parent_id,
                    run_id = %run_id,
                    error = %e,
                    "failed to persist waiting_on_children event"
                );
            }
            self.state.notify(&run_id);
        }
    }

    // ── Task tree persistence ────────────────────────────────────────────

    /// Persist a task_status transition to the database (best-effort).
    pub(super) async fn persist_task_status(
        &self,
        run_id: &str,
        task_status: &str,
        task_metadata: Option<Value>,
    ) {
        if let Err(e) = crud::update_task_status(&self.db, run_id, task_status, task_metadata).await
        {
            tracing::error!(
                target: "coordinator",
                run_id,
                task_status,
                error = %e,
                "failed to persist task_status"
            );
        }
    }

    // ── Event emission helpers ──────────────────────────────────────────

    pub(super) async fn emit_delegation_started(
        &mut self,
        parent_id: &str,
        child_id: &str,
        target: &DelegationTarget,
        request: &str,
    ) {
        let target_str = match target {
            DelegationTarget::Agent { agent_id } => format!("agent:{agent_id}"),
            DelegationTarget::Automation { workflow_ref } => format!("workflow:{workflow_ref}"),
        };

        if let Some(node) = self.tasks.get_mut(parent_id) {
            let seq = node.next_seq;
            node.next_seq += 1;
            let payload = json!({
                "event_type": "delegation_started",
                "child_task_id": child_id,
                "target": target_str,
                "request": request,
            });
            if let Err(e) = crud::insert_event(
                &self.db,
                &node.run_id,
                seq,
                "delegation_started",
                &payload,
                self.attempt,
            )
            .await
            {
                tracing::error!(
                    target: "coordinator",
                    parent_id,
                    run_id = %node.run_id,
                    error = %e,
                    "failed to persist delegation_started event"
                );
            }
            self.state.notify(&node.run_id);
        }
    }

    pub(super) async fn emit_delegation_completed(
        &mut self,
        parent_id: &str,
        child_id: &str,
        success: bool,
        answer: Option<&str>,
        error: Option<&str>,
    ) {
        if let Some(node) = self.tasks.get_mut(parent_id) {
            let seq = node.next_seq;
            node.next_seq += 1;
            let payload = json!({
                "event_type": "delegation_completed",
                "child_task_id": child_id,
                "success": success,
                "answer": answer,
                "error": error,
            });
            if let Err(e) = crud::insert_event(
                &self.db,
                &node.run_id,
                seq,
                "delegation_completed",
                &payload,
                self.attempt,
            )
            .await
            {
                tracing::error!(
                    target: "coordinator",
                    parent_id,
                    run_id = %node.run_id,
                    error = %e,
                    "failed to persist delegation_completed event"
                );
            }
            self.state.notify(&node.run_id);
        }
    }
}

/// The `task_failed` payload. `ts` is its emit time, in the field and format
/// (`Utc::now().to_rfc3339()`) the airway worker stamps on its own events: an
/// airway run that fails before its worker emits anything (secrets, connector
/// or destination resolution) has this as its only failure event, and the run
/// timeline reads `ts` to place it.
///
/// `attempt` is always 0 and stays only because the web app's `task_failed`
/// type declares it: it counted the coordinator's per-node retries, which no
/// longer exist. A queue-level retry never reaches the coordinator as a
/// failure, so there is no other attempt to number here.
fn task_failed_payload(
    task_id: &str,
    spec_kind: Option<String>,
    step_name: Option<String>,
    error: &str,
) -> Value {
    json!({
        "task_id": task_id,
        "attempt": 0,
        "spec_kind": spec_kind,
        "step_name": step_name,
        "error": error,
        "ts": chrono::Utc::now().to_rfc3339(),
    })
}

/// Pull `step_config["name"]` off a `TaskSpec::AutomationStep` so
/// `task_failed` can carry the failing step's name. Returns `None` for
/// every other spec variant — there's no single step name for
/// `TaskSpec::Agent` (consistency_run > 1 fans out), `TaskSpec::Automation`
/// (a whole sub-automation), or `TaskSpec::AutomationDecision` (a decider
/// pass, which has no "current step" until the decider runs).
fn automation_step_name(spec: &TaskSpec) -> Option<String> {
    if let TaskSpec::AutomationStep { step_config, .. } = spec {
        step_config
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An airway run that fails before its worker emits anything has
    /// `task_failed` as its only failure event, and the run timeline reads
    /// `ts` off it, as it does off the worker's own events.
    #[test]
    fn task_failed_is_stamped_with_its_emit_time() {
        let payload =
            task_failed_payload("t-1", Some("airway".to_string()), None, "secret not found");
        assert_eq!(payload["task_id"], json!("t-1"));
        assert_eq!(payload["attempt"], json!(0));
        assert_eq!(payload["spec_kind"], json!("airway"));
        assert_eq!(payload["step_name"], Value::Null);
        assert_eq!(payload["error"], json!("secret not found"));
        let ts = payload["ts"]
            .as_str()
            .expect("payload carries a `ts` string");
        chrono::DateTime::parse_from_rfc3339(ts).expect("`ts` is RFC 3339");
    }
}
