//! Handlers for `TaskOutcome::Suspended` and human answers.

use std::collections::HashMap;

use agentic_core::delegation::{FanoutFailurePolicy, SuspendReason, TaskAssignment};
use serde_json::json;

use crate::crud;
use crate::lifecycle::state::RunStatus;

use super::{ChildResult, Coordinator, TaskNode, TaskStatus, source_type_for_spec};

impl Coordinator {
    pub(super) async fn handle_suspended(
        &mut self,
        task_id: &str,
        reason: SuspendReason,
        resume_data: agentic_core::human_input::SuspendedRunData,
        _trace_id: String,
    ) {
        // Idempotency: a task already waiting on children is being asked to
        // delegate *again*, without an intervening resume (`resume_parent` puts
        // it back to `Running` before re-assigning). That means something
        // re-drove this task while its first delegation was still outstanding —
        // a re-claimed queue row, a second driver, crash recovery re-running the
        // decider. Honouring it would mint a fresh `{task_id}.{n}` child, orphan
        // the live one (`WaitingOnChildren` is overwritten wholesale below, not
        // appended to), and run the same step a second time concurrently.
        //
        // Dropping the duplicate is safe: the original child is still running and
        // its completion still resumes this parent. The `Suspended` outcome that
        // brought us here writes nothing to the queue row either way.
        if matches!(
            reason,
            SuspendReason::Delegation { .. } | SuspendReason::ParallelDelegation { .. }
        ) && let Some(node) = self.tasks.get(task_id)
            && let TaskStatus::WaitingOnChildren { child_task_ids, .. } = &node.status
        {
            tracing::warn!(
                target: "coordinator",
                task_id,
                existing_children = ?child_task_ids,
                "duplicate delegation for a task already waiting on children; \
                 ignoring re-delegation rather than spawning another child"
            );
            return;
        }

        // Store suspend data (drop mutable borrow before calling other methods).
        {
            // A suspension for a task this coordinator has never heard of. The
            // outcome has nowhere to go — nothing will ever resume it — and it
            // must not be dropped in silence, because the row is still
            // `claimed` with a heartbeat ticker deliberately kept alive. Every
            // backstop is disarmed in that state: the reaper skips a fresh
            // heartbeat, `find_stuck_runs` requires no active queue entry, and
            // `check_suspend_timeouts` reads `suspended_at`, which is set two
            // lines below and so never gets written. It would sit claimed until
            // the process restarts.
            //
            // Fail the row instead: terminal, visible in the dead-letter admin
            // table, and the write itself stops the ticker (the heartbeat
            // predicate needs `queue_status = 'claimed'`).
            //
            // Pathological by construction rather than transient — `claim_task`
            // only lets ROOT tasks be claimed globally precisely so an outcome
            // can't be routed to a coordinator that doesn't own the task.
            let Some(node) = self.tasks.get_mut(task_id) else {
                // `rule = "orphaned-claim"` pairs with the drivers'
                // `rule = "dropped-outcome"`: the two halves of "an outcome went
                // nowhere" are documented together in `worker-fleet.md`, so they
                // should be filterable together too, across the differing
                // tracing targets they legitimately use.
                tracing::error!(
                    target: "coordinator",
                    rule = "orphaned-claim",
                    task_id,
                    ?reason,
                    "suspension for a task absent from this coordinator's map; \
                     nothing can resume it — failing the queue row so it cannot \
                     sit claimed on a live heartbeat"
                );
                if let Err(e) = crud::fail_orphaned_claim(&self.db, task_id).await {
                    tracing::error!(
                        target: "coordinator",
                        rule = "orphaned-claim",
                        task_id,
                        error = %e,
                        "failed to fail the orphaned queue row"
                    );
                }
                return;
            };
            node.suspend_data = Some(resume_data.clone());
            node.suspended_at = Some(tokio::time::Instant::now());
        }

        match reason {
            SuspendReason::HumanInput { questions } => {
                tracing::info!(
                    target: "coordinator",
                    task_id,
                    question_count = questions.len(),
                    "suspended for human input"
                );
                let (run_id, is_root) = {
                    let node = self.tasks.get_mut(task_id).unwrap();
                    node.status = TaskStatus::SuspendedHuman;
                    (node.run_id.clone(), node.parent_task_id.is_none())
                };

                // Persist status + suspension data atomically in one transaction.
                let combined_prompt = questions
                    .iter()
                    .map(|q| q.prompt.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let first_suggestions = questions
                    .first()
                    .map(|q| q.suggestions.clone())
                    .unwrap_or_default();

                match crud::suspend_with_data_txn(
                    &self.db,
                    &run_id,
                    "awaiting_input",
                    None,
                    &combined_prompt,
                    &first_suggestions,
                    &resume_data,
                )
                .await
                {
                    // After the commit: the host's observer reads the run, its
                    // suspension and whether anyone asked to hear, back.
                    Ok(()) => {
                        self.unrecorded_suspensions.remove(task_id);
                        if is_root {
                            crate::lifecycle::suspend_hook::root_run_suspended(&run_id).await
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "coordinator",
                            task_id,
                            run_id = %run_id,
                            error = %e,
                            "failed to persist suspension"
                        );
                        self.unrecorded_suspensions.insert(task_id.to_string());
                    }
                }
                self.state.statuses.insert(
                    run_id,
                    RunStatus::Suspended {
                        questions: questions.clone(),
                    },
                );
            }

            SuspendReason::Delegation {
                target,
                request,
                context,
                policy,
            } => {
                self.child_counter += 1;
                let child_id = format!("{task_id}.{}", self.child_counter);
                tracing::info!(
                    target: "coordinator",
                    task_id,
                    child_id = %child_id,
                    target = ?target,
                    "suspended for delegation, spawning child task"
                );
                let child_run_id = child_id.clone();
                let task_id_owned = task_id.to_string();

                // Emit delegation_started on parent's stream.
                self.emit_delegation_started(&task_id_owned, &child_id, &target, &request)
                    .await;

                // Update parent status (after emit to avoid borrow conflict).
                let run_id = {
                    let node = self.tasks.get_mut(task_id).unwrap();
                    node.status = TaskStatus::WaitingOnChildren {
                        child_task_ids: vec![child_id.clone()],
                        completed: HashMap::new(),
                        failure_policy: FanoutFailurePolicy::FailFast,
                    };
                    node.run_id.clone()
                };

                self.emit_waiting_on_children(
                    &task_id_owned,
                    std::slice::from_ref(&child_id),
                    &FanoutFailurePolicy::FailFast,
                )
                .await;

                // Persist status + suspension data atomically. Use "delegating"
                // (not "awaiting_input") so crash recovery correctly identifies
                // this as WaitingOnChildren via from_db().
                if let Err(e) = crud::suspend_with_data_txn(
                    &self.db,
                    &run_id,
                    "delegating",
                    Some(json!({
                        "child_task_ids": [&child_id],
                        "completed": {},
                        "failure_policy": "fail_fast",
                    })),
                    &format!("Delegation to {request}"),
                    &[],
                    &resume_data,
                )
                .await
                {
                    tracing::error!(
                        target: "coordinator",
                        task_id,
                        run_id = %run_id,
                        error = %e,
                        "failed to persist delegation state"
                    );
                }
                // Save request before it's moved into child_spec.
                let request_for_child = request.clone();
                let run_id_for_child = run_id.clone();

                self.state
                    .statuses
                    .insert(run_id, RunStatus::Suspended { questions: vec![] });

                let child_spec = self.delegation_resolver.resolve(target, request, context);

                // Register child in task tree (store spec + policy for retries).
                // Single-child delegation is never a loop iteration —
                // loops always fan out via the parallel path — so
                // loop_iteration stays None here.
                self.tasks.insert(
                    child_id.clone(),
                    TaskNode {
                        run_id: child_run_id.clone(),
                        parent_task_id: Some(task_id_owned.clone()),
                        status: TaskStatus::Running,
                        suspend_data: None,
                        next_seq: 0,
                        suspended_at: None,
                        original_spec: Some(child_spec.clone()),
                        policy,
                        attempt: 0,
                        fallback_index: 0,
                        loop_iteration: None,
                    },
                );

                // Derive source_type from the child's TaskSpec.
                let child_source_type = source_type_for_spec(&child_spec);

                // Build task_metadata with policy/spec for restart recovery.
                let child_task_metadata = {
                    let node = self.tasks.get(&child_id);
                    let policy_json = node
                        .and_then(|n| n.policy.as_ref())
                        .and_then(|p| serde_json::to_value(p).ok());
                    let spec_json = node
                        .and_then(|n| n.original_spec.as_ref())
                        .and_then(|s| serde_json::to_value(s).ok());
                    if policy_json.is_some() || spec_json.is_some() {
                        Some(json!({
                            "policy": policy_json,
                            "original_spec": spec_json,
                        }))
                    } else {
                        None
                    }
                };

                // Persist child run with parent reference + metadata in one INSERT.
                if let Err(e) = crud::insert_child_run(
                    &self.db,
                    &child_run_id,
                    &run_id_for_child,
                    &request_for_child,
                    &child_source_type,
                    self.attempt,
                    child_task_metadata,
                )
                .await
                {
                    tracing::error!(
                        target: "coordinator",
                        task_id,
                        child_run_id = %child_run_id,
                        error = %e,
                        "failed to persist child run"
                    );
                }

                // Register a notifier so SSE subscribers can stream child events.
                self.state.register_notifier(&child_run_id);

                // Assign child to a worker.
                let assignment = TaskAssignment {
                    task_id: child_id.clone(),
                    parent_task_id: Some(task_id_owned.clone()),
                    run_id: child_run_id,
                    spec: child_spec,
                    policy: None,
                    claim_epoch: agentic_core::NO_CLAIM,
                };

                tracing::info!(target: "coordinator", task_id, child_task_id = %assignment.task_id, "assigning child task to worker");
                if let Err(e) = self.transport.assign(assignment).await {
                    tracing::error!(target: "coordinator", task_id, error = %e, "failed to assign child task, failing it");
                    // Mark the child as failed and resume the parent so it
                    // doesn't hang forever waiting on a child that never started.
                    if let Some(child_node) = self.tasks.get_mut(&child_id) {
                        child_node.status = TaskStatus::Failed;
                    }
                    self.emit_delegation_completed(
                        &task_id_owned,
                        &child_id,
                        false,
                        None,
                        Some(&format!("failed to assign child task: {e}")),
                    )
                    .await;
                    self.resume_parent(
                        &task_id_owned,
                        format!("Delegation failed: could not assign child task: {e}"),
                        "failed",
                    )
                    .await;
                } else {
                    tracing::info!(target: "coordinator", task_id, "child task assigned successfully");
                }
            }

            SuspendReason::ParallelDelegation {
                targets,
                failure_policy,
            } => {
                let task_id_owned = task_id.to_string();
                let child_count = targets.len();
                tracing::info!(
                    target: "coordinator",
                    task_id,
                    child_count,
                    "suspended for parallel delegation, spawning children"
                );

                // Generate child IDs and specs.
                let mut child_ids = Vec::with_capacity(child_count);
                let mut child_specs = Vec::with_capacity(child_count);
                let mut child_requests = Vec::with_capacity(child_count);

                for item in &targets {
                    self.child_counter += 1;
                    let child_id = format!("{task_id}.{}", self.child_counter);
                    child_ids.push(child_id);

                    // Clone target/request/context because we still
                    // need `item.target` and `item.request` for the
                    // `emit_delegation_started` loop below.
                    let spec = self.delegation_resolver.resolve(
                        item.target.clone(),
                        item.request.clone(),
                        item.context.clone(),
                    );
                    child_specs.push(spec);
                    child_requests.push(item.request.clone());
                }

                // Emit delegation_started for each child on parent's stream.
                for (i, item) in targets.iter().enumerate() {
                    self.emit_delegation_started(
                        &task_id_owned,
                        &child_ids[i],
                        &item.target,
                        &item.request,
                    )
                    .await;
                }

                // Update parent status.
                let run_id = {
                    let node = self.tasks.get_mut(task_id).unwrap();
                    node.status = TaskStatus::WaitingOnChildren {
                        child_task_ids: child_ids.clone(),
                        completed: HashMap::new(),
                        failure_policy: failure_policy.clone(),
                    };
                    node.run_id.clone()
                };

                self.emit_waiting_on_children(&task_id_owned, &child_ids, &failure_policy)
                    .await;

                // Persist parent status + suspension data atomically.
                let child_ids_json: Vec<&str> = child_ids.iter().map(|s| s.as_str()).collect();
                if let Err(e) = crud::suspend_with_data_txn(
                    &self.db,
                    &run_id,
                    "delegating",
                    Some(json!({
                        "child_task_ids": child_ids_json,
                        "completed": {},
                        "failure_policy": serde_json::to_value(&failure_policy).unwrap_or_default(),
                    })),
                    "Parallel delegation",
                    &[],
                    &resume_data,
                )
                .await
                {
                    tracing::error!(
                        target: "coordinator",
                        task_id,
                        run_id = %run_id,
                        error = %e,
                        "failed to persist parallel delegation state"
                    );
                }

                self.state
                    .statuses
                    .insert(run_id.clone(), RunStatus::Suspended { questions: vec![] });

                // Register and assign all children.
                for (i, child_id) in child_ids.iter().enumerate() {
                    // Loop-iteration metadata travels through
                    // DelegationItem.context — see step_decider's
                    // loop fan-out where `loop_step_name` and
                    // `loop_index` are stamped. We extract here so
                    // the coordinator can emit per-iteration events
                    // as children land. Returns `None` for non-loop
                    // fan-outs (consistency runs, etc.).
                    let loop_iteration = (|| {
                        let ctx = &targets.get(i)?.context;
                        let step_name = ctx.get("loop_step_name")?.as_str()?.to_string();
                        let index = ctx.get("loop_index")?.as_u64()? as usize;
                        Some(super::LoopIterationMeta { step_name, index })
                    })();
                    self.tasks.insert(
                        child_id.clone(),
                        TaskNode {
                            run_id: child_id.clone(),
                            parent_task_id: Some(task_id_owned.clone()),
                            status: TaskStatus::Running,
                            suspend_data: None,
                            next_seq: 0,
                            suspended_at: None,
                            original_spec: Some(child_specs[i].clone()),
                            policy: None,
                            attempt: 0,
                            fallback_index: 0,
                            loop_iteration,
                        },
                    );

                    // Persist child run with parent reference + spec for recovery.
                    let child_source_type = source_type_for_spec(&child_specs[i]);
                    let child_spec_json = serde_json::to_value(&child_specs[i]).ok();
                    let child_meta = child_spec_json.map(|s| json!({ "original_spec": s }));
                    if let Err(e) = crud::insert_child_run(
                        &self.db,
                        child_id,
                        &run_id,
                        &child_requests[i],
                        &child_source_type,
                        self.attempt,
                        child_meta,
                    )
                    .await
                    {
                        tracing::error!(
                            target: "coordinator",
                            task_id,
                            child_id,
                            error = %e,
                            "failed to persist child run"
                        );
                    }

                    // Register a notifier so SSE subscribers can stream child events.
                    self.state.register_notifier(child_id);

                    let assignment = TaskAssignment {
                        task_id: child_id.clone(),
                        parent_task_id: Some(task_id_owned.clone()),
                        run_id: child_id.clone(),
                        spec: child_specs[i].clone(),
                        policy: None,
                        claim_epoch: agentic_core::NO_CLAIM,
                    };

                    if let Err(e) = self.transport.assign(assignment).await {
                        tracing::error!(
                            target: "coordinator",
                            task_id,
                            child_id,
                            error = %e,
                            "failed to assign child task"
                        );
                        // Mark this child as failed immediately.
                        if let Some(child_node) = self.tasks.get_mut(child_id) {
                            child_node.status = TaskStatus::Failed;
                        }
                        self.emit_delegation_completed(
                            &task_id_owned,
                            child_id,
                            false,
                            None,
                            Some(&format!("failed to assign: {e}")),
                        )
                        .await;
                        self.record_child_result(
                            &task_id_owned,
                            child_id,
                            ChildResult::Failed(format!("failed to assign: {e}")),
                        )
                        .await;
                    }
                }
            }
        }
    }

    pub(super) async fn handle_human_answer(&mut self, task_id: &str, answer: &str) {
        let Some(node) = self.tasks.get(task_id) else {
            tracing::warn!(target: "coordinator", task_id, "human answer for unknown task");
            return;
        };

        match &node.status {
            TaskStatus::SuspendedHuman => {
                tracing::info!(target: "coordinator", task_id, "human answer: direct answer");
                self.unrecorded_suspensions.remove(task_id);
                // resume_parent handles: status update, input_resolved
                // event, DB update, and TaskSpec::Resume assignment.
                // HITL answers are treated as a successful "child" so the
                // automation decider can fold them in normally.
                self.resume_parent(task_id, answer.to_string(), "done")
                    .await;
            }
            TaskStatus::WaitingOnChildren { child_task_ids, .. } => {
                let child_ids = child_task_ids.clone();
                tracing::info!(
                    target: "coordinator",
                    task_id,
                    child_count = child_ids.len(),
                    "human answer: override (cancelling all children)"
                );

                // Human override — cancel all children and resume parent.
                for child_id in &child_ids {
                    self.transport.cancel(child_id).await.ok();
                    if let Some(child_node) = self.tasks.get_mut(child_id) {
                        child_node.status = TaskStatus::Failed;
                        child_node.suspended_at = None;
                    }
                    self.emit_delegation_completed(
                        task_id,
                        child_id,
                        false,
                        None,
                        Some("overridden by human answer"),
                    )
                    .await;
                }

                // Human-override resume: the human's answer replaces the
                // children entirely, so for the decider this looks like a
                // successful single-child completion.
                self.resume_parent(task_id, answer.to_string(), "done")
                    .await;
            }
            _ => {
                tracing::warn!(target: "coordinator", task_id, "human answer for non-suspended task");
            }
        }
    }
}

// `translate_automation_delegation` moved to
// `agentic_automation::AutomationDelegationResolver` — the routing
// between `TaskSpec::Automation` (loop iteration body), `AutomationStep`,
// and `Automation` (on-disk YAML) is automation-domain knowledge and
// doesn't belong in the generic coordinator. Coordinator now calls
// `self.delegation_resolver.resolve(...)`.
