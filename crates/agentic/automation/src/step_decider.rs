//! Stateless automation decision task.
//!
//! Replaces the long-lived `AutomationStepOrchestrator` actor. Each call to
//! `AutomationDecider::decide` loads state, folds in any completed child answer,
//! decides the next action, and returns. No in-memory channels survive a crash.

use std::path::Path;
use std::sync::Arc;

use crate::airway_admission::AirwayAdmissionResolver;
use crate::config::TaskType;
use crate::extension::AutomationRunState;
use crate::render::{render_jinja_string, validate_workspace_relative_path};
use crate::step_hash::{StepHashInputs, compute_step_hash};
use crate::step_orchestrator::{build_minijinja_context, to_column_oriented};
use agentic_core::delegation::{
    ChildCompletion, DelegationItem, DelegationTarget, FanoutFailurePolicy, ResolvedAdmission,
    TaskSpec,
};
use agentic_core::evaluator::ConsistencyEvaluator;
use serde_json::{Value, json};

/// What the decider decided to do next.
#[derive(Debug)]
pub enum AutomationDecision {
    /// Delegate a single child task and wait for its answer.
    DelegateStep {
        step_index: usize,
        step_name: String,
        spec: TaskSpec,
        trace_id: String,
        emitted_events: Vec<(String, Value)>,
    },
    /// Fan-out parallel delegation (consistency runs or sequential loops).
    DelegateParallel {
        step_index: usize,
        step_name: String,
        items: Vec<DelegationItem>,
        failure_policy: FanoutFailurePolicy,
        trace_id: String,
        emitted_events: Vec<(String, Value)>,
    },
    /// Inline step (formatter/conditional) was executed; chain to next decision.
    StepExecutedInline {
        step_name: String,
        emitted_events: Vec<(String, Value)>,
    },
    /// Parallel siblings still in flight — do nothing until another sibling completes.
    WaitForMoreChildren,
    /// All steps done — automation is complete.
    Complete {
        final_answer: String,
        emitted_events: Vec<(String, Value)>,
    },
    /// Unrecoverable error. `emitted_events` carries any events that were
    /// queued during the fold phase before the failure was detected — most
    /// importantly the `subrun_step_completed { success: false, error }`
    /// event for the failing step. Without these the frontend never sees
    /// the per-step error and the failure surfaces only as the run-level
    /// `failed` status.
    Fail {
        error: String,
        emitted_events: Vec<(String, Value)>,
    },
}

/// Stateless automation decider.
///
/// Call [`decide`] with the current DB state and an optional completed child
/// answer. The function returns the updated state and the next action to take.
/// The caller (executor) persists the updated state and acts on the decision.
pub struct AutomationDecider {
    #[allow(dead_code)]
    evaluator: Option<Arc<dyn ConsistencyEvaluator>>,
    /// Host port that resolves `airway_source_config` for an airway step.
    /// `None` = no host wiring (the inline Data-App runner, unit fixtures),
    /// which keeps the previous `permissive` / `production` behaviour.
    airway_admission: Option<Arc<dyn AirwayAdmissionResolver>>,
}

impl AutomationDecider {
    pub fn new(evaluator: Option<Arc<dyn ConsistencyEvaluator>>) -> Self {
        Self {
            evaluator,
            airway_admission: None,
        }
    }

    /// Inject the admission resolver used by [`StepKind::Airway`] dispatch.
    ///
    /// Queue-driven callers (`agentic-pipeline`'s executor) always set this;
    /// without it an airway step inside an automation queues with both
    /// admission fields `None` and silently ignores an operator's
    /// `airway_source_config` policy.
    pub fn with_airway_admission_resolver(
        mut self,
        resolver: Arc<dyn AirwayAdmissionResolver>,
    ) -> Self {
        self.airway_admission = Some(resolver);
        self
    }

    /// Resolve the admission for `pipeline_ref` at dispatch time.
    ///
    /// No resolver injected → the default (both fields `None`). A resolver
    /// that errors propagates: see [`AirwayAdmissionResolver`] for why this
    /// must not fall back to the default.
    async fn resolve_airway_admission(
        &self,
        pipeline_ref: &str,
    ) -> Result<ResolvedAdmission, String> {
        match &self.airway_admission {
            Some(resolver) => resolver.resolve_for_pipeline(pipeline_ref).await,
            None => Ok(ResolvedAdmission::default()),
        }
    }

    /// Core decision function.
    ///
    /// - `state`: loaded from `agentic_workflow_state`.
    /// - `pending_child_answer`: a just-completed child task, if any.
    /// - `prior_state`: the state of the run identified by
    ///   `state.retry_from_run_id`, loaded by the caller. Required for the
    ///   "resume only unchanged steps" path; pass `None` for fresh runs or
    ///   when `state.cache_enabled` is `false`.
    ///
    /// Returns `(updated_state, decision)`. The caller must persist `updated_state`
    /// before acting on the decision (optimistic CC via `decision_version`).
    ///
    /// # Cascading invalidation
    ///
    /// Cascade falls out of the hash construction: every step's hash includes
    /// the full `render_context`, which is built from prior step results. If
    /// step N's output changes, step N+1's render_context differs, so its
    /// hash differs, so it cache-misses and re-executes. No separate "cascade
    /// dirty" flag is needed.
    pub async fn decide(
        &self,
        mut state: AutomationRunState,
        pending_child_answer: Option<ChildCompletion>,
        prior_state: Option<&AutomationRunState>,
        workspace_path: Option<&Path>,
    ) -> (AutomationRunState, AutomationDecision) {
        // Events emitted during the fold phase — prepended to the decision's events.
        let mut fold_events: Vec<(String, Value)> = Vec::new();

        // ── 1. Fold in child answer if present ────────────────────────────
        if let Some(child) = pending_child_answer {
            let step_key = child.step_index.to_string();
            let step_name = child.step_name.clone();
            let answer_value = serde_json::from_str::<Value>(&child.answer)
                .unwrap_or_else(|_| json!({"text": child.answer}));
            let is_preagg = answer_value
                .get("is_preagg")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Loop steps need two extra fold-time invariants beyond the
            // simple `state.results.insert(step, answer)` path:
            // 1. Pre-seeded reusable entries (from per-iteration cache hits
            //    at decide time) must survive the fold; the fresh
            //    aggregated answer covers only the delegated subset.
            // 2. The `iterations` map (hash → outcome) must be (re)built
            //    so a subsequent retry can attribute every iteration's
            //    success/failure back to its (value, index) key.
            // `fold_loop_step_result` handles both; for non-loop steps it
            // returns the answer unchanged.
            let (folded, _folded_iters) = fold_loop_step_result(
                &state.workflow,
                &state.render_context,
                child.step_index,
                answer_value,
                state.results.get(&step_name),
            );
            state.results.insert(step_name.clone(), folded);

            if let Some(siblings) = state.pending_children.get_mut(&step_key) {
                siblings.retain(|id| id != &child.child_task_id);
                if siblings.is_empty() {
                    state.pending_children.remove(&step_key);
                }
            }

            // Still waiting on sibling tasks for this step?
            if state.pending_children.contains_key(&step_key) {
                return (state, AutomationDecision::WaitForMoreChildren);
            }

            // Step complete: emit event, optionally record its hash, update
            // render context, advance — *or* halt the automation on failure.
            let success = child.status == "done";
            // Emit the rich content payload BEFORE the status event so the
            // frontend's log aggregator can attach the per-step body to the
            // step row before it flips to "completed". The frontend treats
            // the absence of an output event as "no body" — losing the body
            // is fine; losing ordering is not.
            if success {
                let task = &state.workflow.tasks[child.step_index];
                if let Some(output) = state.results.get(&step_name) {
                    fold_events.push((
                        "subrun_step_output".to_string(),
                        json!({
                            "step": step_name,
                            "task_type": task.task_type.name(),
                            "output": output,
                        }),
                    ));
                }
            }
            // The completion event must carry `error` for failed children so
            // the frontend can surface it. `child.answer` doubles as the
            // error message when `child.status != "done"`.
            let mut completion =
                json!({ "step": step_name, "success": success, "is_preagg": is_preagg });
            if !success {
                completion["error"] = Value::String(child.answer.clone());
                if let Some(obj) = completion.as_object_mut() {
                    obj.insert("status".to_string(), Value::String(child.status.clone()));
                }
            }
            fold_events.push(("subrun_step_completed".to_string(), completion));

            // On failure, halt the automation. Without this `Fail` return,
            // execution falls through to the "decide on the next step"
            // branch below and silently advances past the broken step —
            // exactly the bug where downstream steps showed output despite
            // the diagram (correctly) marking them as not run.
            //
            // We also tack on a synthetic `subrun_completed` so the
            // frontend's "Automation failed" status flips immediately rather
            // than waiting for a separate event the backend never emits
            // for failures.
            if !success {
                fold_events.push((
                    "subrun_completed".to_string(),
                    json!({
                        "subrun_name": state.workflow.name,
                        "success": false,
                    }),
                ));
                return (
                    state,
                    AutomationDecision::Fail {
                        error: child.answer,
                        emitted_events: fold_events,
                    },
                );
            }

            // Record the step hash *only* on success — and *before*
            // update_render_context so the hash sees the same render context
            // that was used at delegate time. A failed step must not leave a
            // hash behind, otherwise a retry would cache-hit on the failure.
            let task = &state.workflow.tasks[child.step_index];
            if let Ok(hash) = compute_step_hash(&StepHashInputs {
                step_config: task,
                render_context: &state.render_context,
                variables: state.variables.as_ref(),
                loop_idx: None,
                sub_workflow_yaml_hash: None,
            }) {
                state.step_hashes.insert(step_name.clone(), hash);
            }
            update_render_context(&mut state, &step_name);
            state.current_step = child.step_index + 1;
        }

        // ── 2. Check for automation completion ──────────────────────────────
        if state.current_step >= state.workflow.tasks.len() {
            let final_answer = build_final_answer(&state);
            let mut events = fold_events;
            events.push((
                "subrun_completed".to_string(),
                json!({
                    "subrun_name": state.workflow.name,
                    "success": true,
                }),
            ));
            return (
                state,
                AutomationDecision::Complete {
                    final_answer,
                    emitted_events: events,
                },
            );
        }

        // ── 3. Decide on the current step ─────────────────────────────────
        let step_index = state.current_step;
        let task = state.workflow.tasks[step_index].clone();
        let step_name = task.name.clone();
        let trace_id = state.trace_id.clone();
        let wf_name = state.workflow.name.clone();

        let mut events: Vec<(String, Value)> = fold_events;

        // Emit subrun_started on the very first step. `steps` carries the
        // full nested task DAG (loops + pre-resolved sub-automations) so
        // the FE can render every level with per-task-type renderers
        // instead of falling back to raw JSON. See `build_subrun_steps`
        // for the recursive walk and `crate::resolve` for how
        // sub-automation children get pre-resolved before the run starts.
        if step_index == 0 {
            let steps = crate::resolve::build_subrun_steps(&state.workflow.tasks);
            events.push((
                "subrun_started".to_string(),
                json!({ "subrun_name": wf_name, "steps": steps }),
            ));
        }
        events.push((
            "subrun_step_started".to_string(),
            json!({ "step": step_name }),
        ));

        // ── File-presence cache ──────────────────────────────────────────
        //
        // Distinct from the step-hash cache below: this check skips the
        // step iff the configured `cache.path` file already exists on
        // disk. Mirrors the legacy `oxy-workflow::TaskCache` semantics so
        // a customer pattern of "agent generates SQL, user edits the
        // file, subsequent runs use the edited file as-is" keeps
        // working. File presence wins over step-hash — if both apply,
        // the file content is the source of truth (the whole point is
        // that the user may have edited it). See `TaskConfig.cache`.
        if let (Some(cache), Some(workspace)) = (task.cache.as_ref(), workspace_path)
            && cache.enabled
            && let Some((rendered_path, content)) =
                probe_file_cache(workspace, &cache.path, &state.render_context).await
        {
            // The cached content can be either a JSON-shaped result
            // (agents that emit structured output) or a raw text blob
            // (the common case — SQL the user edited, an LLM-generated
            // markdown report). Try JSON first; on parse failure, wrap
            // in `{"text": <content>}` to match the fold-step envelope
            // (see `decide`'s fold path at ~line 113) so a downstream
            // `{{ step.text }}` template resolves the same way on a
            // fresh run and on a cache-hit rerun. Previously this fell
            // back to `Value::String` and `{{ step.text }}` rendered
            // empty after a cache hit.
            let result_value = serde_json::from_str::<Value>(&content)
                .unwrap_or_else(|_| json!({"text": content}));
            state
                .results
                .insert(step_name.clone(), result_value.clone());
            // Intentionally don't write a step_hash entry — the next run
            // will probe the file again and either hit or miss; the
            // step-hash cache is orthogonal and shouldn't be poisoned
            // by a file-presence shortcut.
            update_render_context(&mut state, &step_name);
            events.push((
                "subrun_step_cache_hit".to_string(),
                json!({
                    "step": step_name,
                    "source": "file",
                    "path": rendered_path,
                }),
            ));
            events.push((
                "subrun_step_output".to_string(),
                json!({
                    "step": step_name,
                    "task_type": task.task_type.name(),
                    "output": result_value,
                }),
            ));
            events.push((
                "subrun_step_completed".to_string(),
                json!({ "step": step_name, "success": true, "cached": true, "cache_source": "file" }),
            ));
            state.current_step += 1;
            return (
                state,
                AutomationDecision::StepExecutedInline {
                    step_name,
                    emitted_events: events,
                },
            );
        }

        // ── Cache check (resume-unchanged-steps) ──────────────────────────
        //
        // If the user opted in (`cache_enabled`) and pointed at a prior run
        // (`retry_from_run_id`), compute this step's identity hash and reuse
        // the prior result when the hash matches. Because render_context
        // feeds into the hash, any upstream divergence cascades naturally:
        // the first miss perturbs render_context, every downstream hash
        // changes, every downstream step re-executes.
        let current_hash = match compute_step_hash(&StepHashInputs {
            step_config: &task,
            render_context: &state.render_context,
            variables: state.variables.as_ref(),
            // Loop iteration caching is handled inside the loop step itself
            // (not yet implemented at this granularity).
            loop_idx: None,
            // Sub-automation's child config hash is wired in a later patch —
            // for now we don't reuse cached results across child YAML edits.
            sub_workflow_yaml_hash: None,
        }) {
            Ok(h) => h,
            Err(e) => {
                return (
                    state,
                    AutomationDecision::Fail {
                        error: format!("hash error: {e}"),
                        emitted_events: events,
                    },
                );
            }
        };

        // A step listed in `invalidate_iterations` must skip the
        // step-level cache hit even when its identity hash matches,
        // otherwise the whole step short-circuits to "cached" and the
        // loop branch's per-iteration force-invalidate logic never
        // runs — the user clicks "force-retry iteration 7" and sees
        // the cached iteration 7 answer anyway. The loop branch below
        // still gets to reuse the non-forced iterations from the prior
        // snapshot, so we only forfeit cache for the specific indices
        // the user asked to re-run.
        let has_iteration_overrides = state
            .invalidate_iterations
            .get(&step_name)
            .is_some_and(|v| !v.is_empty());

        if state.cache_enabled
            && !has_iteration_overrides
            && let Some(prior) = prior_state
            && let Some(prior_hash) = prior.step_hashes.get(&step_name)
            && prior_hash == &current_hash
            && let Some(prior_result) = prior.results.get(&step_name).cloned()
        {
            state
                .results
                .insert(step_name.clone(), prior_result.clone());
            state
                .step_hashes
                .insert(step_name.clone(), current_hash.clone());
            update_render_context(&mut state, &step_name);
            events.push((
                "subrun_step_cache_hit".to_string(),
                json!({
                    "step": step_name,
                    "prior_run_id": prior.run_id,
                }),
            ));
            events.push((
                "subrun_step_output".to_string(),
                json!({
                    "step": step_name,
                    "task_type": task.task_type.name(),
                    "output": prior_result,
                }),
            ));
            events.push((
                "subrun_step_completed".to_string(),
                json!({ "step": step_name, "success": true, "cached": true }),
            ));
            state.current_step += 1;
            return (
                state,
                AutomationDecision::StepExecutedInline {
                    step_name,
                    emitted_events: events,
                },
            );
        }

        // Cache miss. The hash is recorded only alongside a successful
        // result (in the inline-success path below or in the fold phase for
        // delegated steps), never on its own. Persisting an orphan hash for
        // a failed step would let a future retry mistakenly accept the
        // failure answer as cached output.

        match classify_step(&state, &task.task_type) {
            StepKind::Inline => match execute_inline(&state, &task.task_type) {
                Ok(output) => {
                    state.results.insert(step_name.clone(), output.clone());
                    state.step_hashes.insert(step_name.clone(), current_hash);
                    update_render_context(&mut state, &step_name);
                    events.push((
                        "subrun_step_output".to_string(),
                        json!({
                            "step": step_name,
                            "task_type": task.task_type.name(),
                            "output": output,
                        }),
                    ));
                    events.push((
                        "subrun_step_completed".to_string(),
                        json!({ "step": step_name, "success": true }),
                    ));
                    state.current_step += 1;
                    (
                        state,
                        AutomationDecision::StepExecutedInline {
                            step_name,
                            emitted_events: events,
                        },
                    )
                }
                Err(e) => {
                    events.push((
                        "subrun_step_completed".to_string(),
                        json!({ "step": step_name, "success": false, "error": &e }),
                    ));
                    events.push((
                        "subrun_completed".to_string(),
                        json!({
                            "subrun_name": state.workflow.name,
                            "success": false,
                        }),
                    ));
                    (
                        state,
                        AutomationDecision::Fail {
                            error: e,
                            emitted_events: events,
                        },
                    )
                }
            },

            StepKind::Airway {
                pipeline_ref,
                resources,
            } => {
                // Straight to the existing airway task spec — no
                // `AutomationStep` wrapper, so the coordinator routes it to
                // `execute_airway` and it inherits secret resolution, the
                // Airhouse credential provider, backfill windowing and
                // run-scoped state. Backfill bounds stay `None`: a windowed
                // backfill is driven by the backfill path, not by an
                // automation step.
                //
                // `contract_policy`/`environment` come from the injected
                // [`AirwayAdmissionResolver`] port, so a step-triggered run
                // carries the same `airway_source_config` admission a
                // schedule- or HTTP-triggered run of the same pipeline gets
                // from `start_airway_run`. Resolved **here**, where the spec
                // is built and before the coordinator writes the child's
                // queue row, matching that site's resolve-at-enqueue
                // ordering: the queued spec records the policy this run was
                // admitted under, and stays explainable after the config
                // changes.
                let admission = match self.resolve_airway_admission(&pipeline_ref).await {
                    Ok(admission) => admission,
                    Err(e) => {
                        // Fail the step rather than queue it under a
                        // silently-defaulted `permissive` — see the port's
                        // doc comment.
                        let error = format!("airway admission for step {step_name}: {e}");
                        events.push((
                            "subrun_step_completed".to_string(),
                            json!({ "step": step_name, "success": false, "error": &error }),
                        ));
                        events.push((
                            "subrun_completed".to_string(),
                            json!({
                                "subrun_name": state.workflow.name,
                                "success": false,
                            }),
                        ));
                        return (
                            state,
                            AutomationDecision::Fail {
                                error,
                                emitted_events: events,
                            },
                        );
                    }
                };
                let spec = TaskSpec::Airway {
                    pipeline_ref,
                    variables: None,
                    resources,
                    backfill_from: None,
                    backfill_to: None,
                    contract_policy: admission.contract_policy,
                    environment: admission.environment,
                };
                (
                    state,
                    AutomationDecision::DelegateStep {
                        step_index,
                        step_name,
                        spec,
                        trace_id,
                        emitted_events: events,
                    },
                )
            }

            StepKind::Delegated => {
                let step_config =
                    serde_json::to_value(&task).unwrap_or_else(|_| json!({"name": step_name}));
                let spec = TaskSpec::AutomationStep {
                    step_config,
                    render_context: state.render_context.clone(),
                    workflow_context: state.workflow_context.clone(),
                };
                (
                    state,
                    AutomationDecision::DelegateStep {
                        step_index,
                        step_name,
                        spec,
                        trace_id,
                        emitted_events: events,
                    },
                )
            }

            StepKind::Agent {
                agent_ref,
                prompt,
                consistency_run,
                output_mode,
                ..
            } => {
                // Render the prompt against the parent context so
                // `{{ prior_step.col[0] }}` / `{{ prior_step }}`
                // references resolve to actual values before the LLM
                // sees them. Without this the agent receives the raw
                // template syntax and complains the data is missing.
                let prompt = match render_jinja_string(&prompt, &state.render_context) {
                    Ok(rendered) => rendered,
                    Err(err) => {
                        let err = format!("agent {step_name} prompt render: {err}");
                        events.push((
                            "subrun_step_completed".to_string(),
                            json!({ "step": step_name, "success": false, "error": &err }),
                        ));
                        events.push((
                            "subrun_completed".to_string(),
                            json!({
                                "subrun_name": state.workflow.name,
                                "success": false,
                            }),
                        ));
                        return (
                            state,
                            AutomationDecision::Fail {
                                error: err,
                                emitted_events: events,
                            },
                        );
                    }
                };
                // The `extra` envelope carries domain-opaque per-agent
                // params. Today it carries the analytics agent's
                // `output_mode` (when `output: { mode: sql }` is set);
                // empty otherwise.
                let extra = build_agent_extra(output_mode);
                // Consistency fan-outs share the same extra envelope —
                // the resolver pulls `extra` out of the per-item
                // context to build each TaskSpec::Agent.
                let agent_context = extra
                    .clone()
                    .map(|v| json!({ "extra": v }))
                    .unwrap_or_else(|| json!({}));
                if consistency_run > 1 {
                    let items = (0..consistency_run)
                        .map(|_| DelegationItem {
                            target: DelegationTarget::Agent {
                                agent_id: agent_ref.clone(),
                            },
                            request: prompt.clone(),
                            context: agent_context.clone(),
                        })
                        .collect();
                    (
                        state,
                        AutomationDecision::DelegateParallel {
                            step_index,
                            step_name,
                            items,
                            failure_policy: FanoutFailurePolicy::BestEffort,
                            trace_id,
                            emitted_events: events,
                        },
                    )
                } else {
                    let spec = TaskSpec::Agent {
                        agent_id: agent_ref,
                        question: prompt,
                        extra,
                    };
                    (
                        state,
                        AutomationDecision::DelegateStep {
                            step_index,
                            step_name,
                            spec,
                            trace_id,
                            emitted_events: events,
                        },
                    )
                }
            }

            StepKind::SubAutomation { src, variables } => {
                // Render the override map against the parent context so a
                // passthrough like `variables: { month: "{{ month }}" }`
                // resolves here instead of reaching the child verbatim.
                let variables = match crate::variables::render_override_variables(
                    variables.as_ref(),
                    &state.render_context,
                ) {
                    Ok(v) => v,
                    Err(err) => {
                        let err = format!("sub-automation {step_name}: {err}");
                        events.push((
                            "subrun_step_completed".to_string(),
                            json!({ "step": step_name, "success": false, "error": &err }),
                        ));
                        events.push((
                            "subrun_completed".to_string(),
                            json!({
                                "subrun_name": state.workflow.name,
                                "success": false,
                            }),
                        ));
                        return (
                            state,
                            AutomationDecision::Fail {
                                error: err,
                                emitted_events: events,
                            },
                        );
                    }
                };
                let spec = TaskSpec::Automation {
                    workflow_ref: src,
                    variables,
                    // Child sub-workflows always run fresh — cache linkage at
                    // child-run granularity is a v2 feature.
                    retry_from_run_id: None,
                    cache_enabled: false,
                    body: None,
                    initial_render_context: None,
                };
                (
                    state,
                    AutomationDecision::DelegateStep {
                        step_index,
                        step_name,
                        spec,
                        trace_id,
                        emitted_events: events,
                    },
                )
            }

            StepKind::Loop {
                values,
                tasks,
                concurrency,
            } => {
                let items_arr = match resolve_loop_values(&values, &state.render_context) {
                    Ok(items) => items,
                    Err(err) => {
                        let err = format!("loop {step_name}: {err}");
                        events.push((
                            "subrun_step_completed".to_string(),
                            json!({ "step": step_name, "success": false, "error": &err }),
                        ));
                        events.push((
                            "subrun_completed".to_string(),
                            json!({
                                "subrun_name": state.workflow.name,
                                "success": false,
                            }),
                        ));
                        return (
                            state,
                            AutomationDecision::Fail {
                                error: err,
                                emitted_events: events,
                            },
                        );
                    }
                };

                if items_arr.is_empty() {
                    state.results.insert(step_name.clone(), json!([]));
                    update_render_context(&mut state, &step_name);
                    events.push((
                        "subrun_step_output".to_string(),
                        json!({
                            "step": step_name,
                            "task_type": task.task_type.name(),
                            "output": [],
                        }),
                    ));
                    events.push((
                        "subrun_step_completed".to_string(),
                        json!({ "step": step_name, "success": true }),
                    ));
                    state.current_step += 1;
                    return (
                        state,
                        AutomationDecision::StepExecutedInline {
                            step_name,
                            emitted_events: events,
                        },
                    );
                }

                // Per-iteration cache: on a `cache_enabled` retry with a
                // prior run, look up each iteration's outcome by
                // `iteration_hash(value, index)`. Items whose prior status
                // is `"done"` skip delegation entirely; everything else
                // (failed, cancelled, missing, or first-run) gets
                // re-fanned-out. `cancelled` flows through as retryable
                // by design — a FailFast cancel didn't get its chance to
                // run.
                // Force-replay set: indices the caller listed in
                // `invalidate_iterations[step_name]`. Even with a "done"
                // prior outcome, these always re-delegate.
                let force_invalidate: std::collections::HashSet<usize> = state
                    .invalidate_iterations
                    .get(&step_name)
                    .map(|v| v.iter().copied().collect())
                    .unwrap_or_default();

                let (reusable_inline, reusable_iterations, to_delegate_indices): (
                    serde_json::Map<String, Value>,
                    serde_json::Map<String, Value>,
                    Vec<usize>,
                ) = if state.cache_enabled
                    && let Some(prior) = prior_state
                    && let Some(prior_iters) = prior
                        .results
                        .get(&step_name)
                        .and_then(|v| v.get("iterations"))
                        .and_then(|v| v.as_object())
                {
                    let mut reuse_inline = serde_json::Map::new();
                    let mut reuse_iters = serde_json::Map::new();
                    let mut to_delegate = Vec::new();
                    for (i, item) in items_arr.iter().enumerate() {
                        let key = iteration_hash(item, i);
                        let prior_entry = prior_iters.get(&key);
                        let is_done = !force_invalidate.contains(&i)
                            && prior_entry
                                .and_then(|e| e.get("status"))
                                .and_then(|s| s.as_str())
                                == Some("done");
                        if is_done && let Some(entry) = prior_entry {
                            let answer = entry.get("answer").cloned().unwrap_or(Value::Null);
                            reuse_inline.insert(
                                format!("inline-{i}"),
                                json!({ "status": "done", "answer": answer.clone(), "index": i }),
                            );
                            reuse_iters.insert(
                                key,
                                json!({
                                    "value": item,
                                    "index": i,
                                    "status": "done",
                                    "answer": answer,
                                }),
                            );
                        } else {
                            to_delegate.push(i);
                        }
                    }
                    (reuse_inline, reuse_iters, to_delegate)
                } else {
                    (
                        serde_json::Map::new(),
                        serde_json::Map::new(),
                        (0..items_arr.len()).collect(),
                    )
                };

                // All iterations cache-hit — synthesise the step's
                // completion inline and short-circuit the fan-out.
                if to_delegate_indices.is_empty() && !reusable_inline.is_empty() {
                    let mut combined = reusable_inline;
                    combined.insert("iterations".to_string(), Value::Object(reusable_iterations));
                    let combined_value = Value::Object(combined);
                    state
                        .results
                        .insert(step_name.clone(), combined_value.clone());
                    if let Ok(hash) = compute_step_hash(&StepHashInputs {
                        step_config: &task,
                        render_context: &state.render_context,
                        variables: state.variables.as_ref(),
                        loop_idx: None,
                        sub_workflow_yaml_hash: None,
                    }) {
                        state.step_hashes.insert(step_name.clone(), hash);
                    }
                    update_render_context(&mut state, &step_name);
                    events.push((
                        "subrun_step_cache_hit".to_string(),
                        json!({
                            "step": step_name,
                            "prior_run_id": prior_state.map(|p| p.run_id.clone()),
                            "iterations_reused": items_arr.len(),
                        }),
                    ));
                    events.push((
                        "subrun_step_output".to_string(),
                        json!({
                            "step": step_name,
                            "task_type": task.task_type.name(),
                            "output": combined_value,
                        }),
                    ));
                    events.push((
                        "subrun_step_completed".to_string(),
                        json!({ "step": step_name, "success": true, "cached": true }),
                    ));
                    state.current_step += 1;
                    return (
                        state,
                        AutomationDecision::StepExecutedInline {
                            step_name,
                            emitted_events: events,
                        },
                    );
                }

                // Partial cache hit: pre-seed reusable entries into the
                // step's result so the fold-time merge combines them with
                // the freshly-delegated subset's outcomes.
                if !reusable_inline.is_empty() {
                    let mut seed = reusable_inline.clone();
                    seed.insert(
                        "iterations".to_string(),
                        Value::Object(reusable_iterations.clone()),
                    );
                    state.results.insert(step_name.clone(), Value::Object(seed));
                }

                // Snapshot the render context once so all iterations share the
                // same base — each iteration only differs by its loop variable.
                let base_context = state.render_context.clone();
                let delegation_items: Vec<DelegationItem> = to_delegate_indices
                    .iter()
                    .map(|&i| {
                        let item = &items_arr[i];
                        let mut iter_context = base_context.clone();
                        if let Some(obj) = iter_context.as_object_mut() {
                            obj.insert(
                                step_name.clone(),
                                json!({ "value": item, "index": i }),
                            );
                            // Also expose `value` / `index` at top level so
                            // bare `{{ value }}` / `{{ index }}` (the
                            // idiomatic loop-body references) resolve
                            // without forcing authors to write the
                            // qualified `{{ <loop_step>.value }}` form.
                            // Nested loops: innermost wins (insert
                            // overwrites), matching standard templating
                            // semantics; the qualified form remains
                            // available for disambiguation.
                            obj.insert("value".to_string(), item.clone());
                            obj.insert("index".to_string(), json!(i));
                        }
                        DelegationItem {
                            target: DelegationTarget::Automation {
                                workflow_ref: "__workflow_step__".to_string(),
                            },
                            request: format!("{step_name}[{i}]"),
                            context: json!({
                                "step_config": { "name": format!("{step_name}_{i}"), "tasks": &tasks },
                                "render_context": iter_context,
                                "workflow_context": &state.workflow_context,
                                "loop_item": item,
                                "loop_index": i,
                                // `loop_step_name` is the *parent* loop's step
                                // name (vs `step_config.name` which is
                                // synthesized per iteration). The coordinator
                                // reads it at suspension time to stash on the
                                // child TaskNode so per-iteration completion
                                // events can carry the right `step` field
                                // without parsing `step_config.name`.
                                "loop_step_name": step_name,
                            }),
                        }
                    })
                    .collect();

                // Emit started+completed pairs for the cached iterations
                // first so they show up as `done` cells in the live
                // progress bar. The FE reducer only builds its
                // `LiveIteration[]` from these event pairs — it does not
                // peek at the pre-seeded `iterations` snapshot — so
                // without these synthetic events a partial-cache replay
                // (e.g. force-retry one iteration out of N) would show
                // only the delegated subset and look like 1/1 instead of
                // N/N. The all-cached path short-circuits earlier and
                // never reaches the bar, so it doesn't need this.
                //
                // The pair is in-order (started → completed) with
                // monotonic seqs, but both land in the same commit so
                // the FE flips them straight to `done` without a visible
                // running flash.
                let mut cached_emit: Vec<(usize, Value, Value)> = reusable_iterations
                    .values()
                    .filter_map(|entry| {
                        let idx = entry.get("index").and_then(Value::as_u64)? as usize;
                        let value = entry.get("value").cloned().unwrap_or(Value::Null);
                        let answer = entry.get("answer").cloned().unwrap_or(Value::Null);
                        Some((idx, value, answer))
                    })
                    .collect();
                cached_emit.sort_by_key(|(i, _, _)| *i);
                for (i, value, _answer) in &cached_emit {
                    events.push((
                        "subrun_step_iteration_started".to_string(),
                        json!({ "step": step_name, "index": i, "value": value }),
                    ));
                    events.push((
                        "subrun_step_iteration_completed".to_string(),
                        json!({ "step": step_name, "index": i, "status": "done" }),
                    ));
                }

                // Emit `iteration_started` per delegated iteration so the
                // FE can flip those cells to `running` immediately; the
                // coordinator emits `iteration_completed` per child as
                // results land (see runtime::coordinator::fanout).
                for &i in &to_delegate_indices {
                    events.push((
                        "subrun_step_iteration_started".to_string(),
                        json!({
                            "step": step_name,
                            "index": i,
                            "value": items_arr[i],
                        }),
                    ));
                }

                let failure_policy = if concurrency > 1 && items_arr.len() > 1 {
                    FanoutFailurePolicy::FailFast
                } else {
                    FanoutFailurePolicy::BestEffort
                };

                (
                    state,
                    AutomationDecision::DelegateParallel {
                        step_index,
                        step_name,
                        items: delegation_items,
                        failure_policy,
                        trace_id,
                        emitted_events: events,
                    },
                )
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

enum StepKind {
    Inline,
    Delegated,
    /// Delegate to the coordinator as a `TaskSpec::Airway`.
    ///
    /// Not `Delegated`: that wraps the step as an opaque `AutomationStep`
    /// routed through `step_executor`, which only sees a `WorkspaceContext`
    /// (no `DatabaseConnection`) and returns `Result<Value, String>` rather
    /// than the streaming handle an airway run needs. Emitting the existing
    /// `TaskSpec::Airway` reuses the working path instead.
    Airway {
        pipeline_ref: String,
        resources: Vec<String>,
    },
    Agent {
        agent_ref: String,
        prompt: String,
        consistency_run: usize,
        #[allow(dead_code)]
        consistency_prompt: Option<String>,
        /// Output mode for the analytics agent path. When set to
        /// `"sql"`, the analytics FSM terminates after producing SQL
        /// instead of running execute + interpret.
        output_mode: Option<&'static str>,
    },
    SubAutomation {
        src: String,
        variables: Option<Value>,
    },
    Loop {
        values: Value,
        tasks: Value,
        concurrency: usize,
    },
}

fn classify_step(state: &AutomationRunState, task_type: &TaskType) -> StepKind {
    match task_type {
        TaskType::Formatter(_) | TaskType::Conditional(_) => StepKind::Inline,

        TaskType::Agent(agent_task) => StepKind::Agent {
            agent_ref: agent_task.agent_ref.clone(),
            prompt: agent_task.prompt.clone(),
            consistency_run: agent_task.consistency_run,
            consistency_prompt: agent_task
                .consistency_prompt
                .clone()
                .or_else(|| state.workflow.consistency_prompt.clone()),
            output_mode: agent_task.output.as_ref().map(|o| match o.mode {
                crate::config::AgentOutputMode::Answer => "answer",
                crate::config::AgentOutputMode::Sql => "sql",
            }),
        },

        TaskType::SubAutomation(wf_task) => StepKind::SubAutomation {
            src: wf_task.src.to_string_lossy().to_string(),
            variables: wf_task
                .variables
                .as_ref()
                .map(|v| serde_json::to_value(v).unwrap_or_default()),
        },

        TaskType::LoopSequential(loop_task) => StepKind::Loop {
            values: serde_json::to_value(&loop_task.values).unwrap_or_default(),
            tasks: serde_json::to_value(&loop_task.tasks).unwrap_or_default(),
            concurrency: loop_task.concurrency,
        },

        TaskType::Airway(cfg) => StepKind::Airway {
            pipeline_ref: cfg.pipeline.clone(),
            resources: cfg.resources.clone().unwrap_or_default(),
        },

        TaskType::ExecuteSql(_)
        | TaskType::SemanticQuery(_)
        | TaskType::OmniQuery(_)
        | TaskType::LookerQuery(_)
        | TaskType::HttpRequest(_)
        | TaskType::Unknown => StepKind::Delegated,
    }
}

/// Build the `extra` envelope for a `TaskSpec::Agent`. Returns `None` when
/// no per-agent knobs are set, so vanilla agent tasks keep the wire
/// shape they had before the envelope existed.
pub(crate) fn build_agent_extra(output_mode: Option<&'static str>) -> Option<Value> {
    let mode = output_mode?;
    let mut map = serde_json::Map::new();
    map.insert("output_mode".to_string(), Value::String(mode.to_string()));
    Some(Value::Object(map))
}

fn execute_inline(state: &AutomationRunState, task_type: &TaskType) -> Result<Value, String> {
    match task_type {
        TaskType::Formatter(fmt) => execute_formatter(&state.render_context, &fmt.template),
        TaskType::Conditional(cond) => execute_conditional(&state.render_context, cond),
        _ => Err("not an inline step".to_string()),
    }
}

fn execute_formatter(render_context: &Value, template: &str) -> Result<Value, String> {
    let env = crate::render::automation_env();
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| format!("template parse error: {e}"))?;
    let ctx = build_minijinja_context(render_context);
    let rendered = tmpl
        .render(&ctx)
        .map_err(|e| format!("template render error: {e}"))?;
    Ok(json!({ "text": rendered }))
}

fn execute_conditional(
    render_context: &Value,
    cond: &crate::config::ConditionalConfig,
) -> Result<Value, String> {
    let env = crate::render::automation_env();
    let ctx = build_minijinja_context(render_context);

    for branch in &cond.conditions {
        if crate::render::condition_holds(&env, &branch.condition, &ctx)? {
            let task_names: Vec<String> = branch.tasks.iter().map(|t| t.name.clone()).collect();
            return Ok(json!({
                "branch": "matched",
                "condition": branch.condition,
                "tasks": task_names,
            }));
        }
    }
    if let Some(else_tasks) = &cond.else_tasks {
        let task_names: Vec<String> = else_tasks.iter().map(|t| t.name.clone()).collect();
        Ok(json!({ "branch": "else", "tasks": task_names }))
    } else {
        Ok(json!({ "branch": "none_matched" }))
    }
}

/// Insert a single completed step result into the render context.
///
/// Called instead of a full rebuild so that each step costs O(1) rather than
/// O(accumulated_results), eliminating the O(S²) scaling that hit nested/loop
/// workflows with many steps or large intermediate result sets.
/// Combine a freshly-folded loop step's aggregated answer with any
/// pre-seeded reusable entries (from per-iteration cache hits at
/// decide time) and stamp every entry with an `iteration_hash`-keyed
/// `iterations` map.
///
/// Non-loop steps: returns `answer_value` unchanged. Loop steps with a
/// non-literal `values` (Jinja expression) that fails to resolve at
/// fold time: also returns unchanged — the cache attribution silently
/// no-ops for that run rather than blocking the fold.
///
/// The resulting shape stored in `state.results[step]`:
/// ```json
/// {
///   "<child_id_or_inline_N>": { "status": "done", "answer": "…", "index": 0 },
///   "<child_id_or_inline_N>": { "status": "failed", "error": "…", "index": 1 },
///   "iterations": {
///     "<hash(value_0, 0)>": { "value": …, "index": 0, "status": "done", "answer": "…" },
///     "<hash(value_1, 1)>": { "value": …, "index": 1, "status": "failed", "error": "…" }
///   }
/// }
/// ```
/// One iteration whose outcome just landed in the loop's `iterations` map.
/// Returned by [`fold_loop_step_result`] so the caller can emit
/// `subrun_step_iteration_completed` events at exactly the same
/// granularity the FE's live progress bar needs.
#[derive(Debug)]
pub(crate) struct FoldedIteration {
    pub index: usize,
    pub status: String,
    pub error: Option<String>,
}

fn fold_loop_step_result(
    automation: &crate::config::AutomationConfig,
    render_context: &Value,
    step_index: usize,
    answer_value: Value,
    existing: Option<&Value>,
) -> (Value, Vec<FoldedIteration>) {
    let Some(loop_cfg) = automation
        .tasks
        .get(step_index)
        .and_then(|t| match &t.task_type {
            TaskType::LoopSequential(cfg) => Some(cfg),
            _ => None,
        })
    else {
        return (answer_value, Vec::new());
    };
    // We need the resolved values list to map an entry's `index` back to
    // its (value, index) hash. The values field can be a Jinja expression;
    // re-resolve against the same `render_context` the decider used at
    // delegate time. The state hasn't yet had this step's render-context
    // update applied (that happens *after* fold).
    let Ok(items) = resolve_loop_values(&loop_cfg.values, render_context) else {
        return (answer_value, Vec::new());
    };

    let Some(mut new_obj) = answer_value.as_object().cloned() else {
        return (answer_value, Vec::new());
    };

    // Start the iterations map from any existing pre-seeded entries
    // (reusable cache hits at decide time) so cache hits survive the fold.
    let mut iterations = existing
        .and_then(|v| v.get("iterations"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    // Stamp fresh entries: each carries `index` (added by the coordinator /
    // inline fan-out aggregator); resolve `value` from the items list and
    // attribute the outcome under `iteration_hash(value, index)`. Each
    // fresh entry also produces one `FoldedIteration` so the caller can
    // emit a live `subrun_step_iteration_completed` event.
    let mut folded: Vec<FoldedIteration> = Vec::new();
    for (_id, entry) in &new_obj {
        let Some(i) = entry.get("index").and_then(|v| v.as_u64()) else {
            continue;
        };
        let i = i as usize;
        let Some(value) = items.get(i) else { continue };
        let key = iteration_hash(value, i);
        let mut iter_entry = entry.clone();
        let status = entry
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("done")
            .to_string();
        let error = entry
            .get("error")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(obj) = iter_entry.as_object_mut() {
            obj.insert("value".to_string(), value.clone());
            obj.entry("index".to_string()).or_insert(Value::from(i));
        }
        iterations.insert(key, iter_entry);
        folded.push(FoldedIteration {
            index: i,
            status,
            error,
        });
    }

    // Pre-seeded `inline-N` / child-id entries from cache hits must
    // survive the fold: the fresh answer doesn't cover them.
    if let Some(Value::Object(existing_obj)) = existing.cloned() {
        for (k, v) in existing_obj {
            if k == "iterations" {
                continue;
            }
            new_obj.entry(k).or_insert(v);
        }
    }

    new_obj.insert("iterations".to_string(), Value::Object(iterations));
    (Value::Object(new_obj), folded)
}

/// Stable per-iteration cache key for a `loop_sequential` step.
///
/// Composed from the iteration's resolved value and its position in the
/// fan-out. Position guards against duplicate values (`[1, 1, 1]` ⇒ three
/// distinct keys); positional binding also means that reordering the
/// `values:` list in the YAML invalidates the cache for moved entries.
/// That's an intentional, predictable trade-off: editing a list to
/// reorder is a strong signal that the workflow author wants those
/// iterations re-run.
///
/// Used at two points:
/// - Loop-step decide time on retry: hash the new run's items, look up
///   `prior.results[step]["iterations"][hash]`; status `"done"` ⇒ reuse
///   the prior answer, status `"failed"` / `"cancelled"` ⇒ re-delegate.
/// - Fold time after a fan-out: stamp each aggregated entry with its
///   `iteration_hash` so the *next* run can find it.
fn iteration_hash(value: &Value, index: usize) -> String {
    // Re-use the canonical hash helper so iteration hashes share the
    // same SHA-256-over-RFC-8785-JSON discipline as step hashes — no
    // separate encoding rules to keep in sync.
    crate::hash::canonical_hash(&json!([value, index])).unwrap_or_default()
}

/// Resolve a `loop_sequential` step's `values` field to a concrete JSON array.
///
/// `LoopConfig.values` is `serde_json::Value` to accept either:
/// - a JSON array literal (e.g. `[1, 2, 3]`) — used directly, or
/// - a Jinja expression (e.g. `"{{ intervals.intervals }}"`) — compiled
///   against the current render context and evaluated to a sequence.
///
/// Returning `Err` produces the user-facing `loop {step}: …` error the
/// caller already wraps; we don't add the step prefix here so the same
/// helper can be unit-tested in isolation.
fn resolve_loop_values(values: &Value, render_context: &Value) -> Result<Vec<Value>, String> {
    if let Some(arr) = values.as_array() {
        return Ok(arr.clone());
    }
    let Some(template) = values.as_str() else {
        return Err(format!(
            "values must be an array or Jinja template string, got: {values}"
        ));
    };

    // Strip an outer `{{ … }}` wrapper so we can compile the inner
    // expression directly — matches the legacy `Renderer::eval_expression`
    // shape and lets us return a typed Value rather than a stringified
    // render output.
    let trimmed = template.trim();
    let inner = trimmed
        .strip_prefix("{{")
        .and_then(|s| s.strip_suffix("}}"))
        .map(str::trim)
        .unwrap_or(trimmed);

    let env = crate::render::automation_env_strict();
    let expr = env
        .compile_expression(inner)
        .map_err(|e| format!("invalid loop values expression {template:?}: {e}"))?;
    let ctx = build_minijinja_context(render_context);
    let rendered = expr.eval(&ctx).map_err(|e| {
        let available: Vec<String> = render_context
            .as_object()
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        format!("eval loop values {template:?}: {e}\nAvailable keys: {available:?}")
    })?;

    let json_value: Value = serde_json::to_value(rendered)
        .map_err(|e| format!("loop values {template:?} did not serialise to JSON: {e}"))?;
    match json_value {
        Value::Array(items) => Ok(items),
        other => Err(format!(
            "loop values {template:?} did not resolve to an array (got {other})"
        )),
    }
}

/// Resolve a `TaskConfig.cache.path` against the step's render context
/// and the workspace root, then return `(rendered_path, content)` if
/// the file exists. Missing file, render failure, traversal-unsafe
/// path, or I/O error all collapse to `None` — the caller treats any
/// of those as a cache miss and runs the step normally.
///
/// I/O is real but the file is expected to be small (SQL text, JSON
/// blob); blocking the decide task for one stat + one read is fine.
async fn probe_file_cache(
    workspace_path: &Path,
    cache_path_template: &str,
    render_context: &Value,
) -> Option<(String, String)> {
    let rendered = render_jinja_string(cache_path_template, render_context).ok()?;
    let abs = validate_workspace_relative_path(workspace_path, &rendered).ok()?;
    match tokio::fs::read_to_string(&abs).await {
        Ok(content) => Some((rendered, content)),
        Err(_) => None,
    }
}

fn update_render_context(state: &mut AutomationRunState, step_name: &str) {
    let Some(value) = state.results.get(step_name) else {
        return;
    };
    let context_value = to_column_oriented(value);
    if let Some(obj) = state.render_context.as_object_mut() {
        obj.insert(step_name.to_string(), context_value);
    } else {
        let mut map = serde_json::Map::new();
        map.insert(step_name.to_string(), context_value);
        state.render_context = Value::Object(map);
    }
}

/// Build a workflow's terminal answer — the JSON string a parent
/// receives when it `type: workflow`-delegates to this run.
///
/// Shape: `{task_name: result, ...}` keyed by task name in declaration
/// order. Matches the legacy `oxy-workflow` `OutputContainer::Map(...)`
/// shape produced by `TaskChainMapper::map_reduce` (`memo.merge({task.name: value})`),
/// so a parent's template can keep referencing sub-results as
/// `{{ sub_step.inner_task.text }}` — same lookup pattern as accessing
/// the parent's own step results.
///
/// Was a `Vec<Value>` previously, which made every parent template
/// indexing by task name fail silently (template chainable-undefined
/// behavior turned `{{ sub.report.text }}` into empty).
fn build_final_answer(state: &AutomationRunState) -> String {
    let mut out = serde_json::Map::with_capacity(state.workflow.tasks.len());
    for task in &state.workflow.tasks {
        if let Some(v) = state.results.get(&task.name) {
            out.insert(task.name.clone(), v.clone());
        }
    }
    serde_json::to_string(&Value::Object(out)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod final_answer_tests {
    use super::*;
    use crate::config::{AutomationConfig, FormatterConfig, TaskConfig, TaskType};
    use std::collections::HashMap;

    fn wf(task_names: &[&str]) -> AutomationConfig {
        AutomationConfig {
            name: "wf".into(),
            description: String::new(),
            tasks: task_names
                .iter()
                .map(|n| TaskConfig {
                    name: (*n).to_string(),
                    task_type: TaskType::Formatter(FormatterConfig {
                        template: String::new(),
                    }),
                    export: None,
                    cache: None,
                })
                .collect(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        }
    }

    fn state_with(automation: AutomationConfig, results: Vec<(&str, Value)>) -> AutomationRunState {
        let mut map = HashMap::new();
        for (k, v) in results {
            map.insert(k.to_string(), v);
        }
        AutomationRunState {
            run_id: "r".into(),
            workflow: automation,
            workflow_yaml_hash: "h".into(),
            workflow_context: json!({}),
            variables: None,
            trace_id: "t".into(),
            current_step: 0,
            results: map,
            render_context: json!({}),
            pending_children: HashMap::new(),
            decision_version: 0,
            step_hashes: HashMap::new(),
            retry_from_run_id: None,
            cache_enabled: false,
            prior_step_hashes: HashMap::new(),
            prior_results: HashMap::new(),
            initial_render_context: json!({}),
            invalidate_iterations: HashMap::new(),
        }
    }

    /// Sub-workflow output is an OBJECT keyed by task name — mirrors
    /// the legacy `oxy-workflow` `OutputContainer::Map` shape so a
    /// parent's template can reference `{{ child.task_a.text }}`.
    /// Regression: was previously a `Vec<Value>` and parent templates
    /// silently rendered empty.
    #[test]
    fn final_answer_is_object_keyed_by_task_name() {
        let s = state_with(
            wf(&["query", "report"]),
            vec![
                ("query", json!({"text": "SELECT 1"})),
                ("report", json!({"text": "ok"})),
            ],
        );
        let parsed: Value = serde_json::from_str(&build_final_answer(&s)).unwrap();
        assert_eq!(
            parsed,
            json!({
                "query": {"text": "SELECT 1"},
                "report": {"text": "ok"},
            })
        );
    }

    /// Steps without a recorded result are omitted (matches legacy
    /// `filter_map` skip-on-missing behaviour).
    #[test]
    fn final_answer_skips_steps_without_results() {
        let s = state_with(wf(&["a", "b"]), vec![("a", json!("done"))]);
        let parsed: Value = serde_json::from_str(&build_final_answer(&s)).unwrap();
        assert_eq!(parsed, json!({"a": "done"}));
    }

    /// Empty results → empty object (was `"[]"` previously).
    #[test]
    fn final_answer_empty_state_is_empty_object() {
        let s = state_with(wf(&["a"]), vec![]);
        assert_eq!(build_final_answer(&s), "{}");
    }
}

#[cfg(test)]
mod loop_values_tests {
    use super::resolve_loop_values;
    use serde_json::{Value, json};

    #[test]
    fn passes_through_literal_array() {
        let resolved = resolve_loop_values(&json!([1, 2, 3]), &json!({})).unwrap();
        assert_eq!(resolved, vec![json!(1), json!(2), json!(3)]);
    }

    #[test]
    fn renders_jinja_expression_against_render_context() {
        let ctx = json!({ "intervals": { "intervals": ["daily", "weekly", "monthly"] } });
        let resolved = resolve_loop_values(&json!("{{ intervals.intervals }}"), &ctx).unwrap();
        assert_eq!(
            resolved,
            vec![json!("daily"), json!("weekly"), json!("monthly")]
        );
    }

    #[test]
    fn accepts_bare_expression_without_braces() {
        // The legacy renderer stripped `{{ … }}` before compiling; we
        // tolerate both shapes so a workflow author can paste either form.
        let ctx = json!({ "items": [10, 20] });
        let resolved = resolve_loop_values(&json!("items"), &ctx).unwrap();
        assert_eq!(resolved, vec![json!(10), json!(20)]);
    }

    #[test]
    fn rejects_non_array_non_string() {
        let err = resolve_loop_values(&Value::Bool(true), &json!({})).unwrap_err();
        assert!(err.contains("array or Jinja template"));
    }

    #[test]
    fn rejects_expression_that_does_not_resolve_to_array() {
        let err =
            resolve_loop_values(&json!("{{ name }}"), &json!({ "name": "scalar" })).unwrap_err();
        assert!(err.contains("did not resolve to an array"), "got: {err}");
    }

    #[test]
    fn surfaces_available_keys_when_expression_fails() {
        let err = resolve_loop_values(
            &json!("{{ missing.nested }}"),
            &json!({ "intervals": {}, "other": [] }),
        )
        .unwrap_err();
        assert!(err.contains("Available keys"), "got: {err}");
    }
}

#[cfg(test)]
mod iteration_cache_tests {
    use super::{FoldedIteration, fold_loop_step_result, iteration_hash};
    use crate::config::{AutomationConfig, LoopConfig, TaskConfig, TaskType};
    use serde_json::{Value, json};

    fn automation_with_loop(values: Value) -> AutomationConfig {
        AutomationConfig {
            name: "wf".into(),
            tasks: vec![TaskConfig {
                name: "items".into(),
                task_type: TaskType::LoopSequential(LoopConfig {
                    values,
                    tasks: vec![],
                    concurrency: 1,
                }),
                export: None,
                cache: None,
            }],
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        }
    }

    #[test]
    fn iteration_hash_is_stable_for_same_value_and_index() {
        let h1 = iteration_hash(&json!(42), 0);
        let h2 = iteration_hash(&json!(42), 0);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn iteration_hash_differs_by_index_for_duplicate_values() {
        // The whole point of including index in the key.
        let a = iteration_hash(&json!(1), 0);
        let b = iteration_hash(&json!(1), 1);
        let c = iteration_hash(&json!(1), 2);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn iteration_hash_differs_by_value_at_same_index() {
        let a = iteration_hash(&json!("one"), 0);
        let b = iteration_hash(&json!("two"), 0);
        assert_ne!(a, b);
    }

    #[test]
    fn fold_builds_iterations_map_for_loop_step() {
        let wf = automation_with_loop(json!([10, 20, 30]));
        let render_context = json!({});
        // Fresh fan-out (no pre-seeded entries): three children all done.
        let answer = json!({
            "task.1": { "status": "done", "answer": "ten",    "index": 0 },
            "task.2": { "status": "failed", "error": "boom",  "index": 1 },
            "task.3": { "status": "done", "answer": "thirty", "index": 2 },
        });
        let (folded, folded_iters) = fold_loop_step_result(&wf, &render_context, 0, answer, None);
        let iterations = folded
            .get("iterations")
            .and_then(|v| v.as_object())
            .expect("iterations map");
        assert_eq!(iterations.len(), 3);

        // Each freshly-folded iteration is also surfaced as a
        // FoldedIteration so the caller can emit a live
        // `subrun_step_iteration_completed` event per entry.
        assert_eq!(folded_iters.len(), 3);
        let by_idx: std::collections::HashMap<usize, &FoldedIteration> =
            folded_iters.iter().map(|f| (f.index, f)).collect();
        assert_eq!(by_idx[&0].status, "done");
        assert!(by_idx[&0].error.is_none());
        assert_eq!(by_idx[&1].status, "failed");
        assert_eq!(by_idx[&1].error.as_deref(), Some("boom"));
        assert_eq!(by_idx[&2].status, "done");

        let h0 = iteration_hash(&json!(10), 0);
        let h1 = iteration_hash(&json!(20), 1);
        let h2 = iteration_hash(&json!(30), 2);

        assert_eq!(iterations[&h0]["status"], "done");
        assert_eq!(iterations[&h0]["value"], json!(10));
        assert_eq!(iterations[&h0]["answer"], "ten");

        assert_eq!(iterations[&h1]["status"], "failed");
        assert_eq!(iterations[&h1]["error"], "boom");
        assert_eq!(iterations[&h1]["value"], json!(20));

        assert_eq!(iterations[&h2]["status"], "done");
        assert_eq!(iterations[&h2]["answer"], "thirty");
    }

    #[test]
    fn fold_merges_pre_seeded_reusable_entries() {
        // Simulates: prior run had 3 items, item 0 cache-hit at decide
        // time (pre-seeded as `inline-0`), items 1 and 2 got delegated.
        // The fold's answer covers only items 1 and 2; the pre-seeded
        // `inline-0` must survive.
        let wf = automation_with_loop(json!([10, 20, 30]));
        let render_context = json!({});

        let h0 = iteration_hash(&json!(10), 0);
        let pre_seeded = json!({
            "inline-0": { "status": "done", "answer": "cached", "index": 0 },
            "iterations": {
                h0.clone(): { "value": 10, "index": 0, "status": "done", "answer": "cached" }
            }
        });

        let fresh_answer = json!({
            "task.7": { "status": "done", "answer": "new-20", "index": 1 },
            "task.8": { "status": "done", "answer": "new-30", "index": 2 },
        });

        let (folded, folded_iters) =
            fold_loop_step_result(&wf, &render_context, 0, fresh_answer, Some(&pre_seeded));
        // Only the two fresh entries should be reported as folded;
        // the pre-seeded inline-0 stays in the map but isn't a *new*
        // iteration completion this call.
        assert_eq!(folded_iters.len(), 2);
        let obj = folded.as_object().expect("object result");

        // Pre-seeded inline entry survived alongside fresh task entries.
        assert!(obj.contains_key("inline-0"));
        assert!(obj.contains_key("task.7"));
        assert!(obj.contains_key("task.8"));

        let iterations = obj["iterations"].as_object().unwrap();
        assert_eq!(iterations.len(), 3);
        assert_eq!(iterations[&h0]["answer"], "cached");
        assert_eq!(
            iterations[&iteration_hash(&json!(20), 1)]["answer"],
            "new-20"
        );
    }

    #[test]
    fn fold_returns_unchanged_for_non_loop_step() {
        let wf = AutomationConfig {
            name: "wf".into(),
            tasks: vec![TaskConfig {
                name: "fmt".into(),
                task_type: TaskType::Formatter(crate::config::FormatterConfig {
                    template: "x".into(),
                }),
                export: None,
                cache: None,
            }],
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
        };
        let answer = json!({"text": "rendered"});
        let (folded, folded_iters) =
            fold_loop_step_result(&wf, &json!({}), 0, answer.clone(), None);
        assert_eq!(folded, answer);
        assert!(folded_iters.is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::config::{AutomationConfig, FormatterConfig, TaskConfig};
    use std::collections::HashMap;

    fn formatter_automation(name: &str, template: &str) -> AutomationConfig {
        AutomationConfig {
            name: "test-wf".into(),
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
            tasks: vec![TaskConfig {
                name: name.into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: template.into(),
                }),
                export: None,
                cache: None,
            }],
        }
    }

    fn fresh_state(
        automation: AutomationConfig,
        cache_enabled: bool,
        retry_from: Option<&str>,
    ) -> AutomationRunState {
        AutomationRunState {
            run_id: "current".into(),
            workflow: automation,
            workflow_yaml_hash: "h".into(),
            workflow_context: json!({}),
            variables: None,
            trace_id: "trace".into(),
            current_step: 0,
            results: HashMap::new(),
            render_context: json!({}),
            pending_children: HashMap::new(),
            decision_version: 0,
            step_hashes: HashMap::new(),
            retry_from_run_id: retry_from.map(str::to_string),
            cache_enabled,
            prior_step_hashes: HashMap::new(),
            prior_results: HashMap::new(),
            initial_render_context: serde_json::json!({}),
            invalidate_iterations: HashMap::new(),
        }
    }

    /// When the prior run has a matching hash, the decider must reuse the
    /// prior result and emit a `subrun_step_cache_hit` event.
    #[tokio::test]
    async fn cache_hit_reuses_prior_result() {
        let wf = formatter_automation("greet", "hi");
        let mut prior = fresh_state(wf.clone(), false, None);
        prior.run_id = "prior-run".into();

        // Prime "prior" with a successful execution: insert result and the
        // hash that would have been computed at that step.
        let task = wf.tasks[0].clone();
        let prior_hash = compute_step_hash(&StepHashInputs {
            step_config: &task,
            render_context: &prior.render_context,
            variables: prior.variables.as_ref(),
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        prior.results.insert("greet".into(), json!({"text": "hi"}));
        prior.step_hashes.insert("greet".into(), prior_hash);

        let current = fresh_state(wf, true, Some("prior-run"));
        let decider = AutomationDecider::new(None);
        let (new_state, decision) = decider.decide(current, None, Some(&prior), None).await;

        assert!(matches!(
            decision,
            AutomationDecision::StepExecutedInline { .. }
        ));
        assert_eq!(new_state.results.get("greet"), Some(&json!({"text": "hi"})));
        let event_types: Vec<&str> = match &decision {
            AutomationDecision::StepExecutedInline { emitted_events, .. } => {
                emitted_events.iter().map(|(t, _)| t.as_str()).collect()
            }
            _ => panic!("expected StepExecutedInline"),
        };
        assert!(event_types.contains(&"subrun_step_cache_hit"));
    }

    /// `invalidate_iterations[step]` must bypass the step-level cache
    /// short-circuit so the loop branch's per-iteration force-invalidate
    /// logic actually runs. Without this, the user clicks "force-retry
    /// iteration 7" but the step's identity hash matches the prior run
    /// and the entire step short-circuits to `cached`, leaving iteration 7
    /// pinned to the stale answer.
    #[tokio::test]
    async fn iteration_overrides_bypass_step_level_cache_hit() {
        let wf = formatter_automation("greet", "hi");
        let mut prior = fresh_state(wf.clone(), false, None);
        prior.run_id = "prior-run".into();
        let prior_hash = compute_step_hash(&StepHashInputs {
            step_config: &wf.tasks[0],
            render_context: &prior.render_context,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        prior.results.insert("greet".into(), json!({"text": "hi"}));
        prior.step_hashes.insert("greet".into(), prior_hash);

        let mut current = fresh_state(wf, true, Some("prior-run"));
        current
            .invalidate_iterations
            .insert("greet".into(), vec![0]);
        let decider = AutomationDecider::new(None);
        let (_, decision) = decider.decide(current, None, Some(&prior), None).await;

        if let AutomationDecision::StepExecutedInline { emitted_events, .. } = &decision {
            assert!(
                !emitted_events
                    .iter()
                    .any(|(t, _)| t == "subrun_step_cache_hit"),
                "iteration override must skip step-level cache hit",
            );
        }
    }

    /// File-presence cache short-circuit: when `cache.path` exists,
    /// the step is skipped and the file contents become the result.
    /// The whole point of this path is preserving manual edits, so
    /// the test writes a deliberately different string into the file
    /// and asserts the step's result matches the file (not the
    /// formatter's actual output).
    #[tokio::test]
    async fn file_cache_hit_short_circuits_step() {
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        let cache_path = "cached.txt";
        let abs_cache = tmpdir.path().join(cache_path);
        // User-edited contents — deliberately different from what the
        // formatter would render, to prove the file wins.
        std::fs::write(&abs_cache, "USER-EDITED-CONTENT").expect("seed cache");

        let wf = AutomationConfig {
            name: "test-wf".into(),
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
            tasks: vec![TaskConfig {
                name: "greet".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: "hello".into(),
                }),
                export: None,
                cache: Some(crate::config::CacheConfig {
                    enabled: true,
                    path: cache_path.into(),
                }),
            }],
        };

        let current = fresh_state(wf, false, None);
        let decider = AutomationDecider::new(None);
        let (new_state, decision) = decider
            .decide(current, None, None, Some(tmpdir.path()))
            .await;

        // Result is the file content wrapped in `{"text": ...}` so it
        // matches the fold-step shape — keeps `{{ step.text }}` working
        // identically on fresh runs and on cache-hit reruns.
        assert_eq!(
            new_state.results.get("greet"),
            Some(&json!({"text": "USER-EDITED-CONTENT"}))
        );

        // subrun_step_cache_hit with source=file is emitted.
        let events = match &decision {
            AutomationDecision::StepExecutedInline { emitted_events, .. } => emitted_events,
            other => panic!("expected StepExecutedInline, got {other:?}"),
        };
        let cache_hit = events
            .iter()
            .find(|(t, _)| t == "subrun_step_cache_hit")
            .expect("file cache hit event");
        assert_eq!(
            cache_hit.1.get("source").and_then(|v| v.as_str()),
            Some("file")
        );
    }

    /// A file cache that already contains a JSON-shaped payload skips
    /// the `{"text": ...}` wrap and uses the parsed JSON directly — so
    /// a step that previously emitted structured output round-trips
    /// without nesting under `text`.
    #[tokio::test]
    async fn file_cache_hit_parses_json_directly() {
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        let cache_path = "structured.json";
        let abs_cache = tmpdir.path().join(cache_path);
        std::fs::write(&abs_cache, r#"{"sql": "SELECT 1", "verified": true}"#).expect("seed cache");

        let wf = AutomationConfig {
            name: "test-wf".into(),
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
            tasks: vec![TaskConfig {
                name: "gen".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: "ignored".into(),
                }),
                export: None,
                cache: Some(crate::config::CacheConfig {
                    enabled: true,
                    path: cache_path.into(),
                }),
            }],
        };

        let current = fresh_state(wf, false, None);
        let decider = AutomationDecider::new(None);
        let (new_state, _) = decider
            .decide(current, None, None, Some(tmpdir.path()))
            .await;

        // Parsed JSON object lands as-is — no extra `{"text": ...}`
        // wrapping, so callers can reach into `{{ step.sql }}` directly.
        assert_eq!(
            new_state.results.get("gen"),
            Some(&json!({"sql": "SELECT 1", "verified": true}))
        );
    }

    /// File cache miss: the agent step runs normally. (Here we
    /// substitute formatter for "the step ran" since the formatter
    /// path executes inline.)
    #[tokio::test]
    async fn file_cache_miss_runs_step_normally() {
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        // Intentionally don't write a file at this path → miss.
        let wf = AutomationConfig {
            name: "test-wf".into(),
            description: String::new(),
            variables: None,
            consistency_prompt: None,
            consistency_model: None,
            tasks: vec![TaskConfig {
                name: "greet".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: "hello".into(),
                }),
                export: None,
                cache: Some(crate::config::CacheConfig {
                    enabled: true,
                    path: "missing.txt".into(),
                }),
            }],
        };
        let current = fresh_state(wf, false, None);
        let decider = AutomationDecider::new(None);
        let (new_state, decision) = decider
            .decide(current, None, None, Some(tmpdir.path()))
            .await;
        // Formatter ran → result is "hello", not from the (missing) file.
        assert_eq!(
            new_state.results.get("greet"),
            Some(&json!({"text": "hello"}))
        );
        // No file-source cache_hit was emitted.
        let events = match &decision {
            AutomationDecision::StepExecutedInline { emitted_events, .. } => emitted_events,
            other => panic!("expected StepExecutedInline, got {other:?}"),
        };
        let any_file_hit = events.iter().any(|(t, p)| {
            t == "subrun_step_cache_hit" && p.get("source").and_then(|v| v.as_str()) == Some("file")
        });
        assert!(!any_file_hit, "cache miss must not emit file cache_hit");
    }

    /// When the user did not opt in (`cache_enabled = false`), the prior run
    /// is ignored even if its hash would match.
    #[tokio::test]
    async fn cache_disabled_skips_lookup() {
        let wf = formatter_automation("greet", "hi");
        let mut prior = fresh_state(wf.clone(), false, None);
        prior.run_id = "prior-run".into();
        prior
            .results
            .insert("greet".into(), json!({"text": "STALE"}));
        let prior_hash = compute_step_hash(&StepHashInputs {
            step_config: &wf.tasks[0],
            render_context: &prior.render_context,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        prior.step_hashes.insert("greet".into(), prior_hash);

        // cache_enabled = false even though retry_from points at prior.
        let current = fresh_state(wf, false, Some("prior-run"));
        let decider = AutomationDecider::new(None);
        let (new_state, _) = decider.decide(current, None, Some(&prior), None).await;

        // Step ran fresh (the formatter rendered "hi"), not "STALE".
        assert_eq!(new_state.results.get("greet"), Some(&json!({"text": "hi"})));
    }

    /// When the step config changes, the hash differs and the cache must miss.
    #[tokio::test]
    async fn config_change_misses_cache() {
        let prior_wf = formatter_automation("greet", "hello");
        let mut prior = fresh_state(prior_wf.clone(), false, None);
        prior.run_id = "prior-run".into();
        let prior_hash = compute_step_hash(&StepHashInputs {
            step_config: &prior_wf.tasks[0],
            render_context: &prior.render_context,
            variables: None,
            loop_idx: None,
            sub_workflow_yaml_hash: None,
        })
        .unwrap();
        prior
            .results
            .insert("greet".into(), json!({"text": "hello"}));
        prior.step_hashes.insert("greet".into(), prior_hash);

        // Same step name, different template — must re-execute.
        let new_wf = formatter_automation("greet", "HOLA");
        let current = fresh_state(new_wf, true, Some("prior-run"));
        let decider = AutomationDecider::new(None);
        let (new_state, decision) = decider.decide(current, None, Some(&prior), None).await;

        // Ran fresh: render produced "HOLA".
        assert_eq!(
            new_state.results.get("greet"),
            Some(&json!({"text": "HOLA"}))
        );
        // No cache_hit event was emitted.
        if let AutomationDecision::StepExecutedInline { emitted_events, .. } = &decision {
            assert!(
                !emitted_events
                    .iter()
                    .any(|(t, _)| t == "subrun_step_cache_hit"),
                "must not emit cache_hit on config mismatch",
            );
        }
    }

    /// When the prior run has no hash for a step (e.g. previous run failed
    /// before the step succeeded), we re-execute. Crucially, even if a stale
    /// `results` entry survived from a prior failure, missing-hash means
    /// missing-cache and the result is regenerated cleanly.
    #[tokio::test]
    async fn missing_prior_hash_misses_cache() {
        let wf = formatter_automation("greet", "hi");
        let mut prior = fresh_state(wf.clone(), false, None);
        prior.run_id = "prior-run".into();
        // Prior has a result but no hash — simulating a failed step.
        prior
            .results
            .insert("greet".into(), json!({"text": "FAILURE_OUTPUT"}));

        let current = fresh_state(wf, true, Some("prior-run"));
        let decider = AutomationDecider::new(None);
        let (new_state, _) = decider.decide(current, None, Some(&prior), None).await;

        assert_eq!(new_state.results.get("greet"), Some(&json!({"text": "hi"})));
    }
}
