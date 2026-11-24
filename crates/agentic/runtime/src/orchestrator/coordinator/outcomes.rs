//! Handlers for `WorkerMessage::Event` and `TaskOutcome` variants.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use serde_json::{Value, json};

use crate::crud;
use crate::lifecycle::state::RunStatus;

use super::policy::{CompletionAction, CompletionContext};
use super::terminal_write::{TerminalWriteTarget, write_terminal};
use super::{ChildResult, Coordinator, TaskStatus};

impl Coordinator {
    // ── Event handling ──────────────────────────────────────────────────

    pub(super) async fn handle_event(&mut self, task_id: &str, event_type: &str, payload: Value) {
        tracing::debug!(target: "coordinator", task_id, event_type, "handle_event");
        let Some(node) = self.tasks.get_mut(task_id) else {
            tracing::warn!(target: "coordinator", task_id, "event for unknown task");
            return;
        };

        let seq = node.next_seq;
        node.next_seq += 1;

        // Persist event for this task's own run.
        if let Err(e) = crud::insert_event(
            &self.db,
            &node.run_id,
            seq,
            event_type,
            &payload,
            self.attempt,
        )
        .await
        {
            tracing::error!(
                target: "coordinator",
                task_id,
                run_id = %node.run_id,
                seq,
                error = %e,
                "failed to persist event"
            );
        }
        self.state.notify(&node.run_id);

        // If this is a child task, inject a wrapped delegation_event into
        // the parent's event stream. Bubble all the way to the root so the
        // SSE stream (which reads from the root run) sees child events.
        let mut current_id = node.parent_task_id.clone();
        while let Some(ancestor_id) = current_id {
            if let Some(ancestor_node) = self.tasks.get_mut(&ancestor_id) {
                let ancestor_seq = ancestor_node.next_seq;
                ancestor_node.next_seq += 1;

                let wrapped = json!({
                    "child_task_id": task_id,
                    "inner_event_type": event_type,
                    "inner": payload,
                });
                if let Err(e) = crud::insert_event(
                    &self.db,
                    &ancestor_node.run_id,
                    ancestor_seq,
                    "delegation_event",
                    &wrapped,
                    self.attempt,
                )
                .await
                {
                    tracing::error!(
                        target: "coordinator",
                        ancestor_id = %ancestor_id,
                        run_id = %ancestor_node.run_id,
                        seq = ancestor_seq,
                        error = %e,
                        "failed to persist delegation_event"
                    );
                }
                self.state.notify(&ancestor_node.run_id);
                current_id = ancestor_node.parent_task_id.clone();
            } else {
                break;
            }
        }
    }

    // ── Outcome handling ────────────────────────────────────────────────

    pub(super) async fn handle_outcome(&mut self, task_id: &str, outcome: TaskOutcome) {
        let outcome_type = match &outcome {
            TaskOutcome::Done { .. } => "Done",
            TaskOutcome::Suspended { .. } => "Suspended",
            TaskOutcome::Failed(_) => "Failed",
            TaskOutcome::Cancelled => "Cancelled",
            TaskOutcome::Deferred { .. } => "Deferred",
        };
        tracing::debug!(target: "coordinator", task_id, outcome_type, "handle_outcome");
        match outcome {
            // Unreachable by construction: the worker converts `Deferred` into
            // `WorkerMessage::Defer` and never forwards it as an outcome. If it
            // arrives here a transport is mistranslating — say so rather than
            // silently recording a result for a task that never ran.
            TaskOutcome::Deferred {
                delay_secs, reason, ..
            } => {
                tracing::error!(
                    target: "coordinator",
                    task_id, delay_secs, %reason,
                    "coordinator received a Deferred outcome; a transport is not \
                     honouring the defer path. Ignoring — the task did not run."
                );
            }
            TaskOutcome::Done { answer, metadata } => {
                self.handle_done(task_id, answer, metadata).await;
            }
            TaskOutcome::Suspended {
                reason,
                resume_data,
                trace_id,
            } => {
                self.handle_suspended(task_id, reason, resume_data, trace_id)
                    .await;
            }
            TaskOutcome::Failed(msg) => {
                self.handle_failed(task_id, msg).await;
            }
            TaskOutcome::Cancelled => {
                self.handle_cancelled(task_id).await;
            }
        }
    }

    async fn handle_done(&mut self, task_id: &str, answer: String, metadata: Option<Value>) {
        // Ask the registered completion policy what to do. This is
        // the seam that keeps the runtime crate domain-agnostic — the
        // policy knows about `workflow_continue` / `workflow_waiting_siblings`
        // / `workflow_version_conflict` etc., the coordinator does not.
        let run_id_for_ctx = self
            .tasks
            .get(task_id)
            .map(|n| n.run_id.clone())
            .unwrap_or_default();
        let parent_for_ctx = self
            .tasks
            .get(task_id)
            .and_then(|n| n.parent_task_id.clone());
        let policy_decision = {
            let ctx = CompletionContext {
                task_id,
                run_id: &run_id_for_ctx,
                parent_task_id: parent_for_ctx.as_deref(),
                answer: &answer,
                metadata: metadata.as_ref(),
            };
            self.completion_policy.on_task_done(&ctx).await
        };

        match policy_decision {
            CompletionAction::Chain { spec } => {
                // Re-assign under the same task_id; the existing task
                // node keeps its `Running` status (the chain represents
                // continuation, not termination). The prior worker
                // already flipped the queue row to `completed`, so a
                // failed reassign here would strand the run — surface
                // that as a parent-run failure rather than swallowing.
                let Some(node) = self.tasks.get(task_id) else {
                    return;
                };
                let run_id = node.run_id.clone();
                let chain_label = chain_label(&spec);
                let assignment = TaskAssignment {
                    task_id: task_id.to_string(),
                    parent_task_id: None,
                    run_id: run_id.clone(),
                    spec,
                    policy: None,
                    claim_epoch: agentic_core::NO_CLAIM,
                };
                if let Err(e) = self.transport.assign(assignment).await {
                    self.fail_parent_on_assign_error(task_id, &run_id, chain_label, e)
                        .await;
                }
                return;
            }
            CompletionAction::Defer => {
                // Another path (sibling completion / peer CAS retry)
                // will drive the run forward. Coordinator does
                // nothing further with this outcome.
                return;
            }
            CompletionAction::Finalize => {
                // Fall through to the standard Done finalization
                // below.
            }
        }

        let Some(node) = self.tasks.get_mut(task_id) else {
            return;
        };
        node.status = TaskStatus::Done;
        node.suspended_at = None;
        let parent_id = node.parent_task_id.clone();
        let run_id = node.run_id.clone();
        let is_root = parent_id.is_none();
        tracing::info!(
            target: "coordinator",
            task_id,
            is_root,
            answer_len = answer.len(),
            "handle_done"
        );

        if let Some(parent_id) = parent_id {
            // Atomically mark child done + record outcome in one transaction.
            if let Err(e) =
                crud::complete_child_done_txn(&self.db, &run_id, task_id, &parent_id, &answer).await
            {
                tracing::error!(
                    target: "coordinator",
                    task_id,
                    run_id = %run_id,
                    parent_id = %parent_id,
                    error = %e,
                    "failed to complete child done"
                );
            }

            self.emit_delegation_completed(&parent_id, task_id, true, Some(&answer), None)
                .await;
            self.record_child_result(&parent_id, task_id, ChildResult::Done(answer))
                .await;
            self.state.statuses.insert(run_id.clone(), RunStatus::Done);
            self.state.notify(&run_id);
            self.state.deregister_notifier(&run_id);
        } else {
            // Root task done.
            match crud::update_run_done(&self.db, &run_id, &answer, metadata).await {
                // After the commit: the host's observer reads the row (and
                // whether anyone asked to hear of it) back.
                Ok(()) => crate::lifecycle::done_hook::root_run_done(&run_id).await,
                Err(e) => tracing::error!(
                    target: "coordinator",
                    task_id,
                    run_id = %run_id,
                    error = %e,
                    "failed to persist run done status"
                ),
            }
            self.state.statuses.insert(run_id.clone(), RunStatus::Done);
            self.state.notify(&run_id);
        }
    }

    async fn handle_failed(&mut self, task_id: &str, msg: String) {
        // Named for what happened, not for the function: "handle_failed" was
        // the single most frequent ERROR message in prod and said nothing a
        // reader scanning the error view could act on. The run id and the
        // kind of task are what tell a compile failure from an agent crash.
        let (run_id, task_kind, parent_task_id) = match self.tasks.get(task_id) {
            Some(node) => (
                node.run_id.as_str(),
                node.original_spec
                    .as_ref()
                    .map(super::source_type_for_spec)
                    .unwrap_or_default(),
                node.parent_task_id.as_deref().unwrap_or(""),
            ),
            None => ("", String::new(), ""),
        };
        if is_user_input_request(&msg) {
            tracing::warn!(
                target: "coordinator",
                task_id,
                run_id,
                task_kind = %task_kind,
                parent_task_id,
                error = %msg,
                "agentic task stopped to ask the user for input"
            );
        } else if is_pending_compile(&msg) {
            tracing::warn!(
                target: "coordinator",
                task_id,
                run_id,
                task_kind = %task_kind,
                parent_task_id,
                error = %msg,
                "agentic task failed while the workspace was still compiling"
            );
        } else if is_workspace_config_refusal(&msg) {
            tracing::warn!(
                target: "coordinator",
                task_id,
                run_id,
                task_kind = %task_kind,
                parent_task_id,
                error = %msg,
                "agentic task refused the workspace's configuration"
            );
        } else {
            tracing::error!(
                target: "coordinator",
                task_id,
                run_id,
                task_kind = %task_kind,
                parent_task_id,
                error = %msg,
                "agentic task failed"
            );
        }

        // Surface the raw worker failure on the parent's event stream, ahead
        // of the `delegation_completed` that `finalize_failed` emits for a
        // child: it names the spec kind and step that errored.
        self.emit_task_failed(task_id, &msg).await;

        // The coordinator does not retry: a failure that reaches it is
        // terminal. A root whose run opts in is retried beneath it, on its
        // queue row (`transport/durable/retry.rs`), and reports `Failed` here
        // only once those retries are spent.
        self.finalize_failed(task_id, msg).await;
    }

    /// Terminal failure: mark node failed and propagate to parent or root.
    async fn finalize_failed(&mut self, task_id: &str, msg: String) {
        let Some(node) = self.tasks.get_mut(task_id) else {
            return;
        };
        node.status = TaskStatus::Failed;
        node.suspended_at = None;
        let parent_id = node.parent_task_id.clone();
        let run_id = node.run_id.clone();

        if let Some(parent_id) = parent_id {
            // Atomically mark child failed + record outcome in one transaction.
            if let Err(e) = crud::complete_child_failed_txn(
                &self.db, &run_id, task_id, &parent_id, "failed", &msg,
            )
            .await
            {
                tracing::error!(
                    target: "coordinator",
                    task_id,
                    run_id = %run_id,
                    parent_id = %parent_id,
                    error = %e,
                    "failed to complete child failure"
                );
            }

            self.emit_delegation_completed(&parent_id, task_id, false, None, Some(&msg))
                .await;
            self.record_child_result(&parent_id, task_id, ChildResult::Failed(msg))
                .await;
            self.state
                .statuses
                .insert(run_id.clone(), RunStatus::Failed("child failed".into()));
            self.state.notify(&run_id);
            self.state.deregister_notifier(&run_id);
        } else {
            // Root task failed. Terminal: a failure reaching the coordinator
            // has already spent its queue retries.
            match crud::update_run_failed(&self.db, &run_id, &msg).await {
                Ok(()) => crate::lifecycle::failure_hook::root_run_failed(&run_id).await,
                Err(e) => tracing::error!(
                    target: "coordinator",
                    task_id,
                    run_id = %run_id,
                    error = %e,
                    "failed to persist run failed status"
                ),
            }
            self.state
                .statuses
                .insert(run_id.clone(), RunStatus::Failed(msg));
            self.state.notify(&run_id);
        }
    }

    /// Terminal cancellation: mark node cancelled and propagate to parent.
    ///
    /// Distinct from `handle_failed`: the user intentionally stopped the task,
    /// so it is recorded as `cancelled`, not `failed`.
    pub(super) async fn handle_cancelled(&mut self, task_id: &str) {
        tracing::info!(target: "coordinator", task_id, "handle_cancelled");

        let Some(node) = self.tasks.get_mut(task_id) else {
            return;
        };
        node.status = TaskStatus::Failed; // In-memory: reuse Failed variant
        node.suspended_at = None;
        let parent_id = node.parent_task_id.clone();
        let run_id = node.run_id.clone();

        let msg = "Operation cancelled";
        // Retried, not dropped: the node is terminal in memory already, so a
        // lost write would leave the row live with nobody driving it.
        let db = &self.db;
        let target = TerminalWriteTarget {
            what: "cancelled",
            task_id,
            run_id: &run_id,
        };
        if let Some(parent_id) = parent_id {
            // Atomically mark child cancelled + record outcome in one transaction.
            write_terminal(target, || {
                crud::complete_child_failed_txn(db, &run_id, task_id, &parent_id, "cancelled", msg)
            })
            .await;

            self.emit_delegation_completed(&parent_id, task_id, false, None, Some(msg))
                .await;
            self.record_child_result(&parent_id, task_id, ChildResult::Failed(msg.into()))
                .await;
            self.state
                .statuses
                .insert(run_id.clone(), RunStatus::Cancelled);
            self.state.notify(&run_id);
            self.state.deregister_notifier(&run_id);
        } else {
            // Root task cancelled — single write.
            write_terminal(target, || {
                crud::transition_run(db, &run_id, "cancelled", None, None, Some(msg))
            })
            .await;
            self.state
                .statuses
                .insert(run_id.clone(), RunStatus::Cancelled);
            self.state.notify(&run_id);
        }
    }
}

/// Short human-readable label for a chained spec, used only in
/// `fail_parent_on_assign_error` logging when reassignment fails.
/// Kept here so the policy trait doesn't need to surface
/// observability strings.
fn chain_label(spec: &TaskSpec) -> &'static str {
    match spec {
        TaskSpec::Agent { .. } => "Agent (chain)",
        TaskSpec::Automation { .. } => "Automation (chain)",
        TaskSpec::AutomationDecision { .. } => "AutomationDecision (chain)",
        TaskSpec::AutomationStep { .. } => "AutomationStep (chain)",
        TaskSpec::Resume { .. } => "Resume (chain)",
        TaskSpec::Custom { .. } => "Custom (chain)",
        TaskSpec::Airway { .. } => "Airway (chain)",
        TaskSpec::Compile { .. } => "Compile (chain)",
    }
}

/// Is this failure the solver stopping to ask the user a question?
///
/// The analytics solver reports `NeedsUserInput` as a fatal failure — the
/// runtime receives `fatal: NeedsUserInput { prompt: .. }` from
/// `format!("fatal: {e:?}")` and, being domain-agnostic, only that string. It
/// is an expected outcome rather than a fault, and its prompt can quote the
/// customer's question, so it is logged at WARN: kept in the log store, out of
/// Sentry's error events. It reached prod's Sentry as `agentic task failed` on
/// 2026-09-15.
fn is_user_input_request(msg: &str) -> bool {
    msg.contains("NeedsUserInput {")
}

/// Is this failure a stateless replica answering "not compiled yet"?
///
/// `ScanUnavailable` (`oxy_workspace_runtime::scan_source`) states this condition as
/// explicitly retryable — every variant of its message ends "a (re)compile has
/// been enqueued" — and that queued compile clears it with nobody touching
/// anything. `drivers::recovery` already logs the very same `ScanUnavailable` at
/// WARN for the health eval; the coordinator was the one place that called it an
/// error.
///
/// Only the level changes. The run still failed, so `emit_task_failed` and the
/// failure propagation below are untouched — an outcome that heals itself is
/// just not one to page on.
///
/// It was the loudest ERROR in production by a wide margin: 993 events in the
/// week to 2026-09-21, ~4x the next distinct fault, and a single workspace
/// retrying produced nearly all of them.
fn is_pending_compile(msg: &str) -> bool {
    msg.contains("a (re)compile has been enqueued")
}

/// Is this failure a commit compile refusing something only the workspace's
/// owner can change?
///
/// `CompileGitError::task_failure` (`oxy-compile-runtime`) leads every commit
/// compile failure with `compile from git failed [<code>]: `. The codes below
/// name the repository's own content — a DuckDB file database a cloud
/// deployment no longer serves, a missing `config.yml`, a commit that was
/// force-pushed away — so nobody here can act on them, and `fetch_for_task`
/// has already logged each one at WARN with its `workspace_id` and `code`.
/// The coordinator logged the same fact a second time at ERROR, without the
/// workspace id: `duckdb_file_database` alone was 391 of production's
/// `agentic task failed` events in the week to 2026-11-23.
///
/// Codes that can be ours stay ERROR: `io`, `bad_archive`, `database`,
/// `not_a_commit`, `duckdb_mirror_unconfigured` (our own
/// `OXY_COMPILE_BLOB_S3_BUCKET` is unset, not the customer's repository), and
/// every retryable one — a retryable failure carries `, retryable` inside the
/// brackets, so the exact `[<code>]` match below cannot catch one.
///
/// Matched as text because `agentic-runtime` may not depend on
/// `oxy-compile-runtime` (it depends on `agentic-core` only), the same way
/// `is_pending_compile` matches `ScanUnavailable`. The strings are pinned by
/// the tests below against `CompileGitError::code`.
fn is_workspace_config_refusal(msg: &str) -> bool {
    const CODES: [&str; 7] = [
        "duckdb_file_database",
        "duckdb_data_missing",
        "duckdb_path_outside_tree",
        "no_config",
        "subdir_missing",
        "commit_not_found",
        "tree_too_large",
    ];
    CODES.iter().any(|code| {
        msg.strip_prefix("compile from git failed [")
            .and_then(|rest| rest.strip_prefix(*code))
            .is_some_and(|rest| rest.starts_with("]: "))
    })
}

#[cfg(test)]
mod tests {
    use super::{is_pending_compile, is_user_input_request, is_workspace_config_refusal};

    #[test]
    fn a_needs_user_input_failure_is_recognised() {
        assert!(is_user_input_request(
            r#"fatal: NeedsUserInput { prompt: "Which location did you mean?" }"#
        ));
    }

    #[test]
    fn other_failures_are_not() {
        assert!(!is_user_input_request(r#"fatal: LlmError("rate limited")"#));
        assert!(!is_user_input_request("worker panicked"));
    }

    /// Both `ScanUnavailable` variants, verbatim from `semantic.rs` — the
    /// working-copy-refused one wraps the marker across source lines there, so
    /// only the built string proves the match holds.
    #[test]
    fn a_pending_compile_is_recognised() {
        assert!(is_pending_compile(
            "workspace 0effae8f-c261-4cd3-afb5-865867b8a999 has no compiled semantic model \
             available on this stateless replica; a (re)compile has been enqueued — retry shortly"
        ));
        assert!(is_pending_compile(
            "workspace 0effae8f-c261-4cd3-afb5-865867b8a999 has no compiled semantic model; \
             this check reads the promoted revision only and will not fall back to a working \
             copy — a (re)compile has been enqueued"
        ));
    }

    #[test]
    fn a_real_compile_failure_is_not_a_pending_compile() {
        assert!(!is_pending_compile(
            r#"fatal: CompileError("models/orders.view.yml: missing field `dimensions`")"#
        ));
        assert!(!is_pending_compile("worker panicked"));
    }

    /// The message production reported, verbatim from Sentry OXY-2G, and the
    /// shape `CompileGitError::task_failure` builds for the rest of the codes.
    #[test]
    fn a_workspace_config_refusal_is_recognised() {
        assert!(is_workspace_config_refusal(
            "compile from git failed [duckdb_file_database]: DuckDB database \"gallant\" is a \
             file database (`dataset:` or `path:`), which a cloud deployment no longer \
             supports: its data is files in the repository. Move the data to the workspace's \
             Airhouse and declare the database as `type: airhouse_managed` under the same name"
        ));
        assert!(is_workspace_config_refusal(
            "compile from git failed [no_config]: the commit has no config.yml at the \
             workspace root; if the workspace lives in a subdirectory of its repository, \
             workspaces.repo_subdir is not recorded yet"
        ));
        assert!(is_workspace_config_refusal(
            "compile from git failed [commit_not_found]: commit not found"
        ));
        assert!(is_workspace_config_refusal(
            "compile from git failed [tree_too_large]: the commit unpacks to more than 20000 \
             files"
        ));
    }

    /// The codes that can be our fault, and the retryable form: a retryable
    /// failure appends `, retryable` inside the brackets
    /// (`CompileGitError::RETRYABLE_MARKER`), so no exact `[<code>]` matches.
    #[test]
    fn a_failure_that_can_be_ours_is_not_a_config_refusal() {
        assert!(!is_workspace_config_refusal(
            "compile from git failed [io]: could not prepare a directory for the fetched commit"
        ));
        assert!(!is_workspace_config_refusal(
            "compile from git failed [database]: database error: pool timed out"
        ));
        assert!(!is_workspace_config_refusal(
            "compile from git failed [duckdb_mirror_unconfigured]: DuckDB database \"gallant\" \
             is a local file and no S3 mirror bucket is configured"
        ));
        assert!(!is_workspace_config_refusal(
            "compile from git failed [github_unavailable, retryable]: GitHub is unavailable"
        ));
        // Not a commit compile at all.
        assert!(!is_workspace_config_refusal("worker panicked"));
        assert!(!is_workspace_config_refusal(
            r#"fatal: CompileError("models/orders.view.yml: missing field `dimensions`")"#
        ));
    }

    /// A code that only starts with a listed one, and a listed code quoted
    /// mid-message rather than leading it, are both ERROR: the match is the
    /// whole bracketed code at the front of the message.
    #[test]
    fn a_config_refusal_matches_the_whole_leading_code_only() {
        assert!(!is_workspace_config_refusal(
            "compile from git failed [no_config_whatsoever]: invented"
        ));
        assert!(!is_workspace_config_refusal(
            "worker panicked while handling compile from git failed [no_config]: ..."
        ));
    }
}
