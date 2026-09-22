//! Automation-related executor methods + the automation decision runner.

use std::collections::HashSet;
use std::sync::Arc;

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use agentic_core::hub_task::spawn_with_hub;
use agentic_runtime::worker::ExecutingTask;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::PipelineTaskExecutor;

impl PipelineTaskExecutor {
    pub(super) async fn execute_automation_step(
        &self,
        step_config: Value,
        render_context: Value,
        automation_context: Value,
    ) -> Result<ExecutingTask, String> {
        let (event_tx, event_rx) = mpsc::channel::<(String, Value)>(256);
        let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(4);
        let cancel = CancellationToken::new();

        let workspace: Arc<dyn agentic_automation::WorkspaceContext> = self.platform.clone();
        spawn_with_hub(async move {
            let result = agentic_automation::run_automation_step(
                workspace.as_ref(),
                step_config,
                render_context,
                automation_context,
            )
            .await;
            match result {
                Ok(output) => {
                    let _ = outcome_tx
                        .send(TaskOutcome::Done {
                            answer: output,
                            metadata: None,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = outcome_tx.send(TaskOutcome::Failed(e)).await;
                }
            }
            drop(event_tx);
        });

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }

    /// Seed an automation run and chain to the first `AutomationDecision`.
    ///
    /// This is the entry point for `TaskSpec::Automation`. It:
    /// 1. Loads and parses the automation YAML.
    /// 2. Inserts `agentic_workflow_state` into the DB.
    /// 3. Returns `Done { workflow_continue: true }` immediately.
    ///
    /// The coordinator's `handle_done` detects `workflow_continue` and
    /// enqueues `TaskSpec::AutomationDecision` — the stateless decider then
    /// drives the automation from DB state. No long-lived channels survive a crash.
    pub(super) async fn execute_automation(
        &self,
        run_id: &str,
        workflow_ref: &str,
        variables: Option<Value>,
        retry_from_run_id: Option<String>,
        cache_enabled: bool,
        body: Option<Value>,
        initial_render_context: Option<Value>,
    ) -> Result<ExecutingTask, String> {
        let workspace: Arc<dyn agentic_automation::WorkspaceContext> = self.platform.clone();
        let workflow_context = serde_json::json!({
            // A template value, not a file handle. A node with no files says
            // so rather than rendering a path that resolves to nothing.
            "workspace_path": workspace.workspace_path().map(|p| p.to_string_lossy().to_string()),
        });

        // Inline-body path: a loop iteration's `{name, tasks}` body is
        // dispatched as a synthetic sub-automation with `body: Some(_)` and
        // no on-disk file. Skip `resolve_automation_yaml` in that case and
        // deserialize the body directly.
        let mut workflow_config: agentic_automation::AutomationConfig = if let Some(body) =
            body.as_ref()
        {
            serde_json::from_value(body.clone())
                .map_err(|e| format!("failed to parse inline automation body: {e}"))?
        } else {
            let yaml = workspace
                .resolve_automation_yaml(workflow_ref)
                .await
                .map_err(|e| format!("failed to load automation: {e}"))?;
            serde_yaml::from_str(&yaml).map_err(|e| format!("failed to parse automation: {e}"))?
        };

        // Pre-resolve every nested sub-automation's tasks so the decider can
        // emit the full recursive DAG in `subrun_started` (the FE relies on
        // `inner_tasks` for nested step breakdown — see `agentic_automation::
        // resolve::build_subrun_steps`). Resolution is best-effort: missing
        // children leave `resolved_tasks` empty and the run still executes.
        agentic_automation::resolve_sub_automations(&mut workflow_config, &*workspace).await;

        // Hash the parsed config (canonical SHA-256), not raw YAML bytes —
        // reformatting must not invalidate cache, but a semantic edit must.
        let yaml_hash = agentic_automation::hash::canonical_hash(&workflow_config)
            .map_err(|e| format!("failed to hash automation config: {e}"))?;

        // Pre-materialise the prior-run cache snapshot exactly once, here at
        // seed time. The old design re-loaded the prior row and re-read
        // `metadata.invalidate_steps` on every decision — wasteful because
        // both inputs are immutable for the lifetime of this run, and after
        // the first cache-miss every downstream step naturally diverges
        // anyway (render_context shifts). We snapshot the filtered prior
        // cache into the new run's own state row and never consult either
        // source again.
        let (prior_step_hashes, prior_results) =
            if cache_enabled && let Some(prior_id) = retry_from_run_id.as_deref() {
                build_prior_snapshot(&self.db, run_id, prior_id).await?
            } else {
                (
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                )
            };

        // Seed durable automation state. `initial_render_context` lets a
        // synthetic sub-automation (loop iteration body) start with its
        // parent's accumulated render context — without it, inner
        // template references like `{{ schedules.value }}` resolve to
        // undefined. The value lands on both the in-memory
        // `render_context` (for the first decider pass) and the
        // `initial_render_context` column (for subsequent passes
        // after state is loaded back from disk — the rebuilt-from-
        // results context alone doesn't carry the iteration variable).
        let mut initial_render_context =
            initial_render_context.unwrap_or_else(|| serde_json::json!({}));
        // Fold effective variables (automation declarations + runtime
        // overrides) into the seed context so templates can reference
        // them by name (`{{ metric_label }}`). The overrides
        // come from the parent's `type: workflow.variables:` block;
        // the declarations come from `AutomationConfig.variables` and
        // are stripped of any `{default: X}` wrapper. Without this
        // fold, the variables sit in `state.variables` and are
        // invisible to render-context lookups.
        let declared = workflow_config
            .variables
            .as_ref()
            .and_then(|m| serde_json::to_value(m).ok());
        let effective = agentic_automation::variables::effective_variables(
            declared.as_ref(),
            variables.as_ref(),
        );
        if let (Some(ctx_obj), Some(vars_obj)) = (
            initial_render_context.as_object_mut(),
            effective.as_object(),
        ) {
            for (k, v) in vars_obj {
                ctx_obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }

        // Read `invalidate_iterations` once from the run row's metadata
        // and stamp onto the workflow_state row — same one-time-at-seed
        // pattern `build_prior_snapshot` uses for `invalidate_steps`.
        // Persisted inline so the decider's loop branch checks it
        // without re-querying `agentic_runs` per decision pass.
        let invalidate_iterations = read_invalidate_iterations(&self.db, run_id)
            .await
            .unwrap_or_default();

        let initial_state = agentic_automation::extension::AutomationRunState {
            run_id: run_id.to_string(),
            workflow: workflow_config,
            workflow_yaml_hash: yaml_hash,
            workflow_context,
            variables,
            trace_id: format!("wf-{}", uuid::Uuid::new_v4()),
            current_step: 0,
            results: std::collections::HashMap::new(),
            render_context: initial_render_context.clone(),
            pending_children: std::collections::HashMap::new(),
            decision_version: 0,
            step_hashes: std::collections::HashMap::new(),
            retry_from_run_id,
            cache_enabled,
            prior_step_hashes,
            prior_results,
            initial_render_context,
            invalidate_iterations,
        };
        agentic_automation::extension::insert_automation_state(&self.db, &initial_state)
            .await
            .map_err(|e| format!("failed to seed automation state: {e}"))?;

        // Immediately signal the coordinator to chain the first AutomationDecision.
        let (_, event_rx) = mpsc::channel::<(String, Value)>(1);
        let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(1);
        let _ = outcome_tx
            .send(TaskOutcome::Done {
                answer: String::new(),
                metadata: Some(serde_json::json!({"workflow_continue": true})),
            })
            .await;

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }

    /// Execute a stateless `AutomationDecision` task.
    ///
    /// Loads `agentic_workflow_state`, calls `AutomationDecider::decide`, and
    /// atomically commits the resulting state patch, emitted events, and any
    /// terminal queue/run transition via
    /// [`agentic_automation::extension::commit_decision`]. Only after that commit
    /// succeeds does it drive the decision as an [`ExecutingTask`] — the
    /// outcome channel merely signals the coordinator; durable state is
    /// already on disk.
    ///
    /// On a `decision_version` mismatch the commit rolls back and this
    /// function returns a no-op task (`workflow_version_conflict`), exactly as
    /// before. Because the pre-refactor flow wrote state, events, and queue
    /// status as three independent statements, a silent failure between them
    /// could strand an automation with advanced state but no follow-up events or
    /// queue entries — this rewrite closes that gap.
    pub(super) async fn execute_automation_decision(
        &self,
        run_id: &str,
        pending_child_answer: Option<agentic_core::delegation::ChildCompletion>,
    ) -> Result<ExecutingTask, String> {
        let state = agentic_automation::extension::load_automation_state(&self.db, run_id)
            .await
            .map_err(|e| format!("load automation state: {e}"))?
            .ok_or_else(|| format!("automation state not found for run {run_id}"))?;

        let expected_version = state.decision_version;

        // Snapshot the result + step_hash keys BEFORE the decide call so we
        // can compute the delta afterward.  `state` is moved into `decide`,
        // so we capture the keys while we still own it.
        let pre_decide_keys: std::collections::HashSet<String> =
            state.results.keys().cloned().collect();
        let pre_decide_hash_keys: std::collections::HashSet<String> =
            state.step_hashes.keys().cloned().collect();

        // The prior-cache snapshot (already minus invalidated steps) was
        // materialised on this row at seed time. Build a thin synthetic
        // `AutomationRunState` from it for the decider — only `.run_id`,
        // `.step_hashes`, and `.results` are read, so the rest can stay
        // default. This replaces the old per-decision `load_automation_state`
        // round-trip plus the `read_invalidate_steps` metadata read.
        let prior_state_owned = if state.cache_enabled
            && let Some(prior_run_id) = state.retry_from_run_id.clone()
        {
            Some(synthetic_prior_state(
                prior_run_id,
                state.prior_step_hashes.clone(),
                state.prior_results.clone(),
            ))
        } else {
            None
        };

        // Workspace path is needed by `decide()` for the file-presence
        // cache check (`TaskConfig.cache.path`). The workspace
        // doesn't change across decision passes within a run, so we
        // clone the path once here rather than threading it through
        // `AutomationRunState`.
        let workspace: std::sync::Arc<dyn agentic_automation::WorkspaceContext> =
            self.platform.clone();
        // `decide` takes `Option<&Path>` and skips the file-cache probe when it
        // is `None` (`step_decider::probe_file_cache`), so a node with no files
        // degrades to "cache miss" instead of resolving against a directory
        // that is not there.
        //
        // The `.filter(|p| p.is_dir())` is what makes that sentence true. It
        // used to say "a node with no files" while testing only whether the
        // manager DECLARED a working copy — which it does on every node,
        // replica included. So a worker took the `Some` branch and probed a
        // directory that is not there. Benign here (a probe against a missing
        // root simply misses, which is the same answer), but the premise was
        // false, and the identical wrong premise one layer over is what took a
        // pipeline down in #3056. Enforced by
        // `crates/app/tests/platform/workspace_path_needs_is_dir.rs`.
        let workspace_path: Option<std::path::PathBuf> = workspace
            .workspace_path()
            .filter(|p| p.is_dir())
            .map(|p| p.to_path_buf());
        // An airway step inside this automation queues the same
        // `TaskSpec::Airway` a schedule does, so it must be admitted under the
        // same `airway_source_config` policy. The domain crate has neither a
        // `DatabaseConnection` nor the run's `workspace_id`, so the facade
        // injects the resolver; `decide()` calls it where it builds the spec,
        // before the coordinator writes the child's queue row.
        let admission_resolver =
            std::sync::Arc::new(crate::airway_config::PipelineAirwayAdmissionResolver::new(
                self.db.clone(),
                workspace,
                self.platform.workspace_id(),
            ));
        let decider = agentic_automation::AutomationDecider::new(None)
            .with_airway_admission_resolver(admission_resolver);
        let (new_state, decision) = decider
            .decide(
                state,
                pending_child_answer,
                prior_state_owned.as_ref(),
                workspace_path.as_deref(),
            )
            .await;

        // Compute the new step result(s) added this decision and pack them
        // all into a single delta object. The original implementation
        // rejected any decision that added more than one key — but the
        // decider legitimately produces multiple new keys per call: when a
        // delegated child completes (fold inserts result for step N), the
        // same `decide()` immediately advances and may inline-execute
        // step N+1 (formatter, conditional, or cache-hit). Both inserts
        // are correct and must be persisted atomically. Postgres'
        // `results || $delta::jsonb` handles a multi-key object the same
        // way it handles a single-key one.
        let result_delta: serde_json::Value = serde_json::Value::Object(
            new_state
                .results
                .iter()
                .filter(|(k, _)| !pre_decide_keys.contains(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );

        let step_hash_delta: serde_json::Value = serde_json::Value::Object(
            new_state
                .step_hashes
                .iter()
                .filter(|(k, _)| !pre_decide_hash_keys.contains(*k))
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        );

        // Prepend a `decider_decided` trace event so admins can see *why*
        // the decider went the way it did — which variant it picked,
        // for which step, and with what fan-out shape. Inserted at the
        // head of the events list so it lands before the decider's own
        // per-variant `subrun_step_*` events for the same decision.
        let mut events = vec![decider_decided_event(&decision)];
        events.extend(decision_events(&decision).iter().cloned());

        // Build the export + cache-write plan from the delta + automation
        // config *before* `new_state` moves into `commit_decision`. Run
        // the writes only after the commit succeeds — that way a
        // version-conflict rollback can't leave half-written files for
        // a step the runtime doesn't actually consider committed.
        //
        // Steps that came from a file-cache hit (`subrun_step_cache_hit`
        // with `source: "file"`) are excluded from the cache-write pass
        // so we don't clobber the file the user may have just edited;
        // those rows still go through `export:` if it's configured
        // separately.
        let file_cache_hit_steps = collect_file_cache_hit_steps(&events);
        let failed_steps = collect_failed_steps(&events);
        let export_plan = build_export_plan(
            &new_state,
            &result_delta,
            &file_cache_hit_steps,
            &failed_steps,
        );
        let terminal = decision_terminal(&decision);

        // AutomationDecision tasks use their run_id as their queue task_id.
        let decision_task_id = run_id.to_string();
        // We are running inside `TaskExecutor::execute`, i.e. on the worker
        // that just claimed `decision_task_id`. Scope the commit's terminal
        // queue write to that claim so a peer that re-claimed the row after a
        // graceful release can't have its live claim stamped terminal by this
        // (possibly dying) process — see `DecisionClaim`.
        //
        // `process_worker_id_if_initialized` rather than `process_worker_id`:
        // an uninitialized cell is a *fact* that this process never built a
        // `DurableTransport` and so holds no claims at all, and minting an id
        // here would fabricate an owner that matches nothing.
        let claim = match agentic_runtime::transport::process_worker_id_if_initialized() {
            Some(worker_id) => {
                agentic_automation::extension::DecisionClaim::HeldBy(worker_id.to_string())
            }
            None => agentic_automation::extension::DecisionClaim::Unclaimed,
        };
        let outcome = agentic_automation::extension::commit_decision(
            &self.db,
            agentic_automation::extension::DecisionCommit {
                run_id: run_id.to_string(),
                decision_task_id,
                claim,
                expected_version,
                new_state,
                result_delta,
                step_hash_delta,
                events,
                attempt: 0,
                terminal,
            },
        )
        .await
        .map_err(|e| format!("commit_decision: {e}"))?;

        match outcome {
            agentic_automation::extension::CommitOutcome::VersionConflict => {
                tracing::debug!(run_id = %run_id, "AutomationDecision: version conflict — discarding");
                return Ok(noop_stop_task("workflow_version_conflict").await);
            }
            // A peer re-claimed the decision task while this decision was
            // running (graceful release during shutdown). Nothing was
            // persisted; the peer drives the run from durable state.
            agentic_automation::extension::CommitOutcome::ClaimLost => {
                tracing::warn!(run_id = %run_id, "AutomationDecision: claim lost to a peer — discarding");
                return Ok(noop_stop_task("workflow_claim_lost").await);
            }
            agentic_automation::extension::CommitOutcome::Committed => {}
        }

        // After commit succeeds, run the export plan. Walking the delta
        // covers every task type uniformly: inline (formatter /
        // conditional), delegated automation steps (execute_sql /
        // semantic_query / …), agent tasks, and sub-automations — they
        // all surface as a new key in `state.results` either inline
        // within this decide call or via the fold path after a child
        // completion.
        //
        // Export failures are logged but do NOT fail the decision —
        // the step already succeeded and committed. A missing file is
        // an ops issue, not a task-correctness issue. The legacy
        // behavior of "fail the step on export failure" only applied
        // to inline tasks (run_automation_step used to do the write);
        // agent / sub-automation steps would have failed silently anyway
        // because they never hit that path.
        run_export_plan(self.platform.clone(), run_id, export_plan).await;

        run_decision_task(decision)
    }
}

/// One step's pre-resolved post-commit write work. Either or both of
/// `export` / `cache` may be set; the runner handles each
/// independently. Cloned out of `new_state` so the commit can move
/// that struct without preventing the post-commit pass.
struct StepWriteEntry {
    step_name: String,
    /// `export: { path, format }` — separate from cache; writes
    /// regardless of how the step ran.
    export: Option<agentic_automation::config::TaskExport>,
    /// `cache: { enabled, path }` — only set when the step actually
    /// ran (cache-hit steps are skipped here to preserve the file the
    /// user may have edited).
    cache: Option<agentic_automation::config::CacheConfig>,
    result: serde_json::Value,
}

/// Scan the about-to-commit events for `subrun_step_cache_hit
/// { source: "file" }` markers — these identify steps whose result
/// came from the file-presence cache, which the post-commit cache-write
/// pass must skip to avoid clobbering the file (the whole point is
/// that the user may have edited it).
fn collect_file_cache_hit_steps(events: &[(String, serde_json::Value)]) -> HashSet<String> {
    let mut hits = HashSet::new();
    for (event_type, payload) in events {
        if event_type != "subrun_step_cache_hit" {
            continue;
        }
        let is_file = payload
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s == "file")
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        if let Some(step) = payload.get("step").and_then(|v| v.as_str()) {
            hits.insert(step.to_string());
        }
    }
    hits
}

/// Scan the about-to-commit events for `subrun_step_completed
/// { success: false }` — failed steps must be excluded from the
/// post-commit export / cache-write pass.
///
/// Without this, the fold path inserts the failure answer (the error
/// string) into `state.results`, the result_delta picks it up, and
/// `write_cache` writes the error to `cache.path`. The next run then
/// sees a file at the cache path, short-circuits the agent step
/// with the error string as its result, and downstream steps fail.
/// Drop them from the plan instead.
fn collect_failed_steps(events: &[(String, serde_json::Value)]) -> HashSet<String> {
    let mut failed = HashSet::new();
    for (event_type, payload) in events {
        if event_type != "subrun_step_completed" {
            continue;
        }
        let success = payload
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if success {
            continue;
        }
        if let Some(step) = payload.get("step").and_then(|v| v.as_str()) {
            failed.insert(step.to_string());
        }
    }
    failed
}

/// Cross-reference `result_delta` with the automation's task list to
/// figure out which newly-finalised steps need an `export:` write or a
/// `cache:` write (or both).
///
/// - Steps in `file_cache_hits` skip the cache-write side because their
///   result came from the file itself — re-writing would clobber any
///   user edits.
/// - Steps in `failed_steps` are excluded entirely — the fold path
///   inserts the failure answer (an error string) into `state.results`
///   so it lands in `result_delta`, but persisting that to `cache.path`
///   or an `export:` target would silently poison the artifact and
///   break the next run.
fn build_export_plan(
    state: &agentic_automation::extension::AutomationRunState,
    result_delta: &serde_json::Value,
    file_cache_hits: &HashSet<String>,
    failed_steps: &HashSet<String>,
) -> (Vec<StepWriteEntry>, serde_json::Value) {
    let mut plan = Vec::new();
    let render_context = state.render_context.clone();
    let Some(delta) = result_delta.as_object() else {
        return (plan, render_context);
    };
    for (step_name, result) in delta {
        if failed_steps.contains(step_name) {
            continue;
        }
        let Some(task) = state.workflow.tasks.iter().find(|t| &t.name == step_name) else {
            continue;
        };
        let cache = task
            .cache
            .as_ref()
            .filter(|c| c.enabled && !file_cache_hits.contains(step_name))
            .cloned();
        let export = task.export.clone();
        if cache.is_none() && export.is_none() {
            continue;
        }
        plan.push(StepWriteEntry {
            step_name: step_name.clone(),
            export,
            cache,
            result: result.clone(),
        });
    }
    (plan, render_context)
}

/// Execute a previously-built export/cache plan. Logs on failure;
/// never returns an error — the caller has already committed the
/// decision.
async fn run_export_plan(
    platform: std::sync::Arc<dyn crate::platform::PlatformContext>,
    run_id: &str,
    plan: (Vec<StepWriteEntry>, serde_json::Value),
) {
    let (entries, render_context) = plan;
    if entries.is_empty() {
        return;
    }
    let workspace: std::sync::Arc<dyn agentic_automation::WorkspaceContext> = platform;
    for entry in entries {
        if let Some(export) = &entry.export
            && let Err(e) = agentic_automation::export::write_export(
                workspace.as_ref(),
                &entry.step_name,
                export,
                &entry.result,
                &render_context,
            )
            .await
        {
            tracing::warn!(
                run_id,
                step = %entry.step_name,
                error = %e,
                "step export failed (post-commit; step still succeeded)"
            );
        }
        if let Some(cache) = &entry.cache
            && let Err(e) = agentic_automation::export::write_cache(
                workspace.as_ref(),
                &entry.step_name,
                cache,
                &entry.result,
                &render_context,
            )
            .await
        {
            tracing::warn!(
                run_id,
                step = %entry.step_name,
                error = %e,
                "step cache-write failed (post-commit; step still succeeded)"
            );
        }
    }
}

/// A decision that persisted nothing and must not advance the run.
///
/// `flag` names *why* — `workflow_version_conflict` (a peer won the
/// `decision_version` CAS) or `workflow_claim_lost` (a peer re-claimed the
/// decision task's queue row after this process was gracefully released).
/// Both are handled identically by `AutomationCompletionPolicy`: defer, and
/// let the peer that actually owns the run drive it. The flags stay distinct
/// so the reason is legible in the event log.
async fn noop_stop_task(flag: &str) -> ExecutingTask {
    let (_, event_rx) = mpsc::channel::<(String, Value)>(1);
    let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(1);
    let _ = outcome_tx
        .send(TaskOutcome::Done {
            answer: String::new(),
            metadata: Some(serde_json::json!({ flag: true })),
        })
        .await;
    ExecutingTask {
        events: event_rx,
        outcomes: outcome_rx,
        cancel: CancellationToken::new(),
        answers: None,
    }
}

/// Build a `decider_decided` trace event capturing which
/// [`AutomationDecision`] variant came out of the decider for the current
/// pass, with just enough structured context for an admin to reconstruct
/// the run's control flow without re-running the decider in their head.
///
/// Emitted from `execute_automation_decision` *before* the variant's own
/// `subrun_step_*` events, so on the SSE timeline a decision pass
/// reads as: "decider chose X → here's what it emitted." Lives on the
/// run's stream alongside `worker_task_claimed` / `task_failed` /
/// `waiting_on_children`.
fn decider_decided_event(d: &agentic_automation::AutomationDecision) -> (String, Value) {
    use agentic_automation::AutomationDecision as D;
    use agentic_core::delegation::DelegationTarget;
    let payload = match d {
        D::DelegateStep {
            step_index,
            step_name,
            spec,
            ..
        } => {
            let target = match spec {
                agentic_core::delegation::TaskSpec::Agent { agent_id, .. } => {
                    serde_json::json!({ "kind": "agent", "agent_id": agent_id })
                }
                agentic_core::delegation::TaskSpec::Automation { workflow_ref, .. } => {
                    serde_json::json!({ "kind": "workflow", "workflow_ref": workflow_ref })
                }
                agentic_core::delegation::TaskSpec::AutomationStep { .. } => {
                    serde_json::json!({ "kind": "workflow_step" })
                }
                _ => serde_json::json!({ "kind": "other" }),
            };
            serde_json::json!({
                "kind": "delegate_step",
                "step_index": step_index,
                "step_name": step_name,
                "target": target,
            })
        }
        D::DelegateParallel {
            step_index,
            step_name,
            items,
            failure_policy,
            ..
        } => {
            let targets: Vec<&'static str> = items
                .iter()
                .map(|it| match &it.target {
                    DelegationTarget::Agent { .. } => "agent",
                    DelegationTarget::Automation { .. } => "workflow",
                })
                .collect();
            serde_json::json!({
                "kind": "delegate_parallel",
                "step_index": step_index,
                "step_name": step_name,
                "item_count": items.len(),
                "targets": targets,
                "failure_policy": serde_json::to_value(failure_policy).unwrap_or(Value::Null),
            })
        }
        D::StepExecutedInline { step_name, .. } => {
            serde_json::json!({ "kind": "step_executed_inline", "step_name": step_name })
        }
        D::WaitForMoreChildren => serde_json::json!({ "kind": "wait_for_more_children" }),
        D::Complete { .. } => serde_json::json!({ "kind": "complete" }),
        D::Fail { error, .. } => serde_json::json!({ "kind": "fail", "error": error }),
    };
    ("decider_decided".to_string(), payload)
}

fn decision_events(d: &agentic_automation::AutomationDecision) -> &[(String, Value)] {
    use agentic_automation::AutomationDecision as D;
    match d {
        D::Complete { emitted_events, .. }
        | D::StepExecutedInline { emitted_events, .. }
        | D::DelegateStep { emitted_events, .. }
        | D::DelegateParallel { emitted_events, .. }
        | D::Fail { emitted_events, .. } => emitted_events.as_slice(),
        D::WaitForMoreChildren => &[],
    }
}

/// Map an [`AutomationDecision`] variant onto the terminal behavior
/// [`commit_decision`] applies to the decision task's queue row and the
/// automation run row.
///
/// Only `Complete` and `Fail` flip the run + queue rows to `done`/`failed`
/// inside the commit; every other variant leaves them alone because the
/// worker's downstream `Suspended`/`Done` outcome still has to flow through
/// the coordinator to schedule the next activity.
fn decision_terminal(
    d: &agentic_automation::AutomationDecision,
) -> agentic_automation::extension::DecisionTerminal {
    use agentic_automation::AutomationDecision as D;
    use agentic_automation::extension::DecisionTerminal;
    match d {
        D::Complete { final_answer, .. } => DecisionTerminal::CompleteAutomation {
            final_answer: final_answer.clone(),
        },
        D::Fail { error, .. } => DecisionTerminal::FailAutomation {
            error: error.clone(),
        },
        _ => DecisionTerminal::Continuing,
    }
}

/// Convert an `AutomationDecision` into an `ExecutingTask` that emits the
/// appropriate events and outcome on its channels, then exits.
pub fn run_decision_task(
    decision: agentic_automation::AutomationDecision,
) -> Result<ExecutingTask, String> {
    use agentic_automation::AutomationDecision as D;
    use agentic_core::delegation::SuspendReason;
    use agentic_core::human_input::SuspendedRunData;

    // Larger buffers prevent the spawned task from blocking on channel sends
    // when the coordinator is briefly busy, especially for DelegateParallel
    // fan-outs that emit many events before returning a single outcome.
    let (event_tx, event_rx) = mpsc::channel::<(String, Value)>(256);
    let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(16);
    let cancel = CancellationToken::new();

    spawn_with_hub(async move {
        match decision {
            D::Complete {
                final_answer,
                emitted_events,
            } => {
                for (et, p) in emitted_events {
                    let _ = event_tx.send((et, p)).await;
                }
                let _ = outcome_tx
                    .send(TaskOutcome::Done {
                        answer: final_answer,
                        metadata: None,
                    })
                    .await;
            }

            D::StepExecutedInline { emitted_events, .. } => {
                for (et, p) in emitted_events {
                    let _ = event_tx.send((et, p)).await;
                }
                // Chain to next decision immediately.
                let _ = outcome_tx
                    .send(TaskOutcome::Done {
                        answer: String::new(),
                        metadata: Some(serde_json::json!({"workflow_continue": true})),
                    })
                    .await;
            }

            D::WaitForMoreChildren => {
                let _ = outcome_tx
                    .send(TaskOutcome::Done {
                        answer: String::new(),
                        metadata: Some(serde_json::json!({"workflow_waiting_siblings": true})),
                    })
                    .await;
            }

            D::DelegateStep {
                step_index,
                step_name,
                spec,
                trace_id,
                emitted_events,
            } => {
                for (et, p) in emitted_events {
                    let _ = event_tx.send((et, p)).await;
                }
                let (target, request, context) = spec_to_delegation_parts(&spec, &step_name);
                let resume_data = SuspendedRunData {
                    from_state: "workflow_decision".to_string(),
                    original_input: step_name.clone(),
                    trace_id,
                    stage_data: serde_json::json!({"step_name": step_name, "step_index": step_index}),
                    question: format!("Executing step: {step_name}"),
                    suggestions: vec![],
                };
                let _ = outcome_tx
                    .send(TaskOutcome::Suspended {
                        reason: SuspendReason::Delegation {
                            target,
                            request,
                            context,
                            policy: None,
                        },
                        resume_data,
                        trace_id: String::new(),
                    })
                    .await;
            }

            D::DelegateParallel {
                step_index,
                step_name,
                items,
                failure_policy,
                trace_id,
                emitted_events,
            } => {
                for (et, p) in emitted_events {
                    let _ = event_tx.send((et, p)).await;
                }
                let resume_data = SuspendedRunData {
                    from_state: "workflow_decision".to_string(),
                    original_input: step_name.clone(),
                    trace_id,
                    stage_data: serde_json::json!({"step_name": step_name, "step_index": step_index}),
                    question: format!("Executing step: {step_name}"),
                    suggestions: vec![],
                };
                let _ = outcome_tx
                    .send(TaskOutcome::Suspended {
                        reason: SuspendReason::ParallelDelegation {
                            targets: items,
                            failure_policy,
                        },
                        resume_data,
                        trace_id: String::new(),
                    })
                    .await;
            }

            D::Fail {
                error,
                emitted_events,
            } => {
                // Pump any events queued before the Fail decision (in
                // particular, the failing step's `subrun_step_completed`
                // with the error message) before reporting the outcome —
                // otherwise the SSE stream closes without the per-step
                // failure event and the frontend has no error to render.
                for (et, p) in emitted_events {
                    let _ = event_tx.send((et, p)).await;
                }
                let _ = outcome_tx.send(TaskOutcome::Failed(error)).await;
            }
        }
    });

    Ok(ExecutingTask {
        events: event_rx,
        outcomes: outcome_rx,
        cancel,
        answers: None,
    })
}

/// Extract delegation target, request, and context from a `TaskSpec`.
fn spec_to_delegation_parts(
    spec: &TaskSpec,
    step_name: &str,
) -> (
    agentic_core::delegation::DelegationTarget,
    String,
    serde_json::Value,
) {
    use agentic_core::delegation::DelegationTarget;
    match spec {
        TaskSpec::Agent {
            agent_id,
            question,
            extra,
        } => (
            DelegationTarget::Agent {
                agent_id: agent_id.clone(),
            },
            question.clone(),
            // The delegation resolver expects `extra` nested under the
            // context (`context.get("extra")` in
            // `coordinator/policy.rs::resolve_agent`). Without this
            // envelope, the automation's per-step `extra` payload
            // (analytics `output_mode`) is silently lost on the
            // delegation hop.
            extra
                .clone()
                .map(|v| serde_json::json!({ "extra": v }))
                .unwrap_or_else(|| serde_json::json!({})),
        ),
        TaskSpec::Automation {
            workflow_ref,
            variables,
            ..
        } => (
            DelegationTarget::Automation {
                workflow_ref: workflow_ref.clone(),
            },
            format!("Execute sub-workflow: {workflow_ref}"),
            variables.clone().unwrap_or(serde_json::json!({})),
        ),
        TaskSpec::AutomationStep {
            step_config,
            render_context,
            workflow_context,
        } => (
            DelegationTarget::Automation {
                workflow_ref: "__workflow_step__".to_string(),
            },
            step_name.to_string(),
            serde_json::json!({
                "step_config": step_config,
                "render_context": render_context,
                "workflow_context": workflow_context,
            }),
        ),
        // `DelegationTarget` has no `Airway` variant, so — like
        // `AutomationStep` above — tunnel through an `Automation` target with
        // a sentinel ref and carry the real spec in context. The automation
        // resolver (`AutomationDelegationResolver::resolve_automation`)
        // rebuilds `TaskSpec::Airway` from `airway_spec`.
        TaskSpec::Airway { .. } => (
            DelegationTarget::Automation {
                workflow_ref: "__airway__".to_string(),
            },
            step_name.to_string(),
            serde_json::json!({
                "airway_spec": serde_json::to_value(spec).unwrap_or(serde_json::Value::Null),
            }),
        ),
        _ => (
            DelegationTarget::Automation {
                workflow_ref: "__unknown__".to_string(),
            },
            step_name.to_string(),
            serde_json::json!({}),
        ),
    }
}

/// Compute the filtered prior-cache snapshot used by the decider.
///
/// Loads the prior run's state once and reads `metadata.invalidate_steps`
/// off the current run's `agentic_runs` row once, then strips invalidated
/// entries from both `step_hashes` and `results`. The two maps are returned
/// for the caller to stamp onto the new run's seed state — after this point
/// the decider operates entirely off in-memory copies.
///
/// A missing prior row, missing metadata, or malformed `invalidate_steps`
/// is treated as a soft no-op (empty / unfiltered, respectively) rather
/// than failing the seed. The user can still see the seed happened; the
/// worst outcome is that one step that *should* have been replayed gets
/// reused. That's the same forgiving behavior the pre-refactor code had.
async fn build_prior_snapshot(
    db: &sea_orm::DatabaseConnection,
    run_id: &str,
    prior_run_id: &str,
) -> Result<
    (
        std::collections::HashMap<String, String>,
        std::collections::HashMap<String, serde_json::Value>,
    ),
    String,
> {
    let Some(prior) = agentic_automation::extension::load_automation_state(db, prior_run_id)
        .await
        .map_err(|e| format!("load prior automation state: {e}"))?
    else {
        return Ok((
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        ));
    };
    let mut step_hashes = prior.step_hashes;
    let mut results = prior.results;
    if let Some(invalidate) = read_invalidate_steps(db, run_id).await {
        for step in invalidate {
            step_hashes.remove(&step);
            results.remove(&step);
        }
    }
    Ok((step_hashes, results))
}

/// Read `metadata.invalidate_steps` from `agentic_runs` for the given run.
///
/// Called exactly once per run, at seed time — never on the per-decision
/// hot path. Returns `None` when the row is missing, the metadata is
/// absent, or the field is empty / wrong shape.
async fn read_invalidate_steps(
    db: &sea_orm::DatabaseConnection,
    run_id: &str,
) -> Option<Vec<String>> {
    let run = agentic_runtime::crud::get_run(db, run_id).await.ok()??;
    let arr = run.metadata.as_ref()?.get("invalidate_steps")?.as_array()?;
    let steps: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if steps.is_empty() { None } else { Some(steps) }
}

/// Read `metadata.invalidate_iterations` from `agentic_runs` for the
/// given run, validated to `{step_name: [indices]}`.
///
/// Called once at seed time alongside `read_invalidate_steps` and
/// stamped onto the workflow_state row's `invalidate_iterations` column
/// so the decider applies it without re-reading metadata per decision
/// pass. Malformed shapes (wrong types, missing keys) silently drop
/// the offending entries — soft hint, same forgiving policy as
/// `read_invalidate_steps`.
async fn read_invalidate_iterations(
    db: &sea_orm::DatabaseConnection,
    run_id: &str,
) -> Option<std::collections::HashMap<String, Vec<usize>>> {
    let run = agentic_runtime::crud::get_run(db, run_id).await.ok()??;
    let obj = run
        .metadata
        .as_ref()?
        .get("invalidate_iterations")?
        .as_object()?;
    let mut out = std::collections::HashMap::new();
    for (step_name, idx_value) in obj {
        let Some(arr) = idx_value.as_array() else {
            continue;
        };
        let indices: Vec<usize> = arr
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as usize))
            .collect();
        if !indices.is_empty() {
            out.insert(step_name.clone(), indices);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Build a minimal `AutomationRunState` to hand to the decider as `prior_state`.
///
/// The decider only reads `prior.run_id`, `prior.step_hashes`, and
/// `prior.results`. Everything else is left at its `Default`-ish zero value —
/// this struct never escapes the decision-time scope and is dropped right
/// after `decide()` returns.
fn synthetic_prior_state(
    run_id: String,
    step_hashes: std::collections::HashMap<String, String>,
    results: std::collections::HashMap<String, serde_json::Value>,
) -> agentic_automation::extension::AutomationRunState {
    agentic_automation::extension::AutomationRunState {
        run_id,
        workflow: agentic_automation::AutomationConfig {
            name: String::new(),
            tasks: vec![],
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        },
        workflow_yaml_hash: String::new(),
        workflow_context: serde_json::json!({}),
        variables: None,
        trace_id: String::new(),
        current_step: 0,
        results,
        render_context: serde_json::json!({}),
        pending_children: std::collections::HashMap::new(),
        decision_version: 0,
        step_hashes,
        retry_from_run_id: None,
        cache_enabled: false,
        prior_step_hashes: std::collections::HashMap::new(),
        prior_results: std::collections::HashMap::new(),
        initial_render_context: serde_json::json!({}),
        invalidate_iterations: std::collections::HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::delegation::DelegationTarget;

    /// Regression: the automation→coordinator delegation hop must
    /// preserve `TaskSpec::Agent.extra` by nesting it under
    /// `{"extra": ...}` so the coordinator's `resolve_agent`
    /// (`crates/agentic/runtime/src/orchestrator/coordinator/policy.rs`)
    /// can pull it back out. A bare `{}` here silently drops the
    /// per-step `output_mode` payload.
    #[test]
    fn spec_to_delegation_parts_preserves_agent_extra() {
        let extra = serde_json::json!({ "output_mode": "sql" });
        let spec = TaskSpec::Agent {
            agent_id: "analytics.agentic.yml".into(),
            question: "aggregate by month".into(),
            extra: Some(extra.clone()),
        };
        let (target, request, context) = spec_to_delegation_parts(&spec, "query");
        assert!(matches!(
            target,
            DelegationTarget::Agent { ref agent_id } if agent_id == "analytics.agentic.yml"
        ));
        assert_eq!(request, "aggregate by month");
        assert_eq!(context.get("extra"), Some(&extra));
    }

    /// Without `extra`, the context is an empty object so the
    /// resolver's `context.get("extra")` returns `None` and the agent
    /// runs in default mode.
    #[test]
    fn spec_to_delegation_parts_handles_none_extra() {
        let spec = TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        };
        let (_, _, context) = spec_to_delegation_parts(&spec, "step");
        assert_eq!(context, serde_json::json!({}));
    }

    /// Regression: `DelegationTarget` has no `Airway` variant, so an airway
    /// step must tunnel through the `__airway__` `Automation` sentinel with
    /// the serialized spec in context — NOT hit the `_ =>` fallback, which
    /// maps to `__unknown__` and makes the coordinator try to load an
    /// automation named "__unknown__" ("failed to read workflow"). The spec
    /// must round-trip back out of the context intact.
    #[test]
    fn spec_to_delegation_parts_tunnels_airway_through_sentinel() {
        let spec = TaskSpec::Airway {
            pipeline_ref: "pipelines/toast_pos.airway.yml".into(),
            variables: None,
            resources: vec!["orders".into()],
            backfill_from: None,
            backfill_to: None,
            contract_policy: None,
            environment: None,
        };
        let (target, _request, context) = spec_to_delegation_parts(&spec, "ingest");
        assert!(
            matches!(target, DelegationTarget::Automation { ref workflow_ref } if workflow_ref == "__airway__"),
            "airway must tunnel through the __airway__ sentinel, got {target:?}"
        );
        let rebuilt: TaskSpec =
            serde_json::from_value(context.get("airway_spec").cloned().unwrap()).unwrap();
        assert!(
            matches!(rebuilt, TaskSpec::Airway { ref pipeline_ref, .. } if pipeline_ref == "pipelines/toast_pos.airway.yml"),
            "context must carry the full Airway spec, got {rebuilt:?}"
        );
    }
}
