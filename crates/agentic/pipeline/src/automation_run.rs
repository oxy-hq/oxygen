//! Shared start/snapshot helpers for automation runs.
//!
//! HTTP, CLI, MCP, and eval all need the same primitive: "create an
//! agentic_run, enqueue a `TaskSpec::Automation`, then either watch events
//! flow or fetch the final state." Putting that here keeps the call sites
//! tiny and ensures they all observe the same retry/cache semantics.
//!
//! [`start_automation_run`] only seeds the DB — it does not run anything.
//! The HTTP handler pairs it with [`spawn_automation_run_drive`], which
//! starts a coordinator + worker that claim the queued task and drive
//! it to completion (events stream out via `agentic_run_events` to the
//! shared SSE registry).

use std::collections::HashMap;
use std::sync::Arc;

use agentic_automation::extension::AutomationRunState;
use agentic_automation::{
    AutomationConfig, AutomationDecider, AutomationDecision, WorkspaceContext, run_automation_step,
};
use agentic_core::delegation::{ChildCompletion, DelegationItem, DelegationTarget, TaskSpec};
use agentic_core::hub_task::spawn_with_hub;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::DurableTransport;
use agentic_runtime::worker::Worker;
use sea_orm::{DatabaseConnection, DbErr};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::executor::PipelineTaskExecutor;
use crate::platform::PlatformContext;

/// Inputs for [`start_automation_run`].
#[derive(Debug, Clone, Deserialize)]
pub struct StartAutomationRequest {
    /// Path to a `.automation.yml` / `.procedure.yml`,
    /// relative to the workspace root.
    pub workflow_ref: String,
    /// Optional variables for the automation's render context.
    #[serde(default)]
    pub variables: Option<Value>,
    /// Prior run id whose successful step results may be reused.
    #[serde(default)]
    pub retry_from_run_id: Option<String>,
    /// Opt-in cache flag; only consulted when `retry_from_run_id` is set.
    #[serde(default)]
    pub cache_enabled: bool,
    /// Step names to force-invalidate on this retry — even if the prior
    /// run's hash for them matches, the decider treats them as cache
    /// misses and re-executes. The cascade naturally invalidates
    /// downstream steps because their `render_context` depends on the
    /// re-run step's fresh result.
    ///
    /// Persisted on the new run's `agentic_runs.metadata` so the
    /// executor's per-decision `prior_state` filter can read it on every
    /// decision pass without a dedicated column.
    #[serde(default)]
    pub invalidate_steps: Option<Vec<String>>,
    /// Per-step iteration indices to force-replay on this retry, ignoring
    /// the per-iteration cache. Map of step name → indices.
    ///
    /// Composes with `cache_enabled` + `retry_from_run_id`: the decider's
    /// loop branch checks this map and skips cache-reuse for any
    /// `(step_name, index)` pair listed here, even if the prior status
    /// was `"done"`. Used by the UI's "Retry with overrides" flow when
    /// an admin wants to re-run specific iterations whose outputs they
    /// know to be stale.
    ///
    /// Persisted at seed onto the new run's
    /// `agentic_workflow_state.invalidate_iterations` column so the
    /// decider reads inline (no per-decision metadata round-trip).
    #[serde(default)]
    pub invalidate_iterations: Option<HashMap<String, Vec<usize>>>,
    /// Thread to associate this run with. When set, the run row is linked
    /// via `agentic_runs.thread_id`, which lets the chat-thread automation
    /// page recover state on reload — without the link, an in-memory-only
    /// zustand log buffer is the sole source of truth and the page goes
    /// blank after a refresh.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Soft FK → `agentic_schedules.id`. Internal-only — only the scheduler
    /// fire path sets this; HTTP/CLI input cannot, so callers can't spoof
    /// which schedule a run "came from".
    #[serde(skip_deserializing, default)]
    pub schedule_id: Option<String>,
    /// How this run was triggered: `"scheduled"`, `"manual"`, `"backfill"`.
    /// Internal-only — see `schedule_id` for the same reasoning. Stamped
    /// onto `agentic_runs.metadata.trigger` so the dashboard can show
    /// where a run came from.
    #[serde(skip_deserializing, default)]
    pub trigger: Option<String>,
    /// The cron-scheduled time this run is replaying (UTC). Set by the
    /// backfill path so downstream date-aware logic can use the
    /// *intended* fire time rather than `now()`. Stamped onto
    /// `agentic_runs.metadata.logical_date`.
    #[serde(skip_deserializing, default)]
    pub logical_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Run id this run is a retry of. Set by `retry_run`; stamped onto
    /// `agentic_runs.metadata.retry_of` so the UI can link a retry back
    /// to the run that triggered it.
    #[serde(skip_deserializing, default)]
    pub retry_of: Option<String>,
}

impl StartAutomationRequest {
    /// Validate the client-supplied fields up-front, before any DB writes.
    ///
    /// Run by [`start_automation_run`] on entry. Returning
    /// [`AutomationRunError::InvalidInput`] surfaces a clear 400 to the HTTP
    /// caller instead of a downstream YAML-parse failure (or worse, a
    /// path-traversal read attempt that's only blocked deeper in the stack).
    pub fn validate(&self) -> Result<(), AutomationRunError> {
        validate_automation_ref(&self.workflow_ref)?;
        if let Some(ref steps) = self.invalidate_steps
            && steps.len() > MAX_INVALIDATE_STEPS
        {
            return Err(AutomationRunError::InvalidInput(format!(
                "invalidate_steps has {} entries; max is {MAX_INVALIDATE_STEPS}",
                steps.len()
            )));
        }
        // Cap `invalidate_iterations` by total entry count across all
        // steps so a malformed payload can't ship (e.g.) 10k indices
        // per step. Unknown step names aren't rejected here — the
        // automation body isn't parsed yet; the decider silently no-ops
        // entries that don't match a step, matching `invalidate_steps`.
        if let Some(ref map) = self.invalidate_iterations {
            let total: usize = map.values().map(|v| v.len()).sum();
            if total > MAX_INVALIDATE_ITERATION_ENTRIES {
                return Err(AutomationRunError::InvalidInput(format!(
                    "invalidate_iterations has {total} total entries; max is \
                     {MAX_INVALIDATE_ITERATION_ENTRIES}"
                )));
            }
        }
        // Parse strictly so a typo'd thread_id surfaces here as a 400
        // rather than silently producing an unlinked run that the chat-thread
        // reload path can't recover.
        if let Some(ref tid) = self.thread_id
            && Uuid::parse_str(tid).is_err()
        {
            return Err(AutomationRunError::InvalidInput(format!(
                "thread_id {tid:?} is not a valid UUID"
            )));
        }
        Ok(())
    }
}

fn validate_automation_ref(workflow_ref: &str) -> Result<(), AutomationRunError> {
    if workflow_ref.is_empty() {
        return Err(AutomationRunError::InvalidInput(
            "workflow_ref is empty".into(),
        ));
    }
    let candidate = std::path::Path::new(workflow_ref);
    if candidate.is_absolute() {
        return Err(AutomationRunError::InvalidInput(format!(
            "workflow_ref {workflow_ref:?} must be relative to the workspace"
        )));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(AutomationRunError::InvalidInput(format!(
            "workflow_ref {workflow_ref:?} must not contain `..` segments"
        )));
    }
    Ok(())
}

/// Snapshot of an automation run: the platform-side run record plus the
/// agentic-automation state row, flattened for serialization.
#[derive(Debug, Clone, Serialize)]
pub struct AutomationRunSnapshot {
    pub run_id: String,
    pub workflow_ref: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub answer: Option<String>,
    pub current_step: usize,
    pub total_steps: usize,
    pub results: serde_json::Map<String, Value>,
    pub step_hashes: serde_json::Map<String, Value>,
    pub retry_from_run_id: Option<String>,
    pub cache_enabled: bool,
}

/// One row in the run-history dropdown.
#[derive(Debug, Clone, Serialize)]
pub struct AutomationRunSummary {
    pub run_id: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Errors from [`start_automation_run`] / [`get_automation_snapshot`] /
/// [`run_inline_automation`].
#[derive(Debug, thiserror::Error)]
pub enum AutomationRunError {
    #[error("database: {0}")]
    Db(#[from] DbErr),
    #[error("run not found")]
    NotFound,
    #[error("inline automation: {0}")]
    Inline(String),
    /// Client-supplied [`StartAutomationRequest`] failed validation. HTTP
    /// callers should map this to `400 Bad Request` with the message — the
    /// content is shaped for end-user display.
    #[error("invalid request: {0}")]
    InvalidInput(String),
}

/// Maximum entries allowed in [`StartAutomationRequest::invalidate_steps`].
///
/// Automations realistically have a few dozen steps; capping the list keeps a
/// malicious or malformed client from shipping (e.g.) 10k step names that
/// would be persisted into `agentic_runs.metadata` and re-read at seed time.
/// Picked generously so a legitimate automation can't trip it.
pub const MAX_INVALIDATE_STEPS: usize = 256;

/// Maximum total entries across all steps in [`StartAutomationRequest::invalidate_iterations`].
///
/// Automation authors might fan out a few dozen iterations per loop; a
/// well-formed "retry these failed iterations" call almost always has
/// fewer than 100 total entries. The cap is generous enough to never
/// trip a legitimate caller and tight enough that a malformed payload
/// can't bloat `agentic_workflow_state.invalidate_iterations`.
pub const MAX_INVALIDATE_ITERATION_ENTRIES: usize = 1024;

/// Host-side adapter for executing a single agent invocation
/// synchronously inside the inline automation runner.
///
/// `agentic-pipeline` is layer-agnostic about *what* "running an agent"
/// means; the host (oxy-app) wires this trait to `AgentLauncherExecutable`
/// or whichever agent surface it owns. This mirrors the trait-injection
/// pattern used by [`PlatformContext`](crate::platform::PlatformContext)
/// and `WorkspaceContext` — the runner stays decoupled from a specific
/// agent runtime.
///
/// Used by the inline runner only when an automation contains agent fan-out
/// (`agent_task` with `consistency_run > 1`). When no runner is supplied,
/// the inline path errors out for those workflows.
#[async_trait::async_trait]
pub trait InlineAgentRunner: Send + Sync {
    /// Run an agent and return its final answer text.
    ///
    /// `agent_ref` matches the `agent_ref` field in the automation's
    /// `agent_task` config (typically a path to a `.agent.yml` file).
    async fn run_agent(&self, agent_ref: &str, prompt: &str) -> Result<String, String>;
}

/// Insert a top-level `agentic_runs` row and enqueue a
/// [`TaskSpec::Automation`] for the coordinator to claim.
///
/// Returns the freshly minted `run_id`. The caller is responsible for any
/// runtime-state side effects (registering cancel/answer channels, spawning
/// SSE subscribers); this function only touches the database.
/// `scope` records who will drive the seeded run: [`TaskScope::Scoped`] when
/// a co-located coordinator is spawned right after (every HTTP/CLI caller
/// today), [`TaskScope::Global`] when the Phase 2 scheduler seeds it for the
/// standalone/recovery loop to pick up.
pub async fn start_automation_run(
    db: &DatabaseConnection,
    request: StartAutomationRequest,
    scope: crud::TaskScope,
    workspace_id: Uuid,
) -> Result<String, AutomationRunError> {
    request.validate()?;

    let run_id = Uuid::new_v4().to_string();
    let mut metadata = serde_json::json!({
        "workflow_ref": request.workflow_ref,
        "cache_enabled": request.cache_enabled,
        "retry_from_run_id": request.retry_from_run_id,
        // `null` when not supplied — the executor only acts when this is
        // a non-empty array, so the conventional `Option<Vec<_>>` →
        // `Some([...])` round-trip is preserved through Serde.
        "invalidate_steps": request.invalidate_steps,
        // `null` when not supplied. Executor reads this once at seed and
        // stamps onto `agentic_workflow_state.invalidate_iterations` so
        // the decider applies it inline without re-reading metadata.
        "invalidate_iterations": request.invalidate_iterations,
        // Stamped so `retry_run` can reconstruct a parameterised retry
        // without re-reading the queue spec.
        "variables": request.variables,
    });
    crate::scheduler::stamp_trigger_metadata(
        &mut metadata,
        &request.trigger,
        &request.logical_date,
        &request.retry_of,
    );

    // `validate()` already confirmed any supplied thread_id parses as UUID,
    // so the unwrap here is infallible — but we route through `parse_str`
    // again rather than carrying state across the gap because the strict
    // check stays co-located with the validator.
    let thread_uuid = match request.thread_id.as_deref() {
        Some(s) => Some(
            Uuid::parse_str(s)
                .map_err(|e| AutomationRunError::InvalidInput(format!("thread_id {s:?}: {e}")))?,
        ),
        None => None,
    };

    let question = format!("workflow: {}", request.workflow_ref);
    if let Some(schedule_id) = request.schedule_id.as_deref() {
        crud::insert_run_with_schedule(
            db,
            &run_id,
            &question,
            thread_uuid,
            agentic_automation::SOURCE_TYPE,
            Some(metadata),
            schedule_id,
            workspace_id,
        )
        .await?;
    } else {
        crud::insert_run(
            db,
            &run_id,
            &question,
            thread_uuid,
            agentic_automation::SOURCE_TYPE,
            Some(metadata),
            workspace_id,
        )
        .await?;
    }

    let spec = TaskSpec::Automation {
        workflow_ref: request.workflow_ref,
        variables: request.variables,
        retry_from_run_id: request.retry_from_run_id,
        cache_enabled: request.cache_enabled,
        body: None,
        initial_render_context: None,
    };
    crud::enqueue_task(db, &run_id, &run_id, None, &spec, None, scope).await?;

    Ok(run_id)
}

/// Spawn a coordinator + worker pair that drives a queued automation run
/// to completion.
///
/// Pairs with [`start_automation_run`]. The coordinator manages the task
/// tree (workflow → workflow_decision → optional child step delegations);
/// the worker claims tasks from `agentic_task_queue` and dispatches them
/// through [`PipelineTaskExecutor`]. Both run in detached `tokio::spawn`
/// tasks — they exit when the run reaches a terminal state.
///
/// Modeled on `recovery::recover_single_run`'s drive setup, minus the
/// task-tree replay: a fresh run has nothing to recover, only a single
/// queued root spec waiting to be claimed. The root task is registered
/// with the coordinator *before* spawning — without that step the
/// coordinator silently drops the worker's `Done` outcome (it filters
/// outcomes for unregistered task ids), which manifests as an automation
/// stuck in the `running` state forever.
///
/// `cancel_rx` is the watch receiver registered with `RuntimeState::cancel_txs`.
/// When the HTTP `/cancel` handler flips it, a forwarder task calls
/// `transport.cancel_subtree(run_id)` so the in-flight task tree winds down
/// (queue rows marked cancelled, cancel tokens fired for active workers).
/// Without this wiring, `state.cancel` is a no-op and the Stop button on the
/// run page hangs in its loading state forever.
/// Drive a queued automation run.
///
/// `builder_bridges` is required for automations that delegate to the
/// built-in builder agent via `agent_ref: __builder__` (SQL gen, file
/// edits, etc.) — without it the executor returns "builder bridges
/// not provided" the moment such a step delegates. `None` is only
/// valid for paths that don't have a workspace context (the recovery
/// sweeper is the historical example), and any automation that does
/// invoke the builder from that path will fail loudly.
pub fn spawn_automation_run_drive(
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    run_id: String,
    platform: Arc<dyn PlatformContext>,
    builder_bridges: Option<crate::platform::BuilderBridges>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
) {
    let executor = Arc::new(PipelineTaskExecutor {
        platform,
        builder_bridges,
        schema_cache: None,
        builder_test_runner: None,
        builder_app_runner: None,
        db: db.clone(),
        state: Some(state.clone()),
        custom_executors: None,
    });
    // Scope the transport to this run's task tree. Without scoping, the
    // worker spawned here will happily claim a queued child task that
    // belongs to a sibling run's coordinator — when it does, the wrong
    // coordinator receives the outcome (the task isn't in its in-memory
    // `self.tasks` map), `handle_done` early-returns, and the right
    // coordinator hangs forever waiting for a child that's already done.
    let transport = DurableTransport::with_router(db.clone(), router, Some(run_id.clone()));

    let mut coordinator = Coordinator::new(
        db,
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    )
    .with_completion_policy(Arc::new(agentic_automation::AutomationCompletionPolicy))
    .with_delegation_resolver(Arc::new(agentic_automation::AutomationDelegationResolver));
    // Tell the coordinator about the root task so it routes the worker's
    // outcome (Done with `workflow_continue=true`) into `handle_done` →
    // policy returns Chain → coordinator enqueues the next
    // `AutomationDecision`. `register_root` (vs. `submit_root`) does
    // not re-enqueue; the queue entry already exists from
    // `start_automation_run`'s `crud::enqueue_task` call.
    coordinator.register_root(run_id.clone(), 0);

    let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

    tracing::debug!(
        target: "workflow_run",
        run_id = %run_id,
        "spawning coordinator + worker for queued automation run"
    );

    // Forward `state.cancel(run_id)` into the transport so the worker's
    // in-flight cancel tokens fire and queued descendants are marked
    // cancelled in `agentic_task_queue`. The coordinator's `handle_cancelled`
    // then propagates the cancellation up to the run row.
    let transport_for_cancel = transport.clone();
    let cancel_task_id = run_id.clone();
    let cancel_forwarder = spawn_with_hub(async move {
        while cancel_rx.changed().await.is_ok() {
            if *cancel_rx.borrow() {
                tracing::info!(
                    target: "workflow_run",
                    run_id = %cancel_task_id,
                    "cancel signal received, cancelling task subtree"
                );
                let _ = transport_for_cancel.cancel_subtree(&cancel_task_id).await;
                break;
            }
        }
    });

    let worker_task = spawn_with_hub(async move {
        worker.run().await;
    });
    let cleanup_run_id = run_id.clone();
    let cleanup_state = state;
    spawn_with_hub(async move {
        let mut coord = coordinator;
        coord.run().await;
        // SSE keeps streaming while the notifier exists; without this
        // cleanup the stream stays open after the run terminates and the
        // client never observes a terminal close. Mirror the analytics-side
        // `drive_with_coordinator` shutdown sequence.
        cleanup_state.notify(&cleanup_run_id);
        cancel_forwarder.abort();
        worker_task.abort();
        cleanup_state.deregister(&cleanup_run_id);
    });
}

/// List recent runs for a given `workflow_ref` in `workspace_id`, newest first.
///
/// Filters `agentic_runs` on `source_type = 'workflow'`, the `workspace_id`
/// and `metadata->>'workflow_ref' = $workflow_ref`. The dropdown UI calls this
/// to populate its run-history list.
///
/// `workspace_id` is not optional. A ref is a workspace-relative path
/// (`automations/daily.automation.yml`), so the same ref names a different
/// automation in every workspace that has one; filtering on the ref alone
/// handed each workspace every other workspace's run ids for that path. Pass
/// the id `start_automation_run` stamps — `PlatformContext::workspace_id()`,
/// the nil UUID in local mode.
pub async fn list_automation_runs(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    workflow_ref: &str,
    limit: u64,
) -> Result<Vec<AutomationRunSummary>, AutomationRunError> {
    use sea_orm::{FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        task_status: Option<String>,
        created_at: chrono::DateTime<chrono::FixedOffset>,
        updated_at: chrono::DateTime<chrono::FixedOffset>,
    }

    // JSONB `->>` returns text. The `metadata` column is `Option<Json>`,
    // so we coalesce the missing-row case to NULL and filter that out.
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"
        SELECT id, task_status, created_at, updated_at
        FROM agentic_runs
        WHERE source_type = $1
          AND parent_run_id IS NULL
          AND workspace_id = $2
          AND metadata->>'workflow_ref' = $3
        ORDER BY created_at DESC
        LIMIT $4
        "#,
        [
            agentic_automation::SOURCE_TYPE.into(),
            workspace_id.into(),
            workflow_ref.into(),
            (limit as i64).into(),
        ],
    );

    let rows = Row::find_by_statement(stmt).all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| AutomationRunSummary {
            run_id: r.id,
            status: r.task_status.unwrap_or_else(|| "unknown".into()),
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Fetch a [`AutomationRunSnapshot`] for the given run id.
///
/// Returns [`AutomationRunError::NotFound`] when the platform-side run record
/// is missing entirely. The agentic-automation extension state row may legally
/// be absent before the worker first claims the queued task; in that case
/// the snapshot reflects the run's queued status with an empty step list.
pub async fn get_automation_snapshot(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<AutomationRunSnapshot, AutomationRunError> {
    let run = crud::get_run(db, run_id)
        .await?
        .ok_or(AutomationRunError::NotFound)?;

    let workflow_ref = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("workflow_ref"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let wf_state = agentic_automation::extension::load_automation_state(db, run_id).await?;

    Ok(match wf_state {
        Some(s) => AutomationRunSnapshot {
            run_id: run_id.to_string(),
            workflow_ref,
            status: run
                .task_status
                .clone()
                .unwrap_or_else(|| "queued".to_string()),
            error_message: run.error_message.clone(),
            answer: run.answer.clone(),
            current_step: s.current_step,
            total_steps: s.workflow.tasks.len(),
            results: s.results.into_iter().collect(),
            step_hashes: s
                .step_hashes
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
            retry_from_run_id: s.retry_from_run_id,
            cache_enabled: s.cache_enabled,
        },
        None => AutomationRunSnapshot {
            run_id: run_id.to_string(),
            workflow_ref,
            status: run
                .task_status
                .clone()
                .unwrap_or_else(|| "queued".to_string()),
            error_message: run.error_message,
            answer: run.answer,
            current_step: 0,
            total_steps: 0,
            results: Default::default(),
            step_hashes: Default::default(),
            retry_from_run_id: None,
            cache_enabled: false,
        },
    })
}

// ── Inline (in-process) execution ──────────────────────────────────────────

/// Run an automation synchronously, in-process, without involving the
/// coordinator / worker queue or any DB row.
///
/// Intended for callers that already have a typed [`AutomationConfig`]
/// in memory and need a final result map back **right now** — most
/// notably the Data Apps server which renders dashboards on each
/// request and cannot wait on a queued run.
///
/// Drives [`AutomationDecider`] in a loop:
/// - inline decisions (formatter, conditional, cache-hit) advance through
///   the decider directly,
/// - delegated decisions (sql / semantic / omni / looker) execute via
///   [`run_automation_step`] and the result is folded back as a child
///   completion,
/// - parallel / fan-out decisions are not yet supported and return
///   [`AutomationRunError::Inline`].
///
/// Returns the accumulated step results: `step_name → result JSON`.
pub async fn run_inline_automation(
    workspace: &dyn WorkspaceContext,
    automation: AutomationConfig,
    variables: Option<Value>,
) -> Result<HashMap<String, Value>, AutomationRunError> {
    run_inline_automation_with(workspace, automation, variables, None).await
}

/// Variant of [`run_inline_automation`] that accepts an [`InlineAgentRunner`]
/// so consistency-run / agent-fanout workflows can complete in-process.
///
/// Pass `None` (or use [`run_inline_automation`]) when the automation does
/// not contain agent fan-outs; agent steps without an injected runner
/// will surface a clear error instead of silently no-oping.
pub async fn run_inline_automation_with(
    workspace: &dyn WorkspaceContext,
    automation: AutomationConfig,
    variables: Option<Value>,
    agent_runner: Option<&dyn InlineAgentRunner>,
) -> Result<HashMap<String, Value>, AutomationRunError> {
    run_inline_automation_internal(workspace, automation, variables, agent_runner, None).await
}

/// Variant of [`run_inline_automation_with`] that ALSO seeds
/// `render_context` so Jinja in task SQL can read caller-supplied
/// keys (`{{ controls.store }}`, etc.).
///
/// `variables` lands in `state.variables` (treated as automation-level
/// metadata, not visible to Jinja in the inline path) while
/// `render_context` is what `merge_sql_variables` and
/// `render_jinja_string` actually read. Data Apps' `controls` need to
/// land in the latter; pass them through here.
pub async fn run_inline_automation_with_render_context(
    workspace: &dyn WorkspaceContext,
    automation: AutomationConfig,
    variables: Option<Value>,
    render_context: Option<Value>,
    agent_runner: Option<&dyn InlineAgentRunner>,
) -> Result<HashMap<String, Value>, AutomationRunError> {
    run_inline_automation_internal(
        workspace,
        automation,
        variables,
        agent_runner,
        render_context,
    )
    .await
}

/// Internal entry point that lets a loop iteration seed its render context
/// from the parent's accumulated state (`schedules.value`, etc.). Public
/// callers go through [`run_inline_automation`] / [`run_inline_automation_with`]
/// which start with an empty context — only the recursive loop path needs
/// to thread inherited context.
async fn run_inline_automation_internal(
    workspace: &dyn WorkspaceContext,
    automation: AutomationConfig,
    variables: Option<Value>,
    agent_runner: Option<&dyn InlineAgentRunner>,
    initial_render_context: Option<Value>,
) -> Result<HashMap<String, Value>, AutomationRunError> {
    let workflow_yaml_hash = agentic_automation::hash::canonical_hash(&automation)
        .map_err(|e| AutomationRunError::Inline(format!("hash automation config: {e}")))?;
    let workflow_context = serde_json::json!({
        "workspace_path": workspace.workspace_path().map(|p| p.to_string_lossy().to_string()),
    });
    // Fold effective variables (automation `variables:` declarations +
    // runtime overrides) into the seed render context so templates can
    // reference them by name (`{{ metric_label }}`). This mirrors the
    // queue path's seed step in `executor::automation::execute_automation`;
    // without it, standalone CLI / Data App runs leave declared
    // `{default: X}` variables invisible to render-context lookups and
    // SQL like `ROUND({{ aggregation_sql }}, 2)` renders as `ROUND(, 2)`.
    let mut initial_render_context =
        initial_render_context.unwrap_or_else(|| serde_json::json!({}));
    let declared = automation
        .variables
        .as_ref()
        .and_then(|m| serde_json::to_value(m).ok());
    let effective =
        agentic_automation::variables::effective_variables(declared.as_ref(), variables.as_ref());
    if let (Some(ctx_obj), Some(vars_obj)) = (
        initial_render_context.as_object_mut(),
        effective.as_object(),
    ) {
        for (k, v) in vars_obj {
            ctx_obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut state = AutomationRunState {
        run_id: format!("inline-{}", Uuid::new_v4()),
        workflow: automation,
        workflow_yaml_hash,
        workflow_context: workflow_context.clone(),
        variables,
        trace_id: format!("inline-{}", Uuid::new_v4()),
        current_step: 0,
        results: HashMap::new(),
        render_context: initial_render_context.clone(),
        pending_children: HashMap::new(),
        decision_version: 0,
        step_hashes: HashMap::new(),
        retry_from_run_id: None,
        cache_enabled: false,
        prior_step_hashes: HashMap::new(),
        prior_results: HashMap::new(),
        initial_render_context,
        invalidate_iterations: HashMap::new(),
    };

    // No airway admission resolver here, deliberately: this is the inline
    // (Data App) runner and `run_delegated_step` rejects `TaskSpec::Airway`
    // outright, so no airway run is ever enqueued down this path and there is
    // no admission to resolve. The queue-driven site
    // (`executor::automation::execute_automation_decision`) injects one.
    let decider = AutomationDecider::new(None);
    let mut pending_child: Option<ChildCompletion> = None;
    // Hard cap on iterations matches `workflow_cache_resume_test::drive_to_complete`
    // — long enough for any realistic Data App, short enough to surface a
    // runaway decider as a failure rather than a hang.
    for _ in 0..1024 {
        let (new_state, decision) = decider
            .decide(state, pending_child.take(), None, None)
            .await;
        state = new_state;
        match decision {
            AutomationDecision::StepExecutedInline { .. } => continue,
            AutomationDecision::Complete { .. } => return Ok(state.results),
            AutomationDecision::Fail { error, .. } => {
                return Err(AutomationRunError::Inline(error));
            }
            AutomationDecision::WaitForMoreChildren => {
                return Err(AutomationRunError::Inline(
                    "WaitForMoreChildren in inline path — should not happen".into(),
                ));
            }
            AutomationDecision::DelegateStep {
                step_index,
                step_name,
                spec,
                ..
            } => {
                let answer = run_delegated_step(workspace, &state, &spec, agent_runner).await?;
                pending_child = Some(ChildCompletion {
                    child_task_id: format!("inline-step-{step_index}"),
                    step_index,
                    step_name,
                    status: "done".into(),
                    answer,
                });
            }
            AutomationDecision::DelegateParallel {
                step_index,
                step_name,
                items,
                ..
            } => {
                // Inline loops + consistency fan-outs: run each child
                // sequentially, then synthesize a single ChildCompletion
                // that mirrors the queue path's aggregated answer
                // (`{ child_id: { status, answer } }`). Concurrency hints
                // are ignored — Data Apps and CLI value correctness over
                // throughput, and a sequential drive sidesteps the
                // need for a Tokio runtime contract here.
                let aggregated =
                    Box::pin(run_parallel_inline(workspace, &items, agent_runner)).await?;
                pending_child = Some(ChildCompletion {
                    child_task_id: format!("inline-parallel-{step_index}"),
                    step_index,
                    step_name,
                    status: "done".into(),
                    answer: aggregated,
                });
            }
        }
    }
    Err(AutomationRunError::Inline(
        "exceeded 1024 decisions without completing".into(),
    ))
}

/// Run every parallel item in order and build the same aggregated answer
/// shape the queue-side `aggregate_child_results` produces.
///
/// **Loops** (`DelegationTarget::Automation { workflow_ref = "__workflow_step__" }`)
/// receive a context object with the iteration's `step_config` (a
/// `{name, tasks}` body), `render_context`, `workflow_context`,
/// `loop_item`, and `loop_index`. Each iteration runs as a recursive
/// inline automation.
///
/// **Consistency runs** (`DelegationTarget::Agent`) are run via the
/// caller-supplied [`InlineAgentRunner`]. Without a runner we surface a
/// clear error rather than silently no-op the fan-out.
async fn run_parallel_inline(
    workspace: &dyn WorkspaceContext,
    items: &[DelegationItem],
    agent_runner: Option<&dyn InlineAgentRunner>,
) -> Result<String, AutomationRunError> {
    let mut aggregated = serde_json::Map::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let entry = match &item.target {
            DelegationTarget::Automation { workflow_ref }
                if workflow_ref == "__workflow_step__" =>
            {
                run_loop_iteration_inline(workspace, &item.context, agent_runner).await?
            }
            DelegationTarget::Automation { workflow_ref } => {
                return Err(AutomationRunError::Inline(format!(
                    "inline parallel: sub-automation target '{workflow_ref}' is unsupported"
                )));
            }
            DelegationTarget::Agent { agent_id } => {
                let runner = agent_runner.ok_or_else(|| {
                    AutomationRunError::Inline(
                        "inline parallel: consistency runs (agent fan-out) require an \
                         InlineAgentRunner; pass one via run_inline_automation_with"
                            .into(),
                    )
                })?;
                let answer = runner
                    .run_agent(agent_id, &item.request)
                    .await
                    .map_err(AutomationRunError::Inline)?;
                serde_json::json!({ "status": "done", "answer": answer, "index": idx })
            }
        };
        // Inject `index` so the decider's fold-time `iterations` builder
        // can map every aggregated entry back to the original loop item
        // position (and from there to the `value` for cache attribution).
        // Inner branches already attach `index` to their entries; if a
        // branch produced something without one, we backfill here.
        let entry = match entry {
            Value::Object(mut map) => {
                map.entry("index".to_string())
                    .or_insert_with(|| Value::from(idx));
                Value::Object(map)
            }
            other => other,
        };
        // Match the queue path's aggregation key: stable `inline-N` ids in
        // iteration order. Downstream consumers treat this as opaque, so
        // any unique id works as long as it's deterministic.
        aggregated.insert(format!("inline-{idx}"), entry);
    }
    serde_json::to_string(&aggregated)
        .map_err(|e| AutomationRunError::Inline(format!("aggregate: {e}")))
}

async fn run_loop_iteration_inline(
    workspace: &dyn WorkspaceContext,
    context: &Value,
    agent_runner: Option<&dyn InlineAgentRunner>,
) -> Result<Value, AutomationRunError> {
    // The decider builds this shape — see `step_decider::Loop` arm.
    let step_config = context
        .get("step_config")
        .ok_or_else(|| AutomationRunError::Inline("loop item: missing step_config".into()))?;
    // step_config is `{name, tasks}` — a sub-automation body. Round-trip via
    // JSON onto the typed config; `AutomationConfig` ignores unknown fields,
    // so the synthetic `name` field doesn't trip parsing.
    let sub_automation: AutomationConfig = serde_json::from_value(step_config.clone())
        .map_err(|e| AutomationRunError::Inline(format!("loop item: parse sub-automation: {e}")))?;

    // Thread the per-iteration render context the decider built (parent's
    // results + `loop_step.value`/`loop_step.index` for this item). Without
    // this, the inner sub-automation starts with an empty context and any
    // `{{ outer.value }}` / `{{ inner.value }}` reference in a nested loop
    // renders as undefined.
    //
    // Pass `agent_runner` through too: nested loops with agent steps
    // (e.g. an inner `metrics` loop whose tasks include `type: agent`)
    // would otherwise hit "requires an InlineAgentRunner" on the second
    // level of recursion.
    let inherited_context = context.get("render_context").cloned();
    let sub_results = Box::pin(run_inline_automation_internal(
        workspace,
        sub_automation,
        None,
        agent_runner,
        inherited_context,
    ))
    .await?;
    let answer = serde_json::to_string(&sub_results)
        .map_err(|e| AutomationRunError::Inline(format!("loop item: serialize answer: {e}")))?;
    Ok(serde_json::json!({ "status": "done", "answer": answer }))
}

async fn run_delegated_step(
    workspace: &dyn WorkspaceContext,
    state: &AutomationRunState,
    spec: &TaskSpec,
    agent_runner: Option<&dyn InlineAgentRunner>,
) -> Result<String, AutomationRunError> {
    match spec {
        TaskSpec::AutomationStep {
            step_config,
            render_context,
            workflow_context,
        } => run_automation_step(
            workspace,
            step_config.clone(),
            render_context.clone(),
            workflow_context.clone(),
        )
        .await
        .map_err(AutomationRunError::Inline),
        // A standalone agent step (no consistency fan-out) lands here as
        // TaskSpec::Agent. The inline runner already accepts an
        // `InlineAgentRunner` for the fan-out path; reuse it here so
        // workflows with a single agent step don't need to be wrapped in
        // a consistency_run > 1 to execute inline.
        TaskSpec::Agent {
            agent_id, question, ..
        } => {
            let runner = agent_runner.ok_or_else(|| {
                AutomationRunError::Inline(format!(
                    "agent step '{agent_id}' requires an InlineAgentRunner; pass one via \
                     run_inline_automation_with"
                ))
            })?;
            runner
                .run_agent(agent_id, question)
                .await
                .map_err(AutomationRunError::Inline)
        }
        // Sub-automation step (`type: workflow`). The queue path delegates
        // this to a child coordinator run; inline we recurse directly.
        // The recursive call folds the sub-automation's own `variables:`
        // defaults plus the parent's override block (`variables`) into
        // its render context, then returns its `step → result` map. We
        // serialize that map to a JSON string because the decider folds
        // a child answer back via `serde_json::from_str` — so the parent
        // template `{{ child_step.report.text }}` resolves identically
        // to the queue path.
        TaskSpec::Automation {
            workflow_ref,
            variables,
            body,
            initial_render_context,
            ..
        } => {
            let sub_automation: AutomationConfig = if let Some(body) = body {
                serde_json::from_value(body.clone()).map_err(|e| {
                    AutomationRunError::Inline(format!("parse inline sub-automation body: {e}"))
                })?
            } else {
                let yaml = workspace
                    .resolve_automation_yaml(workflow_ref)
                    .await
                    .map_err(|e| {
                        AutomationRunError::Inline(format!(
                            "load sub-automation '{workflow_ref}': {e}"
                        ))
                    })?;
                serde_yaml::from_str(&yaml).map_err(|e| {
                    AutomationRunError::Inline(format!(
                        "parse sub-automation '{workflow_ref}': {e}"
                    ))
                })?
            };
            let sub_results = Box::pin(run_inline_automation_internal(
                workspace,
                sub_automation,
                variables.clone(),
                agent_runner,
                initial_render_context.clone(),
            ))
            .await?;
            serde_json::to_string(&sub_results).map_err(|e| {
                AutomationRunError::Inline(format!("serialize sub-automation results: {e}"))
            })
        }
        TaskSpec::Resume { .. }
        | TaskSpec::AutomationDecision { .. }
        | TaskSpec::Airway { .. }
        | TaskSpec::Compile { .. }
        | TaskSpec::Custom { .. } => Err(AutomationRunError::Inline(format!(
            "TaskSpec::{spec:?} cannot run inline; only AutomationStep, Agent and Automation are supported"
        ))),
    }
    .inspect_err(|_e| {
        // Surface a hint about which step failed rather than a bare error
        // string — Data Apps log this directly to the user.
        let _ = state;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request(workflow_ref: &str) -> StartAutomationRequest {
        StartAutomationRequest {
            workflow_ref: workflow_ref.to_string(),
            variables: None,
            retry_from_run_id: None,
            cache_enabled: false,
            invalidate_steps: None,
            invalidate_iterations: None,
            thread_id: None,
            schedule_id: None,
            trigger: None,
            logical_date: None,
            retry_of: None,
        }
    }

    fn assert_invalid(req: &StartAutomationRequest, needle: &str) {
        match req.validate() {
            Err(AutomationRunError::InvalidInput(msg)) => assert!(
                msg.contains(needle),
                "error {msg:?} did not contain {needle:?}"
            ),
            other => panic!("expected InvalidInput containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_automation_ref() {
        assert_invalid(&base_request(""), "workflow_ref is empty");
    }

    #[test]
    fn rejects_absolute_automation_ref() {
        assert_invalid(&base_request("/etc/passwd"), "must be relative");
    }

    #[test]
    fn rejects_parent_dir_automation_ref() {
        for r in [
            "../etc/passwd",
            "workflows/../../../etc/passwd",
            "..",
            "a/../b",
        ] {
            assert_invalid(&base_request(r), "`..`");
        }
    }

    #[test]
    fn accepts_relative_automation_ref() {
        for r in [
            "foo.automation.yml",
            "workflows/foo.automation.yml",
            "./foo.automation.yml",
        ] {
            base_request(r).validate().expect("should accept");
        }
    }

    #[test]
    fn rejects_oversized_invalidate_steps() {
        let mut req = base_request("foo.automation.yml");
        req.invalidate_steps = Some(
            (0..MAX_INVALIDATE_STEPS + 1)
                .map(|i| format!("step_{i}"))
                .collect(),
        );
        assert_invalid(&req, "max is");
    }

    #[test]
    fn accepts_invalidate_steps_at_cap() {
        let mut req = base_request("foo.automation.yml");
        req.invalidate_steps = Some(
            (0..MAX_INVALIDATE_STEPS)
                .map(|i| format!("step_{i}"))
                .collect(),
        );
        req.validate().expect("should accept at cap");
    }

    #[test]
    fn rejects_malformed_thread_id() {
        let mut req = base_request("foo.automation.yml");
        req.thread_id = Some("not-a-uuid".into());
        assert_invalid(&req, "valid UUID");
    }

    #[test]
    fn accepts_valid_thread_id() {
        let mut req = base_request("foo.automation.yml");
        req.thread_id = Some(Uuid::new_v4().to_string());
        req.validate().expect("should accept");
    }

    #[test]
    fn rejects_oversized_invalidate_iterations() {
        let mut req = base_request("foo.automation.yml");
        // One step with MAX+1 indices trips the total-entries cap.
        req.invalidate_iterations = Some(HashMap::from([(
            "step_a".into(),
            (0..MAX_INVALIDATE_ITERATION_ENTRIES + 1).collect(),
        )]));
        assert_invalid(&req, "total entries");
    }

    #[test]
    fn rejects_invalidate_iterations_spread_over_cap() {
        let mut req = base_request("foo.automation.yml");
        // Total across two steps trips the cap even if each step's
        // list is small enough on its own.
        let half = MAX_INVALIDATE_ITERATION_ENTRIES / 2 + 1;
        req.invalidate_iterations = Some(HashMap::from([
            ("step_a".into(), (0..half).collect()),
            ("step_b".into(), (0..half).collect()),
        ]));
        assert_invalid(&req, "total entries");
    }

    #[test]
    fn accepts_invalidate_iterations_at_cap() {
        let mut req = base_request("foo.automation.yml");
        req.invalidate_iterations = Some(HashMap::from([(
            "step_a".into(),
            (0..MAX_INVALIDATE_ITERATION_ENTRIES).collect(),
        )]));
        req.validate().expect("should accept at cap");
    }
}
