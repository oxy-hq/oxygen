//! Automation step orchestrator that drives the step DAG via coordinator
//! suspend/resume.
//!
//! The orchestrator runs as a long-lived task. For each workflow step it
//! either executes inline (formatter, conditional) or suspends to delegate
//! execution to the coordinator which dispatches it to a worker.
//!
//! **[`AutomationStepOrchestrator`] itself has no production caller** — the
//! stateless [`crate::step_decider`] replaced it, and only this module's tests
//! still construct it. The rest of the module is not dead: `StepKind`,
//! [`build_minijinja_context`] and [`to_column_oriented`] are the decider's
//! building blocks. Treat the actor as retained-but-dormant, and see
//! [`AutomationStepOrchestrator::with_airway_admission_resolver`] for what
//! that means for anyone reviving it.

use std::collections::HashMap;
use std::sync::Arc;

use crate::airway_admission::AirwayAdmissionResolver;
use crate::config::{AutomationConfig, TaskType};
use crate::resolve::build_subrun_steps;
use crate::step_decider::build_agent_extra;
use agentic_core::delegation::{
    DelegationItem, DelegationTarget, FanoutFailurePolicy, ResolvedAdmission, SuspendReason,
    TaskOutcome, TaskSpec,
};
use agentic_core::evaluator::ConsistencyEvaluator;
use agentic_core::human_input::SuspendedRunData;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Classifies a task type into an execution strategy.
enum StepKind {
    /// Execute directly in the orchestrator (no I/O, no coordinator round-trip).
    Inline,
    /// Delegate to coordinator as a single `AutomationStep` child task.
    Delegated,
    /// Delegate to coordinator as a `TaskSpec::Agent`.
    Agent {
        agent_ref: String,
        prompt: String,
        consistency_run: usize,
        consistency_prompt: Option<String>,
        /// Pre-built `extra` envelope (`output_mode` for analytics
        /// SQL-gen). Flows through to `TaskSpec::Agent.extra` opaque
        /// to this layer — the executor reads the field per agent path.
        extra: Option<Value>,
    },
    /// Delegate to coordinator as a `TaskSpec::Automation` (sub-workflow).
    SubAutomation {
        src: String,
        variables: Option<Value>,
    },
    /// Delegate to coordinator as a `TaskSpec::Airway`.
    ///
    /// Not `Delegated`: that path wraps the step as an opaque
    /// `AutomationStep` handled by `step_executor`, which only sees a
    /// `WorkspaceContext` (no `DatabaseConnection`) and returns
    /// `Result<Value, String>` rather than the streaming handle an airway
    /// run needs. Emitting `TaskSpec::Airway` reuses the working path.
    Airway {
        pipeline_ref: String,
        resources: Vec<String>,
    },
    /// Fan-out loop iterations via `ParallelDelegation`.
    Loop {
        values: Value,
        tasks: Value,
        concurrency: usize,
    },
}

/// Drives a workflow's steps using the coordinator's suspend/resume mechanism.
///
/// For each I/O step, the orchestrator suspends with a `Delegation` or
/// `ParallelDelegation`, which the coordinator dispatches to a worker.
/// The worker executes the step and returns the result. The orchestrator
/// then merges the result into the render context and moves to the next step.
pub struct AutomationStepOrchestrator {
    workflow: AutomationConfig,
    /// Serialized render context — accumulated step outputs as JSON.
    render_context: Value,
    /// Automation-level context (workspace path, global settings, etc.).
    workflow_context: Value,
    /// Step name → serialized OutputContainer result.
    results: HashMap<String, Value>,
    /// Current step index (for crash recovery).
    current_step: usize,
    /// Trace ID for event correlation.
    trace_id: String,
    /// Optional LLM-based consistency evaluator for pairwise answer comparison.
    evaluator: Option<Arc<dyn ConsistencyEvaluator>>,
    /// Host port that resolves `airway_source_config` for an airway step.
    /// `None` keeps airway's `permissive` / `production` defaults — see
    /// [`crate::AirwayAdmissionResolver`].
    airway_admission: Option<Arc<dyn AirwayAdmissionResolver>>,
}

impl AutomationStepOrchestrator {
    pub fn new(
        workflow: AutomationConfig,
        workflow_context: Value,
        variables: Option<Value>,
        trace_id: String,
        evaluator: Option<Arc<dyn ConsistencyEvaluator>>,
    ) -> Self {
        // Seed render context with workflow-level variables if provided.
        let render_context = variables.unwrap_or(json!({}));
        Self {
            workflow,
            render_context,
            workflow_context,
            results: HashMap::new(),
            current_step: 0,
            trace_id,
            evaluator,
            airway_admission: None,
        }
    }

    /// Inject the admission resolver used by the [`StepKind::Airway`] arm.
    /// Mirrors [`crate::AutomationDecider::with_airway_admission_resolver`].
    ///
    /// **Nothing in production calls this, because nothing in production
    /// constructs an `AutomationStepOrchestrator` at all.** This type is the
    /// long-lived actor that [`crate::step_decider`] replaced; the only
    /// remaining constructor calls are in this module's own tests, and the
    /// queue-driven path injects its resolver on `AutomationDecider` instead
    /// (`agentic_pipeline::executor::automation::execute_automation_decision`).
    /// The struct is kept, not deleted, because the rest of the module is very
    /// much alive — `build_minijinja_context`, `to_column_oriented` and the
    /// `StepKind` classification here are what the decider is built out of —
    /// and unpicking the actor from them is a refactor, not a review fix.
    ///
    /// So this exists for **parity, on purpose**: the orchestrator's airway arm
    /// would otherwise queue under a silently-defaulted `permissive`, which is
    /// exactly the bug the port was added to close. Leaving the seam here means
    /// a future caller that revives the actor has something to wire rather than
    /// a hole to discover. If you are that caller, wire it — the resolver is
    /// `agentic_pipeline::airway_config::PipelineAirwayAdmissionResolver`, and
    /// an orchestrator constructed without it ignores the operator's
    /// `airway_source_config` policy silently.
    pub fn with_airway_admission_resolver(
        mut self,
        resolver: Arc<dyn AirwayAdmissionResolver>,
    ) -> Self {
        self.airway_admission = Some(resolver);
        self
    }

    /// Resolve the admission for `pipeline_ref` at dispatch time; the default
    /// (both fields `None`) when no resolver is injected.
    async fn resolve_airway_admission(
        &self,
        pipeline_ref: &str,
    ) -> Result<ResolvedAdmission, String> {
        match &self.airway_admission {
            Some(resolver) => resolver.resolve_for_pipeline(pipeline_ref).await,
            None => Ok(ResolvedAdmission::default()),
        }
    }

    /// Run the orchestrator loop.
    ///
    /// Iterates over workflow steps, emitting events and outcomes on the
    /// provided channels. Suspends for delegated steps and waits for the
    /// coordinator to resume with the child's answer.
    pub async fn run(
        &mut self,
        event_tx: mpsc::Sender<(String, Value)>,
        outcome_tx: mpsc::Sender<TaskOutcome>,
        mut answer_rx: mpsc::Receiver<String>,
    ) -> Result<(), String> {
        let subrun_name = self.workflow.name.clone();

        // Emit SubrunStarted with the full nested step DAG. The FE uses
        // `inner_tasks` recursively to render loop iterations and
        // sub-workflow expansions with per-task-type renderers.
        let steps = build_subrun_steps(&self.workflow.tasks);
        self.emit_event(
            &event_tx,
            "subrun_started",
            json!({
                "subrun_name": &subrun_name,
                "steps": steps,
            }),
        )
        .await;

        // Process each step.
        while self.current_step < self.workflow.tasks.len() {
            let task = self.workflow.tasks[self.current_step].clone();
            let step_name = task.name.clone();
            let kind = self.classify_step(&task.task_type);

            self.emit_event(
                &event_tx,
                "subrun_step_started",
                json!({ "step": &step_name }),
            )
            .await;

            let result = match kind {
                StepKind::Inline => {
                    // Execute inline (formatter, conditional).
                    self.execute_inline(&task.task_type)
                }

                StepKind::Delegated => {
                    // Suspend for AutomationStep delegation.
                    let step_config = serde_json::to_value(&task)
                        .map_err(|e| format!("failed to serialize step config: {e}"))?;

                    self.suspend_for_step(
                        &outcome_tx,
                        &mut answer_rx,
                        &step_name,
                        TaskSpec::AutomationStep {
                            step_config,
                            render_context: self.render_context.clone(),
                            workflow_context: self.workflow_context.clone(),
                        },
                    )
                    .await
                }

                StepKind::Airway {
                    pipeline_ref,
                    resources,
                } => {
                    // Straight to the existing airway task spec, so the
                    // coordinator routes it to `execute_airway` and it
                    // inherits secret resolution, the Airhouse credential
                    // provider and run-scoped state. Backfill bounds stay
                    // `None` — windowed backfills are driven by the backfill
                    // path, not by an automation step.
                    //
                    // `contract_policy`/`environment` come from the injected
                    // [`AirwayAdmissionResolver`] port, resolved here — where
                    // the spec is built, before the coordinator writes the
                    // child's queue row — so this path is admitted under the
                    // same `airway_source_config` policy `start_airway_run`
                    // applies to schedule- and HTTP-triggered runs.
                    let admission = match self.resolve_airway_admission(&pipeline_ref).await {
                        Ok(admission) => admission,
                        Err(e) => {
                            // Fail rather than queue under a
                            // silently-defaulted `permissive`.
                            return Err(format!("airway admission for step {step_name}: {e}"));
                        }
                    };
                    self.suspend_for_step(
                        &outcome_tx,
                        &mut answer_rx,
                        &step_name,
                        TaskSpec::Airway {
                            pipeline_ref,
                            variables: None,
                            resources,
                            backfill_from: None,
                            backfill_to: None,
                            contract_policy: admission.contract_policy,
                            environment: admission.environment,
                        },
                    )
                    .await
                }

                StepKind::Agent {
                    agent_ref,
                    prompt,
                    consistency_run,
                    consistency_prompt,
                    extra,
                } => {
                    // Render the prompt against the parent context here.
                    // The downstream agent pipeline does NOT re-render
                    // templates, so a passthrough sends raw `{{ ... }}`
                    // straight to the LLM, which then complains the
                    // data wasn't included.
                    let prompt =
                        match crate::render::render_jinja_string(&prompt, &self.render_context) {
                            Ok(rendered) => rendered,
                            Err(err) => {
                                return Err(format!("agent {step_name} prompt render: {err}"));
                            }
                        };
                    if consistency_run > 1 {
                        self.suspend_for_consistency_agents(
                            &outcome_tx,
                            &mut answer_rx,
                            &step_name,
                            &agent_ref,
                            &prompt,
                            consistency_run,
                            consistency_prompt.as_deref(),
                        )
                        .await
                    } else {
                        self.suspend_for_step(
                            &outcome_tx,
                            &mut answer_rx,
                            &step_name,
                            TaskSpec::Agent {
                                agent_id: agent_ref,
                                question: prompt,
                                extra,
                            },
                        )
                        .await
                    }
                }

                StepKind::SubAutomation { src, variables } => {
                    // Render the override map against the parent context so a
                    // passthrough like `variables: { month: "{{ month }}" }`
                    // resolves here instead of reaching the child verbatim.
                    match crate::variables::render_override_variables(
                        variables.as_ref(),
                        &self.render_context,
                    ) {
                        Ok(variables) => {
                            self.suspend_for_step(
                                &outcome_tx,
                                &mut answer_rx,
                                &step_name,
                                TaskSpec::Automation {
                                    workflow_ref: src,
                                    variables,
                                    // Child sub-workflows always run fresh —
                                    // cache linkage at child-run granularity
                                    // is a v2 feature.
                                    retry_from_run_id: None,
                                    cache_enabled: false,
                                    body: None,
                                    initial_render_context: None,
                                },
                            )
                            .await
                        }
                        Err(err) => Err(format!("sub-workflow {step_name}: {err}")),
                    }
                }

                StepKind::Loop {
                    values,
                    tasks,
                    concurrency,
                } => {
                    self.suspend_for_loop(
                        &outcome_tx,
                        &mut answer_rx,
                        &step_name,
                        values,
                        tasks,
                        concurrency,
                    )
                    .await
                }
            };

            match result {
                Ok(output) => {
                    // Merge result into context incrementally — only insert the
                    // new key rather than rebuilding from all results, avoiding
                    // O(S²) cloning as accumulated step outputs grow.
                    let context_value = to_column_oriented(&output);
                    self.results.insert(step_name.clone(), output);
                    if let Some(obj) = self.render_context.as_object_mut() {
                        obj.insert(step_name.clone(), context_value);
                    } else {
                        let mut map = serde_json::Map::new();
                        map.insert(step_name.clone(), context_value);
                        self.render_context = Value::Object(map);
                    }

                    self.emit_event(
                        &event_tx,
                        "subrun_step_completed",
                        json!({ "step": &step_name, "success": true }),
                    )
                    .await;
                }
                Err(e) => {
                    self.emit_event(
                        &event_tx,
                        "subrun_step_completed",
                        json!({ "step": &step_name, "success": false, "error": &e }),
                    )
                    .await;

                    self.emit_event(
                        &event_tx,
                        "subrun_completed",
                        json!({
                            "subrun_name": &subrun_name,
                            "success": false,
                            "error": &e,
                        }),
                    )
                    .await;

                    return Err(e);
                }
            }

            self.current_step += 1;
        }

        // All steps done.
        self.emit_event(
            &event_tx,
            "subrun_completed",
            json!({
                "subrun_name": &subrun_name,
                "success": true,
            }),
        )
        .await;

        // Emit final Done outcome with aggregated results as a JSON array.
        // The analytics Interpreting stage's `parse_delegation_answer` expects
        // `[{columns: [...], rows: [...]}, ...]` — one entry per step.
        let final_output: Vec<Value> = self
            .workflow
            .tasks
            .iter()
            .filter_map(|t| self.results.get(&t.name))
            .cloned()
            .collect();
        let _ = outcome_tx
            .send(TaskOutcome::Done {
                answer: serde_json::to_string(&final_output).unwrap_or_else(|_| "[]".to_string()),
                metadata: None,
            })
            .await;

        Ok(())
    }

    // ── Step classification ─────────────────────────────────────────────

    fn classify_step(&self, task_type: &TaskType) -> StepKind {
        match task_type {
            TaskType::Formatter(_) | TaskType::Conditional(_) => StepKind::Inline,

            TaskType::Agent(agent_task) => {
                let output_mode = agent_task.output.as_ref().map(|o| match o.mode {
                    crate::config::AgentOutputMode::Answer => "answer",
                    crate::config::AgentOutputMode::Sql => "sql",
                });
                StepKind::Agent {
                    agent_ref: agent_task.agent_ref.clone(),
                    prompt: agent_task.prompt.clone(),
                    consistency_run: agent_task.consistency_run,
                    consistency_prompt: agent_task
                        .consistency_prompt
                        .clone()
                        .or_else(|| self.workflow.consistency_prompt.clone()),
                    extra: build_agent_extra(output_mode),
                }
            }

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

            // All I/O task types: delegate to coordinator.
            TaskType::ExecuteSql(_)
            | TaskType::SemanticQuery(_)
            | TaskType::OmniQuery(_)
            | TaskType::LookerQuery(_)
            | TaskType::HttpRequest(_)
            | TaskType::Unknown => StepKind::Delegated,
        }
    }

    // ── Inline execution ────────────────────────────────────────────────

    fn execute_inline(&self, task_type: &TaskType) -> Result<Value, String> {
        match task_type {
            TaskType::Formatter(fmt) => self.execute_formatter(&fmt.template),
            TaskType::Conditional(cond) => self.execute_conditional(cond),
            _ => Err("not an inline step".to_string()),
        }
    }

    /// Render a Jinja2 template with the accumulated render context.
    fn execute_formatter(&self, template: &str) -> Result<Value, String> {
        let env = crate::render::automation_env();

        let tmpl = env
            .template_from_str(template)
            .map_err(|e| format!("template parse error: {e}"))?;

        // Build context with ColumnTable wrappers for table step results.
        let ctx = build_minijinja_context(&self.render_context);
        let rendered = tmpl.render(&ctx).map_err(|e| {
            let available_keys: Vec<String> = self
                .render_context
                .as_object()
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            format!(
                "template render error: {e}\n\
                 Template (first 200 chars): {}\n\
                 Available context keys: {:?}",
                &template[..template.len().min(200)],
                available_keys,
            )
        })?;

        Ok(json!({ "text": rendered }))
    }

    /// Evaluate conditional branches and return the first matching branch's
    /// placeholder result, or the else branch.
    fn execute_conditional(
        &self,
        cond: &crate::config::ConditionalConfig,
    ) -> Result<Value, String> {
        let env = crate::render::automation_env();
        let ctx = build_minijinja_context(&self.render_context);

        for branch in &cond.conditions {
            if crate::render::condition_holds(&env, &branch.condition, &ctx)? {
                // Return branch task names as the result — the actual
                // execution of branch tasks would require delegation.
                let task_names: Vec<String> = branch.tasks.iter().map(|t| t.name.clone()).collect();
                return Ok(json!({
                    "branch": "matched",
                    "condition": &branch.condition,
                    "tasks": task_names,
                }));
            }
        }

        // No condition matched — use else branch if present.
        if let Some(else_tasks) = &cond.else_tasks {
            let task_names: Vec<String> = else_tasks.iter().map(|t| t.name.clone()).collect();
            Ok(json!({
                "branch": "else",
                "tasks": task_names,
            }))
        } else {
            Ok(json!({ "branch": "none_matched" }))
        }
    }

    // ── Delegation helpers ──────────────────────────────────────────────

    /// Suspend for a single child task and wait for the answer.
    async fn suspend_for_step(
        &self,
        outcome_tx: &mpsc::Sender<TaskOutcome>,
        answer_rx: &mut mpsc::Receiver<String>,
        step_name: &str,
        spec: TaskSpec,
    ) -> Result<Value, String> {
        let suspend_data = self.build_suspend_data(step_name);

        // Determine delegation target from spec.
        let (target, request, context) = match &spec {
            TaskSpec::Agent {
                agent_id,
                question,
                extra,
            } => (
                DelegationTarget::Agent {
                    agent_id: agent_id.clone(),
                },
                question.clone(),
                extra
                    .as_ref()
                    .map(|v| json!({ "extra": v }))
                    .unwrap_or_else(|| json!({})),
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
                variables.clone().unwrap_or(json!({})),
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
                json!({
                    "step_config": step_config,
                    "render_context": render_context,
                    "workflow_context": workflow_context,
                }),
            ),
            // `DelegationTarget` has no `Airway` variant, so tunnel through an
            // `Automation` target with a sentinel ref (mirrors
            // `AutomationStep`); the resolver rebuilds the spec from
            // `airway_spec`. Kept in sync with `spec_to_delegation_parts` on
            // the stateless-decider path.
            TaskSpec::Airway { .. } => (
                DelegationTarget::Automation {
                    workflow_ref: "__airway__".to_string(),
                },
                step_name.to_string(),
                json!({
                    "airway_spec": serde_json::to_value(&spec).unwrap_or(Value::Null),
                }),
            ),
            _ => {
                return Err(format!("unexpected spec type for step {step_name}"));
            }
        };

        outcome_tx
            .send(TaskOutcome::Suspended {
                reason: SuspendReason::Delegation {
                    target,
                    request,
                    context,
                    policy: None,
                },
                resume_data: suspend_data,
                trace_id: self.trace_id.clone(),
            })
            .await
            .map_err(|_| "outcome channel closed".to_string())?;

        // Wait for coordinator to resume with child's answer.
        let answer = answer_rx
            .recv()
            .await
            .ok_or_else(|| "answer channel closed".to_string())?;

        // Parse the answer as JSON (OutputContainer or plain text).
        serde_json::from_str::<Value>(&answer).or_else(|_| Ok(json!({ "text": answer })))
    }

    /// Suspend for N parallel agent tasks (consistency run).
    async fn suspend_for_consistency_agents(
        &self,
        outcome_tx: &mpsc::Sender<TaskOutcome>,
        answer_rx: &mut mpsc::Receiver<String>,
        step_name: &str,
        agent_ref: &str,
        prompt: &str,
        n: usize,
        consistency_prompt: Option<&str>,
    ) -> Result<Value, String> {
        let targets: Vec<DelegationItem> = (0..n)
            .map(|_| DelegationItem {
                target: DelegationTarget::Agent {
                    agent_id: agent_ref.to_string(),
                },
                request: prompt.to_string(),
                context: json!({}),
            })
            .collect();

        let suspend_data = self.build_suspend_data(step_name);

        outcome_tx
            .send(TaskOutcome::Suspended {
                reason: SuspendReason::ParallelDelegation {
                    targets,
                    failure_policy: FanoutFailurePolicy::BestEffort,
                },
                resume_data: suspend_data,
                trace_id: self.trace_id.clone(),
            })
            .await
            .map_err(|_| "outcome channel closed".to_string())?;

        let answer = answer_rx
            .recv()
            .await
            .ok_or_else(|| "answer channel closed".to_string())?;

        // Parse the aggregated results from the coordinator.
        // ParallelDelegation with BestEffort returns a JSON object keyed by child_id:
        // { "child_id_1": { "status": "done", "answer": "..." }, ... }
        let aggregated: Value = serde_json::from_str(&answer).unwrap_or(json!({ "text": answer }));

        // Extract individual answers.
        let mut answers: Vec<String> = Vec::new();
        if let Some(obj) = aggregated.as_object() {
            for (_child_id, result) in obj {
                if result.get("status").and_then(|s| s.as_str()) == Some("done")
                    && let Some(a) = result.get("answer").and_then(|a| a.as_str())
                {
                    answers.push(a.to_string());
                }
            }
        }

        if answers.is_empty() {
            // All children failed or no parseable results — return the raw answer.
            return Ok(json!({
                "value": json!({ "text": answer }),
                "score": 0.0,
                "consistency_run": n,
            }));
        }

        // Pick the best answer using the consistency evaluator if available,
        // otherwise fall back to majority-vote by exact string equality.
        let (best_answer, score) = if let Some(evaluator) = &self.evaluator {
            match evaluator
                .evaluate(prompt, &answers, consistency_prompt)
                .await
            {
                Ok(result) => {
                    let selected = answers
                        .get(result.selected_index)
                        .cloned()
                        .unwrap_or_else(|| answers[0].clone());
                    tracing::info!(
                        selected_index = result.selected_index,
                        score = result.score,
                        reasoning = %result.reasoning,
                        "consistency evaluator picked answer"
                    );
                    (selected, result.score)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "consistency evaluator failed, falling back to majority-vote");
                    majority_vote(&answers)
                }
            }
        } else {
            majority_vote(&answers)
        };

        // Parse the winning answer as JSON if possible.
        let value: Value =
            serde_json::from_str(&best_answer).unwrap_or_else(|_| json!({ "text": best_answer }));

        Ok(json!({
            "value": value,
            "score": score,
            "consistency_run": n,
        }))
    }

    /// Suspend for loop iterations via ParallelDelegation or sequential Delegation.
    async fn suspend_for_loop(
        &self,
        outcome_tx: &mpsc::Sender<TaskOutcome>,
        answer_rx: &mut mpsc::Receiver<String>,
        step_name: &str,
        values: Value,
        tasks: Value,
        concurrency: usize,
    ) -> Result<Value, String> {
        let items = values
            .as_array()
            .ok_or_else(|| format!("loop {step_name}: values must be an array"))?;

        if items.is_empty() {
            return Ok(json!([]));
        }

        // Build a AutomationStep for each loop iteration.
        // Snapshot the render context once so all iterations share the same
        // base — each iteration only differs by its loop variable injection.
        let base_context = self.render_context.clone();
        let targets: Vec<DelegationItem> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let mut iter_context = base_context.clone();
                if let Some(obj) = iter_context.as_object_mut() {
                    obj.insert(step_name.to_string(), json!({ "value": item, "index": i }));
                    // Also expose `value` / `index` at top level so bare
                    // `{{ value }}` / `{{ index }}` resolve in the loop
                    // body without forcing the qualified form. Nested
                    // loops: innermost wins.
                    obj.insert("value".to_string(), item.clone());
                    obj.insert("index".to_string(), json!(i));
                }

                DelegationItem {
                    target: DelegationTarget::Automation {
                        workflow_ref: "__workflow_step__".to_string(),
                    },
                    request: format!("{step_name}[{i}]"),
                    context: json!({
                        "step_config": {
                            "name": format!("{step_name}_{i}"),
                            "tasks": &tasks,
                        },
                        "render_context": iter_context,
                        "workflow_context": &self.workflow_context,
                        "loop_item": item,
                        "loop_index": i,
                    }),
                }
            })
            .collect();

        let suspend_data = self.build_suspend_data(step_name);

        if concurrency > 1 && items.len() > 1 {
            // Parallel fan-out.
            outcome_tx
                .send(TaskOutcome::Suspended {
                    reason: SuspendReason::ParallelDelegation {
                        targets,
                        failure_policy: FanoutFailurePolicy::FailFast,
                    },
                    resume_data: suspend_data,
                    trace_id: self.trace_id.clone(),
                })
                .await
                .map_err(|_| "outcome channel closed".to_string())?;

            let answer = answer_rx
                .recv()
                .await
                .ok_or_else(|| "answer channel closed".to_string())?;

            serde_json::from_str::<Value>(&answer).or_else(|_| Ok(json!({ "text": answer })))
        } else {
            // Sequential: delegate one at a time.
            let mut loop_results = Vec::new();
            for target in targets {
                let iter_spec = TaskSpec::AutomationStep {
                    step_config: target.context["step_config"].clone(),
                    render_context: target.context["render_context"].clone(),
                    workflow_context: target.context["workflow_context"].clone(),
                };

                let result = self
                    .suspend_for_step(outcome_tx, answer_rx, &target.request, iter_spec)
                    .await?;
                loop_results.push(result);
            }
            Ok(json!(loop_results))
        }
    }

    // ── State management ────────────────────────────────────────────────

    fn build_suspend_data(&self, step_name: &str) -> SuspendedRunData {
        SuspendedRunData {
            from_state: "workflow".to_string(),
            original_input: self.workflow.name.clone(),
            trace_id: self.trace_id.clone(),
            // Use full to_state() so from_state() can reconstruct the
            // orchestrator after a crash. Includes workflow config,
            // render_context, results, current_step, etc.
            stage_data: self.to_state(),
            question: format!("Executing step: {step_name}"),
            suggestions: vec![],
        }
    }

    /// Serialize orchestrator state for crash recovery.
    pub fn to_state(&self) -> Value {
        json!({
            "current_step": self.current_step,
            "results": self.results,
            "render_context": self.render_context,
            "workflow": serde_json::to_value(&self.workflow).unwrap_or_default(),
            "workflow_context": self.workflow_context,
            "trace_id": self.trace_id,
        })
    }

    /// Restore orchestrator from serialized state.
    pub fn from_state(state: Value) -> Result<Self, String> {
        let workflow: AutomationConfig = serde_json::from_value(state["workflow"].clone())
            .map_err(|e| format!("failed to deserialize workflow: {e}"))?;
        let results: HashMap<String, Value> =
            serde_json::from_value(state["results"].clone()).unwrap_or_default();
        let current_step = state["current_step"].as_u64().unwrap_or(0) as usize;
        let render_context = state["render_context"].clone();
        let workflow_context = state["workflow_context"].clone();
        let trace_id = state["trace_id"].as_str().unwrap_or("unknown").to_string();

        Ok(Self {
            workflow,
            render_context,
            workflow_context,
            results,
            current_step,
            trace_id,
            evaluator: None, // Evaluator is set via set_evaluator() after recovery.
            // Same as `evaluator`: a host port, not serializable state.
            // Re-inject via `with_airway_admission_resolver` after recovery.
            airway_admission: None,
        })
    }

    /// Set the consistency evaluator (used after crash recovery via `from_state`).
    pub fn set_evaluator(&mut self, evaluator: Option<Arc<dyn ConsistencyEvaluator>>) {
        self.evaluator = evaluator;
    }

    /// Access the automation configuration (used by pipeline to build evaluator on resume).
    pub fn automation_config(&self) -> &AutomationConfig {
        &self.workflow
    }

    // ── Event emission ──────────────────────────────────────────────────

    async fn emit_event(
        &self,
        event_tx: &mpsc::Sender<(String, Value)>,
        event_type: &str,
        payload: Value,
    ) {
        let _ = event_tx.send((event_type.to_string(), payload)).await;
    }
}

pub mod minijinja_helpers;

#[cfg(test)]
mod tests;

use minijinja_helpers::majority_vote;
pub(crate) use minijinja_helpers::{build_minijinja_context, to_column_oriented};
