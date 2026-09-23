//! [`TaskExecutor`] implementation for the agentic pipeline layer.
//!
//! This is the composition point where domain knowledge (analytics, builder,
//! automation) meets the generic coordinator-worker infrastructure. The runtime
//! only sees [`TaskExecutor`]; this crate knows how to start the right pipeline
//! for each [`TaskSpec`] variant.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_analytics::SchemaCatalog;
use agentic_builder::{BuilderAppRunner, BuilderTestRunner};
use agentic_core::delegation::{TaskAssignment, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;

use crate::platform::{BuilderBridges, PlatformContext};
use crate::{PipelineBuilder, ThinkingMode};

// ── PipelineTaskExecutor ─────────────────────────────────────────────────────

/// Knows how to start analytics/builder pipelines and automation executions.
///
/// Injected into the [`Worker`](agentic_runtime::worker::Worker) by the
/// HTTP/CLI layer.
pub struct PipelineTaskExecutor {
    pub platform: Arc<dyn PlatformContext>,
    /// Required for builder delegation; `None` is fine for analytics-only runs.
    pub builder_bridges: Option<BuilderBridges>,
    pub schema_cache: Option<Arc<Mutex<HashMap<String, SchemaCatalog>>>>,
    pub builder_test_runner: Option<Arc<dyn BuilderTestRunner>>,
    pub builder_app_runner: Option<Arc<dyn BuilderAppRunner>>,
    pub db: DatabaseConnection,
    /// Runtime state for registering answer channels (needed by automation
    /// orchestrator tasks so the coordinator can resume them via answer channel
    /// instead of TaskSpec::Resume).
    pub state: Option<Arc<agentic_runtime::state::RuntimeState>>,
    /// Host-supplied handlers for `TaskSpec::Custom` kinds (e.g. workspace
    /// health eval). `None`/empty means Custom tasks are unhandled. Injected by
    /// the global-run driver so the pipeline crate need not import the host.
    pub custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
}

/// Error from [`PipelineTaskExecutor::reset_airway_schema`], split so the
/// transport maps a caller mistake to `400` and a server-side failure to `500`
/// (a failed destination drop / state delete is not a client error).
#[derive(Debug)]
pub enum ResetSchemaError {
    /// Bad/unknown `pipeline_ref`, an unparseable spec, or a non-airhouse
    /// destination — the caller's input. → `400`.
    BadRequest(String),
    /// State read/delete or the destination drop failed — server-side. → `500`.
    Internal(String),
    /// The pipeline's YAML could not be resolved **on this node** — a
    /// compile-boundary blip, or a revision still compiling. → `503`.
    ///
    /// Not `BadRequest`: the caller's `pipeline_ref` may be perfectly good, and
    /// telling them it is bad is both false and unactionable. Same reasoning as
    /// `AirwayRunError::Unavailable` on the start path — this variant exists so
    /// the two paths do not disagree about the same condition inside one file.
    Unavailable(String),
}

impl std::fmt::Display for ResetSchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) | Self::Internal(m) | Self::Unavailable(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ResetSchemaError {}

/// How long a contended airway task waits before trying for the lease again.
///
/// Short enough that a pipeline finishing early is picked up promptly, long
/// enough that a long load does not spin the queue. The wait is invisible —
/// the task is not claimed while deferred, so it costs no worker slot.
const AIRWAY_LEASE_RETRY_SECS: u64 = 30;

/// Total wall clock an airway task may spend waiting for the single-flight
/// lease before it is dead-lettered — twice the lease TTL.
///
/// The ceiling exists so a permanently blocked pipeline surfaces as a failure
/// rather than as a queue that grows while looking healthy. Twice the TTL
/// because a lease is at most one TTL old before liveness or expiry reclaims
/// it, so a task that has waited two full TTLs is not contending, it is stuck.
///
/// This is the DOMAIN's number, passed with the deferral: only airway knows how
/// long its work can legitimately be blocked. The queue enforces it and has no
/// opinion on the value.
const AIRWAY_LEASE_MAX_WAIT_SECS: u64 =
    2 * agentic_airway::extension::pipeline_lease::LEASE_TTL_SECS as u64;

/// How long a task whose pipeline YAML could not be resolved on this node
/// waits before being handed to a worker again.
///
/// Equal to [`AIRWAY_LEASE_RETRY_SECS`], which is the cadence that goes with
/// the ceiling below — the two deferral kinds share `first_deferred_at`, so
/// they share a budget, and a budget is a count of retries as much as a span of
/// time. This was `5`, chosen against a 300s ceiling where it meant ~60
/// claim→defer cycles. Against 43200s the same `5` means **~8,640** cycles,
/// each a queue claim, a `defer_task` UPDATE and a log line; N tasks stuck in
/// one compile window would churn at N/5 claims per second for its duration.
/// At 30s the count matches the lease deferral's ~1,440, which is the churn
/// this queue is already designed for.
///
/// The cost is recovery latency: a blip now clears in up to 30s rather than 5.
/// That is the right way round for a condition measured in promote times, and
/// nothing user-facing waits on it — the submit path is bounded far sooner by
/// `ADMISSION_MAX_ATTEMPTS`.
///
/// Escalating (short delays first, then long) would beat both, and needs a
/// defer count the executor cannot see: `TaskAssignment` carries none, so it
/// would mean a change in `agentic-runtime`.
pub const AIRWAY_UNAVAILABLE_RETRY_SECS: u64 = 30;

/// Dead-letter ceiling passed with an unavailable-deferral.
///
/// **Equal to [`AIRWAY_LEASE_MAX_WAIT_SECS`] on purpose, and it must not be
/// lowered.** `defer_task` measures the ceiling against `first_deferred_at`,
/// which is the first defer of the current streak *whatever its reason* —
/// `COALESCE(first_deferred_at, now())`, written once and deliberately not
/// cleared on claim-side paths (`runtime/orchestrator/crud/queue.rs`). Both
/// deferral kinds in `execute_airway` therefore share one clock, so the
/// smaller ceiling silently governs the larger.
///
/// A shorter value here reads as "give up on an unavailable node sooner" and
/// actually means: any task that has waited longer than that on the
/// single-flight lease is dead-lettered by its *first* unavailable defer —
/// minutes into a legitimate 12-hour wait, for the transient condition this
/// deferral exists to survive. It was 300s, which is exactly that bug.
///
/// The cost of matching is dead-letter latency for a *permanently* unresolvable
/// task: 12 hours rather than 5 minutes, and the dead-letter is the only
/// operator-visible signal it produces. A misconfigured `workspace_path` is one
/// such case; the commoner one is a ref that no longer exists in the newly
/// promoted revision, because on a stateless replica "gone from revision B" and
/// "revision B has not compiled yet" are the same `Ok(None)` and so the same
/// `Unavailable`. Neither can reach a caller that way — the submit path is
/// bounded much sooner by `ADMISSION_MAX_ATTEMPTS`.
///
/// Giving each reason its own clock is the real fix and belongs to
/// `agentic-runtime`, not here.
const AIRWAY_UNAVAILABLE_MAX_WAIT_SECS: u64 = AIRWAY_LEASE_MAX_WAIT_SECS;

/// Dead-letter ceiling for a ref the boundary **answered about** and does not
/// have (`PipelineRefError::NotInRevision`). Ten minutes.
///
/// Short on purpose, and the opposite call from the ceiling above, because the
/// two deferrals carry opposite information. `Unavailable` says *nobody here
/// knows*, so waiting is the whole strategy and cutting it short throws away a
/// run over a blip. `NotInRevision` says *the boundary knows, and the answer is
/// no* — the only thing that changes it is a compile promoting the ref, which
/// takes seconds on every workspace we serve. Twenty retries is generous for
/// that, and the alternative is what this was: a run deferring every 30s for
/// thirteen hours because its pipeline lived only in someone's working copy.
///
/// **The shared-streak caveat from `AIRWAY_UNAVAILABLE_MAX_WAIT_SECS` applies
/// here, and it cuts both ways.** `defer_task` measures every ceiling against
/// one `first_deferred_at`, written once per streak whatever the reason, so a
/// `NotInRevision` deferral inherits whatever streak the task already has.
/// There are two predecessors and they are not equally benign:
///
/// * **A lease streak is benign.** A task waiting hours on the single-flight
///   lease and then taking a `NotInRevision` deferral is dead-lettered on the
///   spot — and that is correct, not collateral. The ordering in
///   `execute_airway` is what makes it so: the YAML load runs *before* the
///   lease acquire, so a task cannot hold a lease streak without having
///   resolved at least once. Reaching this variant afterwards means the
///   pipeline left the promoted revision mid-wait — deleted or renamed — and
///   failing it is the answer.
///
/// * **An `Unavailable` streak is NOT, and this ceiling does not help there.**
///   `Unavailable` writes the same column and implies nothing ever resolved.
///   A task deferring against the 12h ceiling because nothing is promoted
///   yet, which then sees a revision promoted without its ref, takes its first
///   `NotInRevision` deferral with a streak already older than 600s — and is
///   dead-lettered with **zero** retries, in exactly the window this ceiling
///   exists to grant. The message is worded not to promise that retry (see
///   `pipeline_ref`), but the window is still lost.
///
/// Closing the second case needs what `#2906` already named as the real fix
/// and deliberately left to `agentic-runtime`: a streak per reason class, so
/// `defer_task` can reset the clock when the reason changes rather than
/// carrying one forward across two conditions that mean different things. It
/// is not reachable from this crate — `TaskAssignment` carries no defer
/// history — and it is a queue change, not an airway one. Until then this
/// ceiling is an improvement on the common path (a fresh streak, which is
/// what the incident had) and a no-op on the uncommon one, which is the right
/// way round but is not the same as being correct in both.
const AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS: u64 = 600;

/// What to do with a pipeline-YAML load failure at claim time.
///
/// Split from the call site so the choice can be asserted without a database, a
/// queue, or a workspace — the same reason `airway_config::classify_load_failure`
/// is a free function. What changed in this area was a *disposition*, and a
/// disposition nothing asserts is a disposition that quietly reverts.
#[derive(Debug, PartialEq)]
enum LoadFailureAction {
    /// Not this node's answer to give: hand the task back to the queue.
    Defer {
        delay_secs: u64,
        max_wait_secs: u64,
        reason: String,
    },
    /// The ref or its bytes are wrong; no node will do better.
    Fail(String),
}

fn action_for_load_failure(e: crate::pipeline_ref::PipelineRefError) -> LoadFailureAction {
    match e {
        crate::pipeline_ref::PipelineRefError::Unavailable(m) => LoadFailureAction::Defer {
            delay_secs: AIRWAY_UNAVAILABLE_RETRY_SECS,
            max_wait_secs: AIRWAY_UNAVAILABLE_MAX_WAIT_SECS,
            reason: m,
        },
        // The boundary answered and the ref is not in the promoted revision.
        // Still a deferral — a compile promoting it is the normal resolution
        // and lands in seconds — but on [`AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS`],
        // not the twelve hours that belong to an unknown answer.
        crate::pipeline_ref::PipelineRefError::NotInRevision(m) => LoadFailureAction::Defer {
            delay_secs: AIRWAY_UNAVAILABLE_RETRY_SECS,
            max_wait_secs: AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS,
            reason: m,
        },
        e => LoadFailureAction::Fail(format!("airway: {e}")),
    }
}

/// Build an [`ExecutingTask`] that immediately reports [`TaskOutcome::Deferred`].
///
/// The executor's contract is to return a HANDLE, not an outcome, so a
/// deferral is expressed by handing back a task whose only outcome is the
/// deferral. No trait change, and the worker's existing outcome loop does the
/// translation.
fn deferred_task(delay_secs: u64, max_wait_secs: u64, reason: String) -> ExecutingTask {
    let (events_tx, events) = tokio::sync::mpsc::channel(1);
    drop(events_tx);
    let (outcomes_tx, outcomes) = tokio::sync::mpsc::channel(1);
    // Capacity 1 and a single send, so this cannot block.
    let _ = outcomes_tx.try_send(agentic_core::delegation::TaskOutcome::Deferred {
        delay_secs,
        max_wait_secs,
        reason,
    });
    ExecutingTask {
        events,
        outcomes,
        cancel: tokio_util::sync::CancellationToken::new(),
        answers: None,
    }
}

#[async_trait]
impl TaskExecutor for PipelineTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        // When this task has a parent, it's a delegation child — the
        // coordinator already created the run row, so pass the run_id
        // through to skip the duplicate insert.
        let is_child = assignment.parent_task_id.is_some();
        match &assignment.spec {
            TaskSpec::Agent {
                agent_id,
                question,
                extra,
            } => {
                // Top-level scheduled agent runs pre-seed the run row in
                // `start_agent_run` (mirrors the automation / airway
                // pattern). Detect that case by checking the DB and pass
                // `existing_run_id` so the analytics builder doesn't try
                // to insert a duplicate row.
                let pre_seeded = !is_child
                    && agentic_runtime::crud::get_run(&self.db, &assignment.run_id)
                        .await
                        .map_err(|e| format!("failed to load run: {e}"))?
                        .is_some();
                let existing_run_id = if is_child || pre_seeded {
                    Some(assignment.run_id.clone())
                } else {
                    None
                };
                self.execute_agent(agent_id, question, existing_run_id, extra.as_ref())
                    .await
            }

            TaskSpec::Automation {
                workflow_ref,
                variables,
                retry_from_run_id,
                cache_enabled,
                body,
                initial_render_context,
            } => {
                self.execute_automation(
                    &assignment.run_id,
                    workflow_ref,
                    variables.clone(),
                    retry_from_run_id.clone(),
                    *cache_enabled,
                    body.clone(),
                    initial_render_context.clone(),
                )
                .await
            }

            TaskSpec::Resume {
                run_id,
                resume_data,
                answer,
            } => {
                self.execute_resume(run_id, resume_data.clone(), answer.clone())
                    .await
            }

            TaskSpec::AutomationStep {
                step_config,
                render_context,
                workflow_context,
            } => {
                self.execute_automation_step(
                    step_config.clone(),
                    render_context.clone(),
                    workflow_context.clone(),
                )
                .await
            }

            TaskSpec::AutomationDecision {
                run_id,
                pending_child_answer,
            } => {
                self.execute_automation_decision(run_id, pending_child_answer.clone())
                    .await
            }

            TaskSpec::Custom { kind, .. } => {
                if let Some(handler) = self.custom_executors.as_ref().and_then(|r| r.get(kind)) {
                    return handler.execute(assignment).await;
                }
                Err(format!(
                    "PipelineTaskExecutor: no registered executor for Custom kind {kind:?}"
                ))
            }

            TaskSpec::Airway {
                pipeline_ref,
                variables,
                resources,
                backfill_from,
                backfill_to,
                contract_policy,
                environment,
            } => {
                // Parsed here, at the decode site, so an unknown spelling
                // surfaces with this run's context rather than deep inside
                // the worker.
                //
                // The explicit `match` rather than `?` is load-bearing: this
                // runs BEFORE `execute_airway`, so an early return would skip
                // the release below and strand the single-flight lease for its
                // full TTL — the exact failure that release exists to prevent.
                let admission = match agentic_airway::AirwayAdmission::from_strings(
                    contract_policy.as_deref(),
                    environment.as_deref(),
                ) {
                    Ok(admission) => admission,
                    Err(e) => {
                        tracing::warn!(
                            run_id = %assignment.run_id, error = %e,
                            "airway admission decode failed; releasing any \
                             single-flight lease held by this run"
                        );
                        crate::airway_run::release_airway_lease(&self.db, &assignment.run_id).await;
                        return Err(e.to_string());
                    }
                };
                // The lease is taken inside `execute_airway`, at claim time,
                // and released by `agentic_airway`'s worker when execution
                // ends. Between those two points sit the failures this arm
                // covers — unresolvable secrets, an unresolvable destination —
                // where the lease is held but no worker exists yet to release
                // it. Without this the run goes terminal with its lease held
                // for the full TTL; observed in dev as a run that reached
                // `failed` 38ms after creation and blocked its pipeline for 27
                // minutes.
                //
                // A DEFERRED task returns `Ok`, not `Err`, and holds no lease —
                // so it correctly does not reach this release.
                //
                // Release is `run_id`-scoped and idempotent, so this is safe
                // even when a later path would have released too.
                let started = self
                    .execute_airway(
                        &assignment.run_id,
                        pipeline_ref,
                        variables.as_ref(),
                        resources,
                        backfill_from.as_deref(),
                        backfill_to.as_deref(),
                        admission,
                    )
                    .await;
                if let Err(e) = &started {
                    tracing::warn!(
                        run_id = %assignment.run_id, error = %e,
                        // "any lease HELD BY THIS RUN" — inline automation airway
                        // steps never acquire at submit, so for those the DELETE is
                        // a correct no-op and the old wording was a false statement
                        // in the log.
                        "airway dispatch failed; releasing any single-flight lease held by this run"
                    );
                    crate::airway_run::release_airway_lease(&self.db, &assignment.run_id).await;
                }
                started
            }

            TaskSpec::Compile {
                workspace_id,
                git_sha,
                branch,
                promote,
                kind,
                owner_user_id,
            } => {
                self.execute_compile(
                    *workspace_id,
                    git_sha.clone(),
                    branch.clone(),
                    *promote,
                    kind.as_deref(),
                    *owner_user_id,
                )
                .await
            }
        }
    }

    async fn resume_from_state(
        &self,
        run: &agentic_runtime::entity::run::Model,
        suspend_data: Option<agentic_core::human_input::SuspendedRunData>,
    ) -> Result<ExecutingTask, String> {
        let source_type = run.source_type.as_deref().unwrap_or("analytics");

        // Temporal-style automation runs: if `agentic_workflow_state` exists for
        // this run, resume by enqueuing an AutomationDecision (stateless path).
        if source_type == "workflow" {
            match agentic_automation::extension::load_automation_state(&self.db, &run.id).await {
                Ok(Some(_)) => {
                    return self.execute_automation_decision(&run.id, None).await;
                }
                Ok(None) => {
                    // No durable state (run started before the Temporal refactor).
                    // Fall through to legacy resume path below.
                }
                Err(e) => {
                    tracing::warn!(
                        target: "pipeline",
                        run_id = %run.id,
                        error = %e,
                        "failed to check automation state; falling back to legacy resume"
                    );
                }
            }
        }

        // Also check task_metadata for automation orchestrator state.
        if let Some(ref meta) = run.task_metadata
            && meta.get("original_spec").is_some()
            && let Some(spec) = meta.get("original_spec")
            && spec.get("type").and_then(|t| t.as_str()) == Some("workflow")
        {
            // This was an automation child — try to re-run the automation.
            if let Some(workflow_ref) = spec.get("workflow_ref").and_then(|v| v.as_str()) {
                return self
                    .execute_automation(&run.id, workflow_ref, None, None, false, None, None)
                    .await;
            }
        }

        match source_type {
            "workflow" | "workflow_step" => {
                // Automation tasks without orchestrator checkpoint.
                if let Some(data) = suspend_data {
                    self.execute_resume(&run.id, data, String::new()).await
                } else {
                    Err(format!(
                        "cannot resume automation run {}: {NO_SAVED_STATE}",
                        run.id
                    ))
                }
            }
            _ => {
                // Analytics/builder: resume from checkpoint if available.
                if let Some(data) = suspend_data {
                    self.execute_resume(&run.id, data, String::new()).await
                } else {
                    // No checkpoint — run hadn't reached a suspension point.
                    // Cannot resume; user needs to resubmit the question.
                    Err(format!(
                        "run {} (type={source_type}) {NO_CHECKPOINT}",
                        run.id
                    ))
                }
            }
        }
    }
}

/// Tail of the resume error for a run that never reached a checkpoint.
///
/// Recovery matches on it (`recovery::is_unresumable`): a run a deploy
/// interrupted before its first checkpoint cannot be resumed, every rolling
/// deploy produces some, and that is a different event from a recovery that
/// broke. One constant, so the producer and the matcher cannot drift apart.
pub(crate) const NO_CHECKPOINT: &str = "has no checkpoint — resubmit the question";

/// Tail of the resume error for an automation run with no saved state. Same
/// role as [`NO_CHECKPOINT`].
pub(crate) const NO_SAVED_STATE: &str = "no saved state";

/// The well-known agent ID that routes to the builder domain instead of
/// analytics.  Used by analytics → builder delegation.
pub const BUILDER_AGENT_ID: &str = "__builder__";

/// Returns `true` when `agent_id` should be routed to the builder domain
/// rather than the analytics domain.
fn is_builder_agent(agent_id: &str) -> bool {
    agent_id == BUILDER_AGENT_ID
}

impl PipelineTaskExecutor {
    /// Minimal executor for host operations that need only workspace/secret
    /// resolution + the db (no builder/automation knobs) — e.g. the
    /// reset-schema endpoint. Single-sources the field list so adding a field
    /// can't silently break a `None`-heavy struct literal in a transport handler.
    pub fn bare(platform: Arc<dyn PlatformContext>, db: DatabaseConnection) -> Self {
        Self {
            platform,
            builder_bridges: None,
            schema_cache: None,
            builder_test_runner: None,
            builder_app_runner: None,
            db,
            state: None,
            custom_executors: None,
        }
    }

    async fn execute_agent(
        &self,
        agent_id: &str,
        question: &str,
        existing_run_id: Option<String>,
        extra: Option<&serde_json::Value>,
    ) -> Result<ExecutingTask, String> {
        let mut pb =
            PipelineBuilder::new(self.platform.clone()).workspace_id(self.platform.workspace_id());
        if let Some(bridges) = self.builder_bridges.clone() {
            pb = pb.with_builder_bridges(bridges);
        }
        let mut builder = if is_builder_agent(agent_id) {
            pb.builder(None)
        } else {
            pb.analytics(agent_id)
        }
        .question(question)
        .thinking_mode(ThinkingMode::Auto);

        // `extra` is an envelope packed by `agentic-automation` carrying
        // domain-opaque per-agent knobs. Today it carries the
        // analytics SQL-gen mode flag (`output_mode == "sql"`); the
        // builder path ignores it.
        if !is_builder_agent(agent_id)
            && let Some(extra_value) = extra
            && let Some(mode) = extra_value.get("output_mode").and_then(|v| v.as_str())
            && mode == "sql"
        {
            builder = builder.analytics_sql_mode();
        }

        // For delegation children, use the coordinator-assigned run_id
        // and skip the duplicate DB insert.
        if let Some(run_id) = existing_run_id.clone() {
            builder = builder.existing_run(run_id);
        }

        // Gate HITL when an agent runs as a delegation child
        // (existing_run_id is set → the coordinator created this
        // task). The parent automation's SSE stream doesn't yet
        // surface child-run events, so a nested suspension leaves
        // the automation UI looking hung. The provider differs by
        // agent type because the expected answer shape differs:
        //
        //   - Builder: `Accept` clears file-change confirmations.
        //   - Analytics: a directive string ("proceed with best
        //     interpretation") is more useful than a literal
        //     `Accept` as the answer to an `ask_user` call.
        //
        // Lift this gate once the automation run page streams nested
        // analytics events (see the streaming-children audit).
        if existing_run_id.is_some() {
            let provider: agentic_core::human_input::HumanInputHandle =
                if is_builder_agent(agent_id) {
                    std::sync::Arc::new(agentic_core::human_input::AutoAcceptInputProvider)
                } else {
                    std::sync::Arc::new(agentic_core::human_input::NoClarificationProvider)
                };
            builder = builder.human_input(provider);
        }

        if let Some(cache) = &self.schema_cache {
            builder = builder.schema_cache(cache.clone());
        }
        if let Some(runner) = &self.builder_test_runner {
            builder = builder.test_runner(runner.clone());
        }
        if let Some(runner) = &self.builder_app_runner {
            builder = builder.app_runner(runner.clone());
        }

        let started = builder
            .start(&self.db)
            .await
            .map_err(|e| format!("failed to start agent pipeline: {e}"))?;

        let (task, _bridge) = started.into_executing_task();
        Ok(task)
    }

    async fn execute_resume(
        &self,
        run_id: &str,
        resume_data: agentic_core::human_input::SuspendedRunData,
        answer: String,
    ) -> Result<ExecutingTask, String> {
        // Load run from DB to get source_type, agent_id, model, thread_id.
        let run = agentic_runtime::crud::get_run(&self.db, run_id)
            .await
            .map_err(|e| format!("failed to load run: {e}"))?
            .ok_or_else(|| format!("run {run_id} not found"))?;

        let source_type = run.source_type.as_deref().unwrap_or("analytics");
        // Resolve agent_id with a fallback. Top-level runs land it on
        // `metadata.agent_id` (via `start_analytics`'s insert path).
        // Delegation children are inserted by `insert_child_run` with
        // `metadata = None`, but their `task_metadata.original_spec`
        // carries the full `TaskSpec::Agent` — including `agent_id`.
        // Without this fallback, resuming an automation → analytics
        // chain would feed `""` into `start_analytics`, which then
        // calls `base_dir.join("")` (returns the workspace root, a
        // directory) and fails with `IO error: Is a directory`.
        let agent_id = run
            .metadata
            .as_ref()
            .and_then(|m| m.get("agent_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                run.task_metadata
                    .as_ref()
                    .and_then(|m| m.get("original_spec"))
                    .and_then(|s| s.get("agent_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let model = run
            .metadata
            .as_ref()
            .and_then(|m| m.get("model"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Resume path: `existing_run_id` will be set below, so the
        // builder skips the DB insert — workspace_id is not consulted at
        // INSERT time. We still set it for trace coherence and so any
        // future cold-resume insert lands on the right row.
        let mut builder = PipelineBuilder::new(self.platform.clone())
            .workspace_id(self.platform.workspace_id())
            .question(&run.question);
        if let Some(bridges) = self.builder_bridges.clone() {
            builder = builder.with_builder_bridges(bridges);
        }

        if let Some(cache) = &self.schema_cache {
            builder = builder.schema_cache(cache.clone());
        }
        if let Some(runner) = &self.builder_test_runner {
            builder = builder.test_runner(runner.clone());
        }
        if let Some(runner) = &self.builder_app_runner {
            builder = builder.app_runner(runner.clone());
        }
        if let Some(tid) = run.thread_id {
            builder = builder.thread(tid);
        }

        let started = builder
            .resume(
                &self.db,
                run_id,
                source_type,
                &agent_id,
                model,
                resume_data,
                answer,
            )
            .await
            .map_err(|e| format!("failed to resume pipeline: {e}"))?;

        let (task, _bridge) = started.into_executing_task();
        Ok(task)
    }

    /// Dispatch a `TaskSpec::Airway`. Loads the `.airway.yml` body through
    /// [`crate::pipeline_ref::load_pipeline_yaml`] (compiled `airway_pipelines`
    /// row first, workspace filesystem only if the host declines), parses it
    /// into an [`AirwayPipelineSpec`], and hands off to `AirwayWorker` which
    /// spawns the engine run and returns the runtime-shape channel pair.
    ///
    /// `variables` are rendered here, at claim time — the queued spec carries
    /// them so this worker reproduces the document the submitter validated.
    async fn execute_airway(
        &self,
        run_id: &str,
        pipeline_ref: &str,
        variables: Option<&serde_json::Value>,
        resources: &[String],
        backfill_from: Option<&str>,
        backfill_to: Option<&str>,
        admission: agentic_airway::AirwayAdmission,
    ) -> Result<ExecutingTask, String> {
        // Compile boundary first: this runs on the durable worker fleet, which
        // is stateless and has NO working copy — an FS read here is the
        // instance-affinity failure ("workspace directory not found" on a
        // replica). `load_pipeline_yaml` serves the compiled `airway_pipelines`
        // row and only falls back to disk when the host declines.
        // Defence-in-depth: `start_airway_run` already contained the ref at
        // submit time, but the guard re-runs at queue-claim too (the queued
        // spec is caller-influenced). Both resolve through `PlatformContext`'s
        // `WorkspaceContext` supertrait.
        //
        // `Unavailable` DEFERS rather than fails. This is the read the compile
        // boundary exists for, and the one most exposed to a mid-deploy blip:
        // the pre-enqueue admission resolve has its own retry, but the task is
        // re-resolved HERE at claim, and `orchestrator::worker` turns an
        // executor `Err` into `TaskOutcome::Failed` with nothing in the
        // orchestrator re-queueing it. So a run claimed while its revision was
        // still compiling died permanently for a condition that clears in
        // seconds.
        //
        // Safe to return early: this load runs BEFORE `pipeline_lease::try_acquire`
        // below, so no lease is held and the deferral correctly skips
        // `release_airway_lease`. If this load ever moves below the acquire,
        // that stops being true and the deferral must release first.
        let yaml =
            match crate::pipeline_ref::load_pipeline_yaml(self.platform.as_ref(), pipeline_ref)
                .await
            {
                Ok(y) => y,
                Err(e) => match action_for_load_failure(e) {
                    LoadFailureAction::Defer {
                        delay_secs,
                        max_wait_secs,
                        reason,
                    } => {
                        tracing::info!(
                            pipeline_ref,
                            reason = %reason,
                            "airway task deferred — pipeline yaml not resolvable on this node"
                        );
                        return Ok(deferred_task(delay_secs, max_wait_secs, reason));
                    }
                    LoadFailureAction::Fail(m) => return Err(m),
                },
            };
        // Render with the same `variables` that `start_airway_run`
        // validated against, so the worker's document matches what the
        // submitter saw.
        let mut spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, variables)
            .map_err(|e| format!("airway: parse `{pipeline_ref}`: {e}"))?;

        // SINGLE-FLIGHT, ACQUIRED HERE — at claim, not at submit.
        //
        // This is the only place an airway pipeline can start, which is what
        // makes the invariant real. Submit-time acquisition protected only the
        // callers that went through submit: an inline `TaskSpec::Airway` step
        // from an automation never did, so two workflows could run one pipeline
        // concurrently and nothing stopped them.
        //
        // Contention is NOT a failure. The task goes back to the queue
        // invisible for a while and tries again, so a contended pipeline
        // serializes instead of erroring — which is the whole point of the
        // redesign. `TaskOutcome::Deferred` is how a domain says that; the
        // worker turns it into the queue write.

        // The one id this run is scoped by: it keys the single-flight lease
        // below AND the cursor row the state store reads and writes. Read once
        // and reused so the two can never name different things — a run holding
        // one workspace's lease while advancing another's cursor is exactly the
        // collision the per-workspace state table exists to end.
        let workspace_id = self.platform.workspace_id();
        if !spec.allow_concurrent_runs {
            use agentic_airway::extension::pipeline_lease;
            match pipeline_lease::try_acquire(
                &self.db,
                workspace_id,
                &spec.name,
                run_id,
                pipeline_lease::LEASE_TTL_SECS,
            )
            .await
            {
                Ok(pipeline_lease::LeaseAcquisition::Acquired) => {}
                Ok(pipeline_lease::LeaseAcquisition::Held { run_id: holder, .. }) => {
                    tracing::info!(
                        pipeline = %spec.name,
                        held_by = %holder,
                        "airway task deferred — single-flight lease held"
                    );
                    return Ok(deferred_task(
                        AIRWAY_LEASE_RETRY_SECS,
                        AIRWAY_LEASE_MAX_WAIT_SECS,
                        format!(
                            "pipeline `{}` is already running ({holder}); waiting for its turn",
                            spec.name
                        ),
                    ));
                }
                Err(e) => {
                    return Err(format!("airway: single-flight lease acquire failed: {e}"));
                }
            }
        }

        // Capture QuickBooks' token var names *before* secret resolution
        // strips them. The write-back sink needs to know which secret to update
        // when Intuit rotates the token mid-run; the read-only source needs to
        // know which secret to re-read.
        let qb_var = |key: &str| -> Option<String> {
            (spec.source.kind == "quickbooks")
                .then(|| spec.source.config.get(key))
                .flatten()
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        // `access_token_var` selects READ-ONLY token custody: some other writer
        // (for Poke House, the scheduled `refresh-qb-token` Oxy Function) owns
        // this grant's rotation, and Intuit expires the previous refresh token
        // whenever it issues a new one — so a second refresher here would fork
        // the chain and brick the grant. Unlike every other `*_var`, this one is
        // deliberately NOT resolved into the spec: an access token lives ~60
        // minutes and a backfill outlives that, so it becomes a provider that
        // re-reads per request instead of a literal frozen at dispatch.
        let qb_tokens =
            quickbooks_token_vars(qb_var("access_token_var"), qb_var("refresh_token_var"))?;

        // Resource override (e.g. "retry failed tables"): restrict the run
        // to the named subset. The worker filters the source by
        // `spec.resources`, so this re-runs only those streams.
        if !resources.is_empty() {
            spec.resources = resources.to_vec();
        }

        let resumable_backfill = apply_backfill_window(
            &spec.source.kind,
            &mut spec.source.config,
            backfill_from.zip(backfill_to),
            pipeline_ref,
        )?;

        // An uploadable source's landing zone is SERVER-owned, so a pipeline
        // that omits `base_path` gets the derived one here rather than making
        // an operator transcribe a bucket, a workspace uuid and a path slug by
        // hand. A declared value is left alone: the source also reads zones
        // nothing derives (`/data/ubereats`, a pre-existing bucket).
        self.fill_upload_zone(&mut spec, pipeline_ref)?;
        self.resolve_airway_source_secrets(&mut spec).await?;
        let airhouse_db = self.resolve_airway_destination(&mut spec).await?;

        let db = Arc::new(self.db.clone());
        let mut worker = match qb_tokens {
            Some(QuickBooksTokenVar::ReadOnly(var_name)) => {
                let source: Arc<dyn agentic_airway::AccessTokenSource> =
                    Arc::new(PlatformAccessTokenSource {
                        platform: self.platform.clone(),
                        var_name,
                    });
                agentic_airway::AirwayWorker::with_access_token_source(db, source, admission)
            }
            Some(QuickBooksTokenVar::Rotating(var_name)) => {
                let sink: Arc<dyn agentic_airway::RefreshTokenSink> =
                    Arc::new(PlatformRefreshTokenSink {
                        platform: self.platform.clone(),
                        var_name,
                    });
                agentic_airway::AirwayWorker::with_refresh_sink(db, sink, admission)
            }
            None => agentic_airway::AirwayWorker::new(db, admission),
        };
        // Airhouse destinations hold/cycle one pgwire connection for the whole
        // load; attach a provider so each (re)connect re-mints a fresh
        // (non-expired) ephemeral credential instead of reusing the static DSN.
        if let Some((database, dataset_name)) = airhouse_db {
            let provider: Arc<dyn agentic_airway::CredentialProvider> =
                Arc::new(PlatformAirhouseCredentialProvider {
                    platform: self.platform.clone(),
                    database,
                    dataset_name,
                });
            worker = worker.with_credential_provider(provider);
        }
        // A resumable backfill drives the run-scoped state store keyed by run_id
        // (cursor → resume_state); everything else uses the pipeline-global store.
        let resume_run_id = resumable_backfill.then(|| run_id.to_string());
        Ok(worker.execute(spec, resume_run_id, run_id.to_string(), workspace_id))
    }

    /// Reset a pipeline's provisioned schema: drop its destination tables and
    /// clear this workspace's stored cursor + schema row so a later run
    /// re-infers a fresh schema from scratch. Returns the dropped table names.
    /// A same-named pipeline in another workspace is untouched.
    ///
    /// Airhouse destinations only — the destination must resolve from a
    /// `config.yml` database reference so the ephemeral credential can be
    /// re-minted for the drop (same posture as `execute_airway`).
    pub async fn reset_airway_schema(
        &self,
        pipeline_ref: &str,
    ) -> Result<Vec<String>, ResetSchemaError> {
        use ResetSchemaError::{BadRequest, Internal};

        // Resolve `pipeline_ref` → yaml → spec, mirroring `execute_airway`'s
        // first lines (compile boundary first, contained FS read on a miss).
        // No `variables` — a reset targets a pipeline's persisted state, keyed
        // by its rendered `name`. A bad ref / unparseable spec is caller input
        // → `BadRequest` (400).
        let yaml =
            match crate::pipeline_ref::load_pipeline_yaml(self.platform.as_ref(), pipeline_ref)
                .await
            {
                Ok(y) => y,
                // Both mean "this node cannot serve that ref", which is a 503
                // and not the caller's mistake — `NotInRevision` would
                // otherwise fall into the `BadRequest` catch-all below and
                // tell the caller their perfectly good ref was malformed.
                Err(crate::pipeline_ref::PipelineRefError::Unavailable(m))
                | Err(crate::pipeline_ref::PipelineRefError::NotInRevision(m)) => {
                    return Err(ResetSchemaError::Unavailable(format!("airway: {m}")));
                }
                Err(e) => return Err(BadRequest(format!("airway: {e}"))),
            };
        let mut spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
            .map_err(|e| BadRequest(format!("airway: parse `{pipeline_ref}`: {e}")))?;

        // The rendered spec `name` plus this workspace's id is the key of
        // `airway_workspace_pipeline_state` — the same pair the run's lease uses.
        let pipeline_name = spec.name.clone();
        let workspace_id = self.platform.workspace_id();

        // Table names in the stored schema — the set to drop. A DB read failure
        // here is server-side → `Internal` (500).
        let tables = agentic_airway::reset::stored_schema_table_names(
            &self.db,
            workspace_id,
            &pipeline_name,
        )
        .await
        .map_err(|e| Internal(e.to_string()))?;
        if tables.is_empty() {
            // Never provisioned (nothing to drop). Still clear any stale state
            // row so the next run is guaranteed a clean slate — cheap and
            // idempotent, and it skips destination resolution (which would
            // otherwise demand an airhouse credential we don't need here).
            agentic_airway::reset::clear_pipeline_state(&self.db, workspace_id, &pipeline_name)
                .await
                .map_err(|e| Internal(format!("reset clear-state: {e}")))?;
            return Ok(vec![]);
        }

        // Resolve the destination into an inline connector. This returns
        // `Some(db_name)` only for airhouse; anything else can't re-mint a
        // credential for the drop, so reject it (caller config → `BadRequest`).
        let airhouse_db = self
            .resolve_airway_destination(&mut spec)
            .await
            .map_err(Internal)?;
        let Some((database, dataset_name)) = airhouse_db else {
            return Err(BadRequest(
                "reset schema is only supported for airhouse destinations".into(),
            ));
        };
        let provider: Arc<dyn agentic_airway::CredentialProvider> =
            Arc::new(PlatformAirhouseCredentialProvider {
                platform: self.platform.clone(),
                database,
                dataset_name,
            });

        let inline_config = spec
            .destination
            .as_inline()
            .map_err(|e| Internal(format!("airway: {e}")))?;

        agentic_airway::reset::drop_destination_tables(inline_config, Some(provider), &tables)
            .await
            .map_err(|e| Internal(format!("reset drop: {e}")))?;

        // The drop succeeded; the airhouse tables are gone. If clearing the
        // state row now fails, the stored schema/cursors persist while the
        // tables don't — recoverable (drop is idempotent, a re-run re-infers),
        // but log this specific window so a post-mortem isn't guesswork.
        if let Err(e) =
            agentic_airway::reset::clear_pipeline_state(&self.db, workspace_id, &pipeline_name)
                .await
        {
            tracing::warn!(
                pipeline = %pipeline_name,
                workspace_id = %workspace_id,
                dropped_tables = tables.len(),
                error = %e,
                "reset: destination tables dropped but clearing the workspace's airway pipeline \
                 state failed; stored schema/cursors persist until a retry — a backfill re-infers",
            );
            return Err(Internal(format!("reset clear-state: {e}")));
        }
        Ok(tables)
    }

    /// Dispatch a `TaskSpec::Compile` through the host-supplied
    /// [`CompileDispatcher`] port. The actual worker (which touches the
    /// `entity` crate for the compile boundary schema) lives in the host
    /// — pipeline keeps no `oxy-compile` / `entity` deps per the
    /// layering rules.
    async fn execute_compile(
        &self,
        workspace_id: uuid::Uuid,
        git_sha: Option<String>,
        branch: Option<String>,
        promote: bool,
        kind: Option<&str>,
        owner_user_id: Option<uuid::Uuid>,
    ) -> Result<ExecutingTask, String> {
        let dispatcher = self.platform.compile_dispatcher().ok_or_else(|| {
            "compile: PlatformContext::compile_dispatcher() returned None — the host \
             needs to wire OxyCompileDispatcher (or equivalent) for compile tasks to run."
                .to_string()
        })?;
        dispatcher
            .dispatch(
                workspace_id,
                git_sha,
                branch,
                promote,
                kind.map(str::to_string),
                owner_user_id,
            )
            .await
    }

    /// Fill in an omitted `base_path` for an uploadable source kind.
    ///
    /// Never REDIRECTS a declared zone, only normalizes it. A pipeline that
    /// names its own zone is pointing at something this cannot derive, and the
    /// upload endpoint already refuses a *declared* zone that disagrees with
    /// where it writes, so re-checking here would duplicate that refusal in a
    /// place where the operator cannot see it.
    ///
    /// It does normalize a declared value through
    /// [`agentic_airway::upload_zone::normalize_base_path`] — the same helper
    /// the upload endpoint compares with — because comparing a normalized
    /// string and then forwarding the raw one is how `".../p/ "` passed the
    /// check and reached the connector with a trailing space.
    ///
    /// The derivation is shared with the upload endpoint
    /// ([`agentic_airway::upload_zone`]) precisely so the zone written to and
    /// the zone read from cannot drift apart.
    fn fill_upload_zone(
        &self,
        spec: &mut agentic_airway::AirwayPipelineSpec,
        pipeline_ref: &str,
    ) -> Result<(), String> {
        use agentic_airway::upload_zone;

        if !upload_zone::is_uploadable(&spec.source.kind) {
            return Ok(());
        }
        // `config` is `#[serde(default)]`, so an omitted block is Null rather
        // than an empty map — and `config: {}` is exactly the shape a pipeline
        // that declares nothing at all now has.
        if spec.source.config.is_null() {
            spec.source.config = serde_json::Value::Object(Default::default());
        }
        let Some(obj) = spec.source.config.as_object_mut() else {
            return Err(format!(
                "airway: source config for `{pipeline_ref}` is not a map"
            ));
        };
        // Everything about a DECLARED value is decided by a free function, so
        // all four arms are testable without an executor — only the derive
        // path below needs `self.platform`.
        if matches!(apply_declared_base_path(obj), DeclaredZone::Keep) {
            return Ok(());
        }

        let derived = upload_zone::derive_base_path(
            self.platform.workspace_id(),
            &spec.source.kind,
            pipeline_ref,
        )
        .map_err(|e| {
            format!("airway: `{pipeline_ref}` omits `base_path` and one cannot be derived: {e}")
        })?;
        tracing::debug!(pipeline_ref, base_path = %derived, "derived the upload zone");
        obj.insert("base_path".into(), serde_json::Value::String(derived));
        Ok(())
    }

    /// Substitute a source's `*_var` credential references with values from the
    /// platform secret manager, then strip the `_var` keys so the connector
    /// factory sees only resolved literals. Each source kind opts in
    /// explicitly to the (field, var-key) pairs it manages as secrets.
    async fn resolve_airway_source_secrets(
        &self,
        spec: &mut agentic_airway::AirwayPipelineSpec,
    ) -> Result<(), String> {
        let kind = spec.source.kind.clone();
        // rest_api carries its credential nested under `config.auth`
        // (`token_var`/`key_var`), not as a flat `config` field like the kinds
        // below, so it resolves through its own helper.
        if kind == "rest_api" {
            return self.resolve_rest_api_auth_secrets(spec).await;
        }
        // google_sheets carries no storable credential at all — see
        // `resolve_google_sheets_auth` for why the secret is the key, not a token.
        if kind == "google_sheets" {
            return self.resolve_google_sheets_auth(spec).await;
        }
        // (field, var-key) pairs each source kind supports as managed secrets.
        // `client_id` / `realm_id` are identifiers, not secrets. Kinds not
        // listed here carry no managed credentials.
        //
        // KNOWN / FOLLOW-UP (out of scope here): this table is stringly typed
        // and has no compile-time link to airway's per-kind source defs (same
        // for the `kind: String` discovery field in agentic-http). The planned
        // cleanup is to expose `Source::managed_secrets()` from each Params
        // struct so this stays in sync automatically.
        let pairs: &[(&str, &str)] = match kind.as_str() {
            "toast" => &[
                ("client_secret", "client_secret_var"),
                ("client_id", "client_id_var"),
            ],
            "quickbooks" => &[
                ("client_secret", "client_secret_var"),
                ("refresh_token", "refresh_token_var"),
            ],
            // Amazon SP-API. `client_id` is an identifier (it travels in every
            // LWA request body and identifies the app, not the seller); the LWA
            // client secret and the seller's refresh token are both bearer
            // credentials and both resolve from the secret manager.
            "sp_api" => &[
                ("client_secret", "client_secret_var"),
                ("refresh_token", "refresh_token_var"),
            ],
            // The PEM private key behind OAuth 2.0 client-credentials (M2M).
            // `account_id` / `client_id` / `certificate_id` are identifiers —
            // the account id is in every API hostname — so the key is the only
            // managed secret. Note it is multi-line, unlike every other entry
            // here: whatever backs the secret must preserve newlines, or the
            // connector refuses it at construction.
            "netsuite" => &[("private_key_pem", "private_key_var")],
            "clickhouse" => &[("password", "password_var")],
            // A whole DSN, not just a password: the per-org OLTP analyst
            // credential is sealed at rest and minted per org, so there is no
            // literal to commit. The literal `connection_string` still works
            // for a fixed, non-secret source — this only adds the indirection.
            // `postgres_cdc` shares the arm because it takes the same flat
            // `connection_string` (see `SqlDatabaseParams` / `PostgresCdcParams`
            // in agentic-airway's `source_factory`); its `slot_name` /
            // `publication_name` are server-side object names, not credentials.
            // Both params structs are `deny_unknown_fields` AND require
            // `connection_string`, so the strip below is what keeps a resolved
            // spec parseable at all, and a resolved-but-empty secret surfaces as
            // a loud "missing field `connection_string`" rather than a silent
            // anonymous connect.
            "sql_database" | "postgres_cdc" => &[("connection_string", "connection_string_var")],
            // Open-Meteo commercial API key → routes the connector to the paid
            // `customer-*` endpoint (the keyless endpoint is non-commercial only).
            "weather" => &[("api_key", "api_key_var")],
            // BestTime private API key → POSTed as `api_key_private` query
            // param to `/forecasts` (every call). Same pattern as `weather`.
            "besttime" => &[("api_key", "api_key_var")],
            _ => return Ok(()),
        };
        let Some(obj) = spec.source.config.as_object_mut() else {
            return Ok(());
        };
        for (field, var_key) in pairs {
            let Some(var_val) = obj.get(*var_key) else {
                continue;
            };
            let var_name = var_val.as_str().ok_or_else(|| {
                format!("airway {kind}: `{var_key}` must be a string secret name")
            })?;
            let secret = self
                .platform
                .resolve_secret(var_name)
                .await
                .ok_or_else(|| {
                    format!(
                        "airway {kind}: secret `{var_name}` (referenced by `{var_key}`) \
                     could not be resolved from the secret manager"
                    )
                })?;
            // A resolved-but-empty secret is treated as "unset": skip the
            // field insert so an absent credential stays absent (e.g.
            // ClickHouse must send no `X-ClickHouse-Key`, not an empty one —
            // see `clickhouse_conn` in agentic-airway). The `var_key` is
            // still removed so the rendered spec never leaks the indirection.
            if !secret.is_empty() {
                obj.insert((*field).to_string(), serde_json::Value::String(secret));
            }
            obj.remove(*var_key);
        }
        Ok(())
    }

    /// rest_api credentials live nested under `config.auth` as `token_var`
    /// (bearer) / `key_var` (`api_key` header + `api_key_query`), unlike the
    /// flat-config kinds in [`Self::resolve_airway_source_secrets`]. Resolve
    /// each from the platform secret manager into its literal `auth.{token,key}`
    /// field, then strip the `*_var` indirection so the connector factory sees
    /// only resolved literals.
    async fn resolve_rest_api_auth_secrets(
        &self,
        spec: &mut agentic_airway::AirwayPipelineSpec,
    ) -> Result<(), String> {
        for (field, var_key, var_name) in rest_api_secret_var_refs(&spec.source.config)? {
            let secret = self
                .platform
                .resolve_secret(&var_name)
                .await
                .ok_or_else(|| {
                    format!(
                        "airway rest_api: secret `{var_name}` (referenced by `auth.{var_key}`) \
                     could not be resolved from the secret manager"
                    )
                })?;
            set_rest_api_auth_secret(&mut spec.source.config, field, var_key, &secret);
        }
        Ok(())
    }

    /// `google_sheets` has no long-lived credential to store. A Google access
    /// token lives one hour, so a stored one makes a scheduled pipeline succeed
    /// exactly once and then 401 forever. The managed secret is therefore the
    /// service-account JSON key, and a fresh token is minted per run.
    ///
    /// The scope is fixed to read-only and deliberately not configurable: this
    /// connector only ever GETs `values/{range}`, and a sheet holding patient
    /// data should not be writable by a pipeline credential.
    async fn resolve_google_sheets_auth(
        &self,
        spec: &mut agentic_airway::AirwayPipelineSpec,
    ) -> Result<(), String> {
        let Some(obj) = spec.source.config.as_object_mut() else {
            return Ok(());
        };
        let Some(var_val) = obj.get("service_account_json_var") else {
            // No managed credential declared. Leave any literal `access_token`
            // untouched so a one-off manual run can still pass one directly.
            return Ok(());
        };
        let var_name = var_val
            .as_str()
            .ok_or("airway google_sheets: `service_account_json_var` must be a string secret name")?
            .to_string();
        let sa_json = self
            .platform
            .resolve_secret(&var_name)
            .await
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "airway google_sheets: secret `{var_name}` (referenced by \
                     `service_account_json_var`) could not be resolved from the secret manager"
                )
            })?;
        let token = mint_google_access_token(
            &sa_json,
            "https://www.googleapis.com/auth/spreadsheets.readonly",
        )
        .await?;
        obj.insert("access_token".to_string(), serde_json::Value::String(token));
        // Strip the indirection so the rendered spec never carries it.
        obj.remove("service_account_json_var");
        Ok(())
    }

    /// Turn a `destination: { database, dataset_name }` reference into a
    /// concrete inline connector by resolving the named `config.yml`
    /// database through the platform (secret substitution + per-subject
    /// `airhouse_managed` minting happen host-side). Inline destinations
    /// (the `memory` fixture, already-resolved specs) pass through.
    ///
    /// Returns `Some(database)` when it resolved to an **airhouse**
    /// destination, so the caller can attach a credential provider that
    /// re-mints the ephemeral credential on every (re)connect; `None`
    /// otherwise.
    async fn resolve_airway_destination(
        &self,
        spec: &mut agentic_airway::AirwayPipelineSpec,
    ) -> Result<Option<(String, String)>, String> {
        let agentic_airway::DestinationSpec::Reference(ref_) = &spec.destination else {
            return Ok(None);
        };
        let database = ref_.database.clone();
        let dataset_name = ref_.dataset_name.clone();
        let schema_separator = ref_.schema_separator.clone();
        let resolved = self
            .platform
            .resolve_pipeline_destination(&database, &dataset_name)
            .await
            .ok_or_else(|| {
                format!(
                    "airway: destination `database: {database}` is not a known \
                     config.yml database with an airway-writable type \
                     (postgres or airhouse)"
                )
            })?;
        // The resolver wins when it names a dataset: `postgres_managed` writes
        // into `raw_<source>`, a schema Oxy owns and granted this credential
        // rights on, so an author-chosen name would only fail at first write.
        let effective_dataset = resolved
            .dataset_name_override
            .clone()
            .unwrap_or_else(|| dataset_name.clone());
        let mut config = serde_json::json!({
            "connection_string": resolved.connection_string,
            "dataset_name": effective_dataset,
        });
        // `schema_separator` is an airhouse-only knob. Gate on the resolved
        // kind: other destinations (`postgres`, `memory`) deny unknown fields,
        // so emitting it would surface as an opaque YAML-parse error at run
        // start. Fail fast with a clear message instead.
        if let Some(sep) = schema_separator {
            if resolved.kind != "airhouse" {
                return Err(format!(
                    "airway: `schema_separator` only applies to airhouse destinations, \
                     but `database: {database}` resolves to `{}`. Remove `schema_separator` \
                     from the pipeline's destination.",
                    resolved.kind
                ));
            }
            config["schema_separator"] = serde_json::Value::String(sep);
        }
        let is_airhouse = resolved.kind == "airhouse";
        spec.destination =
            agentic_airway::DestinationSpec::Inline(agentic_airway::DestinationConfig {
                kind: resolved.kind,
                config,
            });
        Ok(is_airhouse.then_some((database, effective_dataset)))
    }
}

/// Exchange a Google service-account key for a one-hour access token.
///
/// The RS256 JWT-bearer flow, same as `crates/airform/src/adapter.rs` runs for
/// BigQuery. Duplicated rather than shared: the two differ only in scope, and
/// airform is a sibling this crate must not take a dependency on.
async fn mint_google_access_token(sa_json: &str, scope: &str) -> Result<String, String> {
    let key: serde_json::Value = serde_json::from_str(sa_json)
        .map_err(|e| format!("google service-account key is not valid JSON: {e}"))?;
    let client_email = key["client_email"]
        .as_str()
        .ok_or("google service-account key is missing `client_email`")?;
    let private_key_pem = key["private_key"]
        .as_str()
        .ok_or("google service-account key is missing `private_key`")?;
    // Present in every key Google issues; defaulted so a hand-trimmed key
    // still works rather than failing on a field nobody edits on purpose.
    let token_uri = key["token_uri"]
        .as_str()
        .unwrap_or("https://oauth2.googleapis.com/token");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock is before the unix epoch: {e}"))?
        .as_secs();

    #[derive(serde::Serialize)]
    struct Claims<'a> {
        iss: &'a str,
        scope: &'a str,
        aud: &'a str,
        exp: u64,
        iat: u64,
    }

    let assertion = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &Claims {
            iss: client_email,
            scope,
            aud: token_uri,
            exp: now + 3600,
            iat: now,
        },
        &jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
            .map_err(|e| format!("google service-account private key did not parse: {e}"))?,
    )
    .map_err(|e| format!("failed to sign the google service-account JWT: {e}"))?;

    let resp = reqwest::Client::new()
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("google token exchange failed: {e}"))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("google token response was not JSON: {e}"))?;
    body["access_token"]
        .as_str()
        .map(str::to_string)
        // A key the sheet was never shared with comes back 400 with
        // {"error", "error_description"}. Surfacing that beats reporting a
        // missing field, which reads like a bug on our side.
        .ok_or_else(|| {
            format!(
                "google token exchange returned {status} without an access_token: {}",
                body["error_description"]
                    .as_str()
                    .or_else(|| body["error"].as_str())
                    .unwrap_or("no error field in the response")
            )
        })
}

/// Collect `(target_field, var_key, secret_name)` triples for `*_var`
/// credential references nested under a rest_api source's `config.auth`:
/// `token_var` → `token` (bearer) and `key_var` → `key` (`api_key` header +
/// `api_key_query`). Pure — performs no secret lookup; the async resolution
/// happens in [`PipelineTaskExecutor::resolve_rest_api_auth_secrets`].
///
/// A present-but-non-string `*_var` is a hard error (matching the flat-config
/// path in [`PipelineTaskExecutor::resolve_airway_source_secrets`]); an absent
/// one is simply skipped.
fn rest_api_secret_var_refs(
    config: &serde_json::Value,
) -> Result<Vec<(&'static str, &'static str, String)>, String> {
    let mut refs = Vec::new();
    let Some(auth) = config.get("auth").and_then(|a| a.as_object()) else {
        return Ok(refs);
    };
    for (field, var_key) in [("token", "token_var"), ("key", "key_var")] {
        if let Some(value) = auth.get(var_key) {
            let name = value.as_str().ok_or_else(|| {
                format!("airway rest_api: `auth.{var_key}` must be a string secret name")
            })?;
            refs.push((field, var_key, name.to_string()));
        }
    }
    Ok(refs)
}

/// Apply one resolved rest_api auth secret in place: set `config.auth[field]`
/// to the resolved literal (skipped when empty — treated as "unset", mirroring
/// the flat-config kinds in [`PipelineTaskExecutor::resolve_airway_source_secrets`])
/// and always strip `config.auth[var_key]` so the rendered spec never leaks the
/// `*_var` indirection to the connector factory.
fn set_rest_api_auth_secret(
    config: &mut serde_json::Value,
    field: &str,
    var_key: &str,
    secret: &str,
) {
    let Some(auth) = config.get_mut("auth").and_then(|a| a.as_object_mut()) else {
        return;
    };
    if !secret.is_empty() {
        auth.insert(
            field.to_string(),
            serde_json::Value::String(secret.to_string()),
        );
    }
    auth.remove(var_key);
}

/// Persists a rotated OAuth refresh token back to the platform secret
/// store. Wired into the airway worker for `quickbooks` pipelines: when
/// Intuit rotates the refresh token mid-run, the connector calls
/// [`persist`](agentic_airway::RefreshTokenSink::persist) and we upsert
/// the new value under the same `*_var` secret name the run resolved from.
struct PlatformRefreshTokenSink {
    platform: Arc<dyn PlatformContext>,
    var_name: String,
}

#[async_trait]
impl agentic_airway::RefreshTokenSink for PlatformRefreshTokenSink {
    async fn persist(&self, refresh_token: &str) -> Result<(), String> {
        self.platform
            .persist_secret(&self.var_name, refresh_token)
            .await
    }
}

/// Treat a resolved-but-empty secret as unset.
///
/// `resolve_secret` returns `Some("")` for a secret row that exists with an
/// empty value, which is a configuration mistake rather than a credential. The
/// LLM-key readiness check in this crate already draws the line here; naming it
/// keeps the two consistent and makes the rule testable.
fn usable_secret(resolved: Option<String>) -> Option<String> {
    resolved.filter(|value| !value.trim().is_empty())
}

/// Which token-custody var a quickbooks spec declared, and the secret it names.
#[derive(Debug, PartialEq, Eq)]
enum QuickBooksTokenVar {
    /// `access_token_var` — the host owns rotation; this run only reads.
    ReadOnly(String),
    /// `refresh_token_var` — this run owns rotation and writes back.
    Rotating(String),
}

/// Decide token custody from the two `*_var` keys a quickbooks spec may carry.
///
/// **Declaring both is an error, not a precedence question.** They describe
/// mutually exclusive answers to "who rotates this grant?", and the whole point
/// of `QuickBooksTokens` being an enum is that resolving that silently has no
/// correct meaning — picking read-only would leave a `refresh_token_var` in the
/// YAML implying a write-back that never happens, and picking rotating would
/// fork a grant whose owner the config just named. The operator has to say which.
///
/// Pure so the choice is testable without a database or a spec fixture.
fn quickbooks_token_vars(
    access_token_var: Option<String>,
    refresh_token_var: Option<String>,
) -> Result<Option<QuickBooksTokenVar>, String> {
    match (access_token_var, refresh_token_var) {
        (Some(_), Some(_)) => Err(
            "airway quickbooks: `access_token_var` and `refresh_token_var` are mutually \
             exclusive — the first says another writer owns this grant's rotation, the \
             second says this pipeline does. Remove whichever does not apply."
                .into(),
        ),
        (Some(v), None) => Ok(Some(QuickBooksTokenVar::ReadOnly(v))),
        (None, Some(v)) => Ok(Some(QuickBooksTokenVar::Rotating(v))),
        (None, None) => Ok(None),
    }
}

/// Reads a host-maintained OAuth **access** token out of the secret store for a
/// QuickBooks source in read-only token custody.
///
/// `var_name` is typically an app-scoped secret written by a scheduled
/// refresher — e.g. `apps/<app_id>/QB_ACCESS_TOKEN`. That name contains `/`,
/// which `validate_secret_name` rejects on *write*; the read path does not
/// validate, which is why the app-scoped namespace is reachable from here at
/// all (writes go through `set_app_secret`, which builds the prefix itself).
///
/// Resolved per request rather than once per run — see the `access_token_var`
/// capture in `dispatch_airway` for why freezing it would break backfills.
/// `SecretManagerService` caches decrypted values for 300s, so this costs at
/// most one query per 5 minutes while still picking up a rotation well inside
/// the token's ~60-minute life.
struct PlatformAccessTokenSource {
    platform: Arc<dyn PlatformContext>,
    var_name: String,
}

#[async_trait]
impl agentic_airway::AccessTokenSource for PlatformAccessTokenSource {
    async fn access_token(&self) -> Result<String, String> {
        // `usable_secret` drops a resolved-but-empty value: it would otherwise
        // become `Authorization: Bearer ` on every request — a 401 loop that
        // read-only mode cannot refresh its way out of, reported with a message
        // naming neither the secret nor the cause.
        usable_secret(self.platform.resolve_secret(&self.var_name).await)
            // Fail loudly rather than falling back to a refresh: an unenrolled
            // or renamed grant must stop the run, not silently start a second
            // rotation chain.
            .ok_or_else(|| {
                format!(
                    "airway quickbooks: access token secret `{}` (referenced by \
                     `access_token_var`) is missing or empty in the secret manager",
                    self.var_name
                )
            })
    }
}

/// Re-mints a fresh `airhouse_managed` credential on every (re)connect for an
/// airway pipeline destination. Wired into the airway worker for airhouse
/// destinations: when the destination opens or cycles its long-lived pgwire
/// connection, it calls this to get a freshly-minted DSN.
///
/// DESIGN ASSUMPTION (verified against airhouse as of 0.x, but a CP property
/// not enforced here): a credential's `expires_at` is checked **only at the
/// SCRAM handshake** — `get_user_credentials` filters expired rows and is the
/// auth path's lookup — and never per-query, so an established session persists
/// past the credential's expiry (the ephemeral-user sweeper only reclaims
/// storage after a grace window, it doesn't drop live sessions). That's why
/// re-resolving (which re-mints via the broker) on each connect is sufficient
/// and the standard short TTL needs no bump. If airhouse ever starts validating
/// `expires_at` per-query, long single-segment loads would fail and this
/// provider would need to force a full-TTL mint (`evict_and_remint`) per cycle.
struct PlatformAirhouseCredentialProvider {
    platform: Arc<dyn PlatformContext>,
    database: String,
    dataset_name: String,
}

#[async_trait]
impl agentic_airway::CredentialProvider for PlatformAirhouseCredentialProvider {
    async fn connection_string(&self) -> Result<String, String> {
        self.platform
            .resolve_pipeline_destination(&self.database, &self.dataset_name)
            .await
            .map(|resolved| resolved.connection_string)
            .ok_or_else(|| {
                format!(
                    "airway: failed to re-resolve airhouse destination `{}` \
                     for credential refresh",
                    self.database
                )
            })
    }
}

mod automation;
mod cursor_reset;

pub use automation::run_decision_task;
pub use cursor_reset::{ClearedCursors, CursorResetRefusal, CursorScope, ResetCursorsError};

/// What to do with a pipeline's declared `base_path`.
#[derive(Debug, PartialEq, Eq)]
enum DeclaredZone {
    /// Nothing usable was declared — derive the zone.
    Derive,
    /// A declared value stands (normalized in place if it needed it), or is a
    /// non-string the connector factory should reject by type.
    Keep,
}

/// Normalize a declared `base_path` in place, and say whether one was declared
/// at all.
///
/// Split out from `fill_upload_zone` so every arm is reachable in a test: the
/// remaining half needs `self.platform` for the workspace id, this half needs
/// nothing. The arms are exhaustive over what YAML can put here — string,
/// blank string, null, absent, non-string.
fn apply_declared_base_path(obj: &mut serde_json::Map<String, serde_json::Value>) -> DeclaredZone {
    // The decision is taken while `obj` is borrowed, and the write happens
    // after that borrow ends — `match obj.get(..)` holds the shared borrow
    // across every arm, so inserting inside one does not compile.
    enum Decision {
        Derive,
        Keep,
        Rewrite(String),
    }

    let decision = match obj.get("base_path") {
        // Absent, or an explicit `null` — which reads as "no value" in YAML and
        // would otherwise reach the factory as a type error.
        None | Some(serde_json::Value::Null) => Decision::Derive,
        Some(serde_json::Value::String(declared)) => {
            match agentic_airway::upload_zone::normalize_base_path(declared) {
                // Blank, whitespace, or nothing but slashes: it names nowhere,
                // so it has not disagreed with anything — derive.
                None => Decision::Derive,
                Some(normalized) if normalized != declared.as_str() => {
                    Decision::Rewrite(normalized.to_string())
                }
                Some(_) => Decision::Keep,
            }
        }
        // Present and not a string: left for the connector factory to reject by
        // type rather than silently derived over.
        Some(_) => Decision::Keep,
    };

    match decision {
        Decision::Derive => DeclaredZone::Derive,
        Decision::Keep => DeclaredZone::Keep,
        Decision::Rewrite(normalized) => {
            obj.insert("base_path".into(), serde_json::Value::String(normalized));
            DeclaredZone::Keep
        }
    }
}

/// Kinds whose airway source reads `backfill_start` / `backfill_end`.
///
/// `pub` so the several doc comments that describe this list — in `airway_run`,
/// in `agentic-airway`'s `task_spec`, on the `/backfill` route — can name it
/// rather than restate it. Restating is what put four of them out of date at
/// once when `sp_api` was added.
///
/// Adding a kind here is only half the job — the source builder in
/// `agentic-airway`'s `source_factory` has to parse the pair and hand it to the
/// connector, or the window is accepted here and silently ignored there.
pub const WINDOWED_BACKFILL_KINDS: [&str; 3] = ["toast", "quickbooks", "sp_api"];

/// Kinds whose backfill cursor must land in the RUN-SCOPED store.
///
/// A resumable backfill advances its cursor; against the pipeline-global store
/// that advance would move a historical replay's position onto the live
/// incremental one. QuickBooks is absent on purpose — it freezes its cursor
/// instead, so it has nothing to isolate.
///
/// sp_api is here for a reason neither of the others shares: one window becomes
/// N Amazon report jobs, each costing tens of minutes of build time on Amazon's
/// side, and the connector persists the reportId so an interrupted run picks
/// the same job back up. A frozen cursor throws that away on every retry and
/// pays the build cost again from the start of the window.
pub const RESUMABLE_BACKFILL_KINDS: [&str; 2] = ["toast", "sp_api"];

/// Put this run's backfill window into the source config, and refuse a kind
/// that cannot honour one. Returns whether the cursor needs the run-scoped
/// store.
///
/// Two rules, and the first is easy to mistake for housekeeping:
///
/// 1. **A hand-written window is always stripped**, whether or not this run is
///    a backfill. These keys are injection-only and the executor is their sole
///    writer. A pair left in the YAML would otherwise make every normal and
///    scheduled run replay that window forever — a backfill freezes or
///    run-scopes the cursor, so the live one never advances past it.
/// 2. **Only a kind that actually reads the pair may be given one**, so a
///    window asked for on any other source fails loudly instead of quietly
///    running an unbounded load that looks like it honoured the request.
///
/// Pure over the config so both rules are testable. That is not gratuitous:
/// rule 1 is what stands between a hand-edited pipeline file and the symptom in
/// oxygen-internal#3092 — a run pinned to a date range nobody asked for on that
/// run — and it became load-bearing for `sp_api` the moment its params struct
/// started accepting these keys. `SpApiParams` is `deny_unknown_fields`, so
/// before that a stray pair was a hard parse error; now it is a window, and the
/// only thing keeping it off an incremental run is this strip.
fn apply_backfill_window(
    source_kind: &str,
    config: &mut serde_json::Value,
    window: Option<(&str, &str)>,
    pipeline_ref: &str,
) -> Result<bool, String> {
    if let Some(obj) = config.as_object_mut() {
        obj.remove("backfill_start");
        obj.remove("backfill_end");
    }
    let Some((from, to)) = window else {
        return Ok(false);
    };
    if !WINDOWED_BACKFILL_KINDS.contains(&source_kind) {
        return Err(format!(
            "airway backfill: source kind `{source_kind}` does not support a date-window \
             backfill (supported: {})",
            WINDOWED_BACKFILL_KINDS.join(", ")
        ));
    }
    let obj = config.as_object_mut().ok_or_else(|| {
        format!("airway backfill: source config for `{pipeline_ref}` is not a map")
    })?;
    obj.insert(
        "backfill_start".into(),
        serde_json::Value::String(from.to_string()),
    );
    obj.insert(
        "backfill_end".into(),
        serde_json::Value::String(to.to_string()),
    );
    Ok(RESUMABLE_BACKFILL_KINDS.contains(&source_kind))
}

#[cfg(test)]
mod backfill_window_tests {
    use super::*;
    use serde_json::json;

    /// **A window a user hand-wrote into the YAML never survives**, backfill or
    /// not.
    ///
    /// This is the rule that keeps oxygen-internal#3092's symptom — a run
    /// pinned to a range nobody asked for on that run — off the incremental
    /// path. It matters more for `sp_api` than for the others: `SpApiParams` is
    /// `deny_unknown_fields`, so a stray pair used to be a loud parse failure,
    /// and now that the params struct accepts these keys this strip is the only
    /// thing between a hand-edited file and a permanently pinned pipeline.
    #[test]
    fn a_hand_written_window_is_stripped_from_an_ordinary_run() {
        for kind in ["sp_api", "toast", "quickbooks", "stripe"] {
            let mut config = json!({
                "marketplace_id": "A2EUQ1WTGCTBG2",
                "backfill_start": "2026-08-01T00:00:00Z",
                "backfill_end": "2026-09-01T00:00:00Z",
            });
            let resumable = apply_backfill_window(kind, &mut config, None, "p.airway.yml")
                .expect("an ordinary run is never refused");
            assert!(!resumable, "{kind}");
            assert_eq!(
                config,
                json!({ "marketplace_id": "A2EUQ1WTGCTBG2" }),
                "{kind} kept a window nobody asked for on this run"
            );
        }
    }

    /// A backfill injects the window it was actually given — over any hand-
    /// written pair, which the strip removes first.
    #[test]
    fn a_backfill_injects_its_own_window_over_a_hand_written_one() {
        let mut config = json!({
            "backfill_start": "2020-01-01T00:00:00Z",
            "backfill_end": "2020-02-01T00:00:00Z",
        });
        let resumable = apply_backfill_window(
            "sp_api",
            &mut config,
            Some(("2026-03-01T00:00:00Z", "2026-09-01T00:00:00Z")),
            "p.airway.yml",
        )
        .expect("sp_api accepts a window");
        assert!(resumable, "sp_api backfills are resumable");
        assert_eq!(config["backfill_start"], "2026-03-01T00:00:00Z");
        assert_eq!(config["backfill_end"], "2026-09-01T00:00:00Z");
    }

    /// Which kinds may be given a window, and which of those advance a cursor
    /// that needs the run-scoped store. QuickBooks takes a window and freezes
    /// its cursor, so it is the one that separates the two lists.
    #[test]
    fn only_the_kinds_that_read_a_window_accept_one() {
        let window = Some(("2026-03-01T00:00:00Z", "2026-09-01T00:00:00Z"));
        for (kind, accepted, resumable) in [
            ("sp_api", true, true),
            ("toast", true, true),
            ("quickbooks", true, false),
            ("stripe", false, false),
            ("filesystem", false, false),
        ] {
            let mut config = json!({});
            match apply_backfill_window(kind, &mut config, window, "p.airway.yml") {
                Ok(got) => {
                    assert!(accepted, "{kind} should have been refused");
                    assert_eq!(got, resumable, "{kind} resumability");
                }
                Err(e) => {
                    assert!(!accepted, "{kind} should have been accepted: {e}");
                    // The message names the kind AND the alternatives, because
                    // an operator reading it is deciding what to do next.
                    assert!(e.contains(kind), "{e}");
                    assert!(e.contains("sp_api"), "{e}");
                }
            }
        }
    }

    /// Every kind that is resumable must also be one that takes a window at
    /// all. The two lists are edited separately and a kind in only the second
    /// would silently select the run-scoped store for a source that never
    /// receives a window.
    #[test]
    fn resumable_kinds_are_a_subset_of_windowed_kinds() {
        for kind in RESUMABLE_BACKFILL_KINDS {
            assert!(
                WINDOWED_BACKFILL_KINDS.contains(&kind),
                "{kind} is resumable but cannot be given a window"
            );
        }
    }

    /// A config that is not a map is refused rather than silently dropped —
    /// and the message names the pipeline, since that is what the operator has
    /// to go and edit.
    #[test]
    fn a_non_map_config_is_refused_by_name() {
        let mut config = json!("not-a-map");
        let err = apply_backfill_window(
            "sp_api",
            &mut config,
            Some(("2026-03-01T00:00:00Z", "2026-09-01T00:00:00Z")),
            "pipelines/amazon_sp.airway.yml",
        )
        .expect_err("a scalar config cannot carry a window");
        assert!(err.contains("pipelines/amazon_sp.airway.yml"), "{err}");
    }
}

#[cfg(test)]
mod tests {
    fn cfg(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::json!({ "base_path": v }) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    /// `".../p/ "` is the string this whole change exists for: the endpoint
    /// compared a normalized value and the executor forwarded the raw one, so
    /// a trailing space reached the connector inside the object key.
    #[test]
    fn a_declared_zone_is_normalized_in_place() {
        let mut obj = cfg(serde_json::json!("s3://z/p/ "));
        assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Keep);
        assert_eq!(obj["base_path"], serde_json::json!("s3://z/p"));

        // Slashes and whitespace collapse in one pass, in any interleaving.
        let mut obj = cfg(serde_json::json!(" s3://z/p /"));
        assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Keep);
        assert_eq!(obj["base_path"], serde_json::json!("s3://z/p"));

        // Already clean: kept byte-for-byte.
        let mut obj = cfg(serde_json::json!("/tmp/ue"));
        assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Keep);
        assert_eq!(obj["base_path"], serde_json::json!("/tmp/ue"));
    }

    /// A zone that names nowhere has not disagreed with anything, so it
    /// derives — the same reading the upload endpoint takes.
    #[test]
    fn a_zone_that_names_nowhere_derives() {
        for blank in ["", "   ", "/", "///"] {
            let mut obj = cfg(serde_json::json!(blank));
            assert_eq!(
                apply_declared_base_path(&mut obj),
                DeclaredZone::Derive,
                "`{blank}` names nowhere, so it is absent rather than wrong"
            );
        }
    }

    #[test]
    fn absent_and_null_both_derive() {
        let mut obj = cfg(serde_json::Value::Null);
        assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Derive);

        let mut obj = serde_json::Map::new();
        assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Derive);
    }

    /// Not derived over: the connector factory rejects it by type, which names
    /// the actual problem instead of silently substituting a zone.
    #[test]
    fn a_non_string_zone_is_left_for_the_factory() {
        for bad in [
            serde_json::json!(42),
            serde_json::json!(true),
            serde_json::json!(["s3://z"]),
        ] {
            let mut obj = cfg(bad.clone());
            assert_eq!(apply_declared_base_path(&mut obj), DeclaredZone::Keep);
            assert_eq!(obj["base_path"], bad, "left untouched: {bad}");
        }
    }

    use super::*;

    #[test]
    fn builder_agent_id_routes_to_builder() {
        assert!(is_builder_agent("__builder__"));
    }

    // ── QuickBooks token custody ─────────────────────────────────────────

    #[test]
    fn access_token_var_selects_read_only_custody() {
        assert_eq!(
            quickbooks_token_vars(Some("apps/app-id/QB_ACCESS_TOKEN".into()), None).unwrap(),
            Some(QuickBooksTokenVar::ReadOnly(
                "apps/app-id/QB_ACCESS_TOKEN".into()
            ))
        );
    }

    #[test]
    fn refresh_token_var_selects_rotating_custody() {
        assert_eq!(
            quickbooks_token_vars(None, Some("QB_REFRESH_TOKEN".into())).unwrap(),
            Some(QuickBooksTokenVar::Rotating("QB_REFRESH_TOKEN".into()))
        );
    }

    #[test]
    fn no_token_var_means_no_hook() {
        assert_eq!(quickbooks_token_vars(None, None).unwrap(), None);
    }

    /// Declaring both is ambiguous about who rotates the grant, and either
    /// silent resolution is wrong — so it is refused rather than ordered.
    #[test]
    fn declaring_both_token_vars_is_refused() {
        let err = quickbooks_token_vars(Some("A".into()), Some("R".into()))
            .err()
            .expect("expected refusal");
        assert!(
            err.contains("mutually exclusive"),
            "unexpected error: {err}"
        );
    }

    /// A secret that exists but is empty must read as unset. Otherwise it
    /// becomes `Authorization: Bearer ` on every request — a 401 loop that
    /// read-only mode cannot refresh its way out of.
    #[test]
    fn an_empty_secret_reads_as_unset() {
        assert_eq!(usable_secret(None), None);
        assert_eq!(usable_secret(Some(String::new())), None);
        assert_eq!(usable_secret(Some("   ".into())), None);
        assert_eq!(usable_secret(Some("\n".into())), None);
        assert_eq!(
            usable_secret(Some("eyJ.abc".into())).as_deref(),
            Some("eyJ.abc")
        );
    }

    #[test]
    fn regular_agent_id_routes_to_analytics() {
        assert!(!is_builder_agent("revenue"));
        assert!(!is_builder_agent("duckdb"));
        assert!(!is_builder_agent(""));
    }

    #[test]
    fn rest_api_bearer_token_var_is_collected() {
        let config = serde_json::json!({
            "base_url": "https://api.yelp.com/v3",
            "auth": { "type": "bearer", "token_var": "YELP_API_KEY" }
        });
        assert_eq!(
            rest_api_secret_var_refs(&config).unwrap(),
            vec![("token", "token_var", "YELP_API_KEY".to_string())]
        );
    }

    #[test]
    fn rest_api_api_key_query_key_var_is_collected() {
        let config = serde_json::json!({
            "auth": { "type": "api_key_query", "key_var": "CENSUS_API_KEY", "param": "key" }
        });
        assert_eq!(
            rest_api_secret_var_refs(&config).unwrap(),
            vec![("key", "key_var", "CENSUS_API_KEY".to_string())]
        );
    }

    #[test]
    fn rest_api_literal_or_absent_auth_collects_no_refs() {
        // already-literal token (no `*_var` indirection) → nothing to resolve
        let literal = serde_json::json!({ "auth": { "type": "bearer", "token": "sk-literal" } });
        assert!(rest_api_secret_var_refs(&literal).unwrap().is_empty());
        // no auth block at all (e.g. keyless public API like nces_schools)
        let none = serde_json::json!({ "base_url": "https://example.com" });
        assert!(rest_api_secret_var_refs(&none).unwrap().is_empty());
    }

    #[test]
    fn rest_api_non_string_var_is_a_hard_error() {
        // A present-but-non-string `*_var` is a config typo; error loudly like
        // the flat-config path rather than silently skipping the credential.
        let config = serde_json::json!({
            "auth": { "type": "bearer", "token_var": 123 }
        });
        let err = rest_api_secret_var_refs(&config).unwrap_err();
        assert!(err.contains("must be a string secret name"), "got: {err}");
    }

    #[test]
    fn set_rest_api_auth_secret_writes_field_and_strips_var() {
        let mut config = serde_json::json!({
            "auth": { "type": "bearer", "token_var": "YELP_API_KEY" }
        });
        set_rest_api_auth_secret(&mut config, "token", "token_var", "sk-abc123");
        assert_eq!(config["auth"]["token"], "sk-abc123");
        assert!(
            config["auth"].get("token_var").is_none(),
            "the `*_var` indirection must be stripped so the connector never sees it"
        );
    }

    #[test]
    fn set_rest_api_auth_secret_empty_secret_skips_field_and_strips_var() {
        // An empty resolved secret is "unset": don't write an empty token, but
        // still strip the var so the rendered spec carries no indirection.
        let mut config = serde_json::json!({
            "auth": { "type": "bearer", "token_var": "YELP_API_KEY" }
        });
        set_rest_api_auth_secret(&mut config, "token", "token_var", "");
        assert!(config["auth"].get("token").is_none());
        assert!(config["auth"].get("token_var").is_none());
    }

    // ── flat-config source secrets (`resolve_airway_source_secrets`) ─────

    /// A `ProjectContext` that knows exactly one secret. `resolve_airway_source_secrets`
    /// touches nothing else on the platform port, so the rest is stubbed.
    struct OneSecretPlatform {
        name: &'static str,
        value: &'static str,
    }

    #[async_trait]
    impl crate::platform::ProjectContext for OneSecretPlatform {
        async fn resolve_connector(
            &self,
            _db_name: &str,
        ) -> Option<agentic_connector::ConnectorConfig> {
            None
        }
        async fn resolve_model(
            &self,
            _model_ref: Option<&str>,
            _has_explicit_model: bool,
        ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
            None
        }
        async fn resolve_secret(&self, var_name: &str) -> Option<String> {
            (var_name == self.name).then(|| self.value.to_string())
        }
    }

    #[async_trait]
    impl agentic_automation::WorkspaceContext for OneSecretPlatform {
        fn workspace_path(&self) -> Option<&std::path::Path> {
            None
        }
        fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
            vec![]
        }
        async fn get_connector(
            &self,
            _name: &str,
        ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
            Err("unused".into())
        }
        async fn get_integration(
            &self,
            _name: &str,
        ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
            Err("unused".into())
        }
        async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
            Ok(vec![])
        }
        async fn resolve_automation_yaml(
            &self,
            _r: &str,
        ) -> Result<String, crate::WorkspaceReadError> {
            Err("unused".into())
        }
    }

    fn executor_knowing(name: &'static str, value: &'static str) -> PipelineTaskExecutor {
        PipelineTaskExecutor {
            platform: Arc::new(OneSecretPlatform { name, value }),
            builder_bridges: None,
            schema_cache: None,
            builder_test_runner: None,
            builder_app_runner: None,
            // Never queried: `resolve_airway_source_secrets` reads only the
            // platform port. A `Disconnected` handle errors on any query, so a
            // future DB read here fails the test rather than passing quietly.
            db: DatabaseConnection::default(),
            state: None,
            custom_executors: None,
        }
    }

    fn spec_with_source(
        kind: &str,
        config: serde_json::Value,
    ) -> agentic_airway::AirwayPipelineSpec {
        serde_json::from_value(serde_json::json!({
            "name": "oltp_orders",
            "source": { "kind": kind, "config": config },
            "destination": { "database": "airhouse", "dataset_name": "raw_oltp" },
        }))
        .expect("fixture spec must parse")
    }

    /// The per-org OLTP analyst DSN is a sealed credential, so a `sql_database`
    /// pipeline must be able to name it instead of committing the literal.
    /// `SqlDatabaseParams` is `deny_unknown_fields`, so the strip is not
    /// cosmetic: a surviving `connection_string_var` fails the source build.
    #[tokio::test]
    async fn sql_database_connection_string_var_is_resolved_and_stripped() {
        let executor = executor_knowing(
            "OLTP_ANALYST_DSN",
            "postgresql://analyst:pw@db.example:5432/org_42",
        );
        let mut spec = spec_with_source(
            "sql_database",
            serde_json::json!({
                "connection_string_var": "OLTP_ANALYST_DSN",
                "backend": "postgres",
                "tables": [{ "name": "orders" }],
            }),
        );

        executor
            .resolve_airway_source_secrets(&mut spec)
            .await
            .expect("the secret is resolvable");

        assert_eq!(
            spec.source.config["connection_string"],
            "postgresql://analyst:pw@db.example:5432/org_42"
        );
        assert!(
            spec.source.config.get("connection_string_var").is_none(),
            "the `*_var` indirection must be stripped so the connector never sees it"
        );
        // Untouched neighbours: the arm resolves one field, not the block.
        assert_eq!(spec.source.config["backend"], "postgres");
    }

    /// `postgres_cdc` rides the same arm because it takes the same flat
    /// `connection_string`. Same assertion so the two cannot drift apart.
    #[tokio::test]
    async fn postgres_cdc_connection_string_var_is_resolved_and_stripped() {
        let executor = executor_knowing("CDC_DSN", "postgresql://cdc@db.example/org_42");
        let mut spec = spec_with_source(
            "postgres_cdc",
            serde_json::json!({
                "connection_string_var": "CDC_DSN",
                "slot_name": "oxy_slot",
                "publication_name": "oxy_pub",
            }),
        );

        executor
            .resolve_airway_source_secrets(&mut spec)
            .await
            .expect("the secret is resolvable");

        assert_eq!(
            spec.source.config["connection_string"],
            "postgresql://cdc@db.example/org_42"
        );
        assert!(spec.source.config.get("connection_string_var").is_none());
        assert_eq!(spec.source.config["slot_name"], "oxy_slot");
    }

    /// A literal DSN carries no `*_var`, so the arm is a no-op for the fixed,
    /// non-secret source that the existing shape already served.
    #[tokio::test]
    async fn sql_database_literal_connection_string_is_left_alone() {
        let executor = executor_knowing("UNUSED", "unused");
        let mut spec = spec_with_source(
            "sql_database",
            serde_json::json!({
                "connection_string": "sqlite://./fixtures/demo.db",
                "backend": "sqlite",
            }),
        );

        executor
            .resolve_airway_source_secrets(&mut spec)
            .await
            .expect("nothing to resolve");

        assert_eq!(
            spec.source.config["connection_string"],
            "sqlite://./fixtures/demo.db"
        );
    }

    /// An unresolvable secret is a hard error, not a silent anonymous connect.
    #[tokio::test]
    async fn sql_database_unresolvable_connection_string_var_is_an_error() {
        let executor = executor_knowing("SOME_OTHER_SECRET", "x");
        let mut spec = spec_with_source(
            "sql_database",
            serde_json::json!({
                "connection_string_var": "OLTP_ANALYST_DSN",
                "backend": "postgres",
            }),
        );

        let err = executor
            .resolve_airway_source_secrets(&mut spec)
            .await
            .expect_err("an unresolvable secret must fail loudly");
        assert!(err.contains("OLTP_ANALYST_DSN"), "got: {err}");
    }
}

#[cfg(test)]
mod load_failure_action_tests {
    use super::{
        AIRWAY_LEASE_MAX_WAIT_SECS, AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS,
        AIRWAY_UNAVAILABLE_MAX_WAIT_SECS, AIRWAY_UNAVAILABLE_RETRY_SECS, LoadFailureAction,
        action_for_load_failure,
    };
    use crate::pipeline_ref::PipelineRefError;

    /// The regression guard for the shared deferral clock.
    ///
    /// `defer_task` compares whatever ceiling a defer passes against
    /// `first_deferred_at` — the first defer of the streak, *any* reason. Both
    /// deferral kinds in `execute_airway` write that one column, so a smaller
    /// ceiling here silently governs the lease's: a task already waiting its
    /// turn is dead-lettered by its first unavailable defer. This assertion is
    /// the only thing standing between a plausible-looking "300s is plenty for
    /// a compile" edit and that bug.
    #[test]
    fn the_unavailable_ceiling_never_undercuts_the_lease_ceiling() {
        assert!(
            AIRWAY_UNAVAILABLE_MAX_WAIT_SECS >= AIRWAY_LEASE_MAX_WAIT_SECS,
            "the two deferral kinds share `first_deferred_at`, so the smaller \
             ceiling wins for both: {AIRWAY_UNAVAILABLE_MAX_WAIT_SECS} < \
             {AIRWAY_LEASE_MAX_WAIT_SECS}"
        );
    }

    /// The disposition this branch exists to change: the worker turns an
    /// executor `Err` into `TaskOutcome::Failed` and nothing re-queues it, so
    /// failing here is permanent for a condition that clears in seconds.
    #[test]
    fn unavailable_defers_rather_than_failing() {
        let action = action_for_load_failure(PipelineRefError::Unavailable("db blip".into()));
        assert_eq!(
            action,
            LoadFailureAction::Defer {
                delay_secs: AIRWAY_UNAVAILABLE_RETRY_SECS,
                max_wait_secs: AIRWAY_UNAVAILABLE_MAX_WAIT_SECS,
                reason: "db blip".to_string(),
            }
        );
    }

    /// The other side of the line: deferring a bad ref would retry it until the
    /// ceiling and then dead-letter it, turning a clear error into a slow one.
    #[test]
    fn a_bad_ref_still_fails_immediately() {
        for e in [
            PipelineRefError::Invalid("pipeline_ref \"x\" not found".into()),
            PipelineRefError::Io("permission denied".into()),
        ] {
            assert!(
                matches!(action_for_load_failure(e), LoadFailureAction::Fail(_)),
                "only the two `can't answer here` variants may defer"
            );
        }
    }

    /// `NotInRevision` defers like `Unavailable` but on the SHORT ceiling.
    ///
    /// The pair is the whole point of splitting the variant: both are "not
    /// this node's answer to give", so both defer rather than failing a run
    /// whose compile is seconds away — but one of them has been answered by
    /// the boundary and the other has not, and only the unanswered one
    /// deserves twelve hours. Asserting the ceilings *differ* is what stops
    /// the split collapsing back into one disposition in a later tidy-up.
    #[test]
    fn not_in_revision_defers_on_the_short_ceiling() {
        let action = action_for_load_failure(PipelineRefError::NotInRevision("gone".into()));
        assert_eq!(
            action,
            LoadFailureAction::Defer {
                delay_secs: AIRWAY_UNAVAILABLE_RETRY_SECS,
                max_wait_secs: AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS,
                reason: "gone".to_string(),
            }
        );
        assert!(
            AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS < AIRWAY_UNAVAILABLE_MAX_WAIT_SECS,
            "an answered `no` must not wait as long as an unanswered question: \
             {AIRWAY_NOT_IN_REVISION_MAX_WAIT_SECS} vs \
             {AIRWAY_UNAVAILABLE_MAX_WAIT_SECS}"
        );
    }
}
