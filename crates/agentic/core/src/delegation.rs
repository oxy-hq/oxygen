//! Types for cross-agent delegation and task coordination.
//!
//! These types are used by the coordinator to manage a tree of tasks where
//! agents and automations can delegate work to each other via the suspend/resume
//! mechanism.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::events::HumanInputQuestion;
use crate::human_input::SuspendedRunData;

#[inline]
fn is_false(b: &bool) -> bool {
    !*b
}

// ── SuspendReason ────────────────────────────────────────────────────────────

/// Why a pipeline suspended.
///
/// Carried by [`BackTarget::Suspend`] and [`PipelineOutcome::Suspended`] to
/// tell the coordinator how to fulfil the suspension: either present questions
/// to a human or spawn a child task.
///
/// [`BackTarget::Suspend`]: crate::back_target::BackTarget::Suspend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SuspendReason {
    /// The LLM invoked `ask_user` — a human must answer.
    HumanInput { questions: Vec<HumanInputQuestion> },
    /// The solver requested delegation to another agent or automation.
    Delegation {
        target: DelegationTarget,
        /// The question/instruction for the delegate.
        request: String,
        /// Opaque context the coordinator may forward to the child task.
        context: Value,
        /// Optional retry/fallback policy for the delegated task.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        policy: Option<TaskPolicy>,
    },
    /// The solver requested parallel delegation to multiple targets.
    ParallelDelegation {
        targets: Vec<DelegationItem>,
        /// How to handle partial failures.
        failure_policy: FanoutFailurePolicy,
    },
}

// ── Parallel delegation types ───────────────────────────────────────────────

/// A single delegation target within a parallel fan-out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationItem {
    pub target: DelegationTarget,
    /// The question/instruction for the delegate.
    pub request: String,
    /// Opaque context forwarded to the child task.
    #[serde(default)]
    pub context: Value,
}

/// How the coordinator handles partial failures in a parallel delegation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FanoutFailurePolicy {
    /// Fail the parent immediately when any child fails; cancel remaining siblings.
    #[default]
    FailFast,
    /// Wait for all children to complete; parent receives partial results.
    BestEffort,
}

// ── Task policies ───────────────────────────────────────────────────────────

/// Retry and fallback configuration for a delegated task.
///
/// Attached to [`TaskAssignment`] and enforced transparently by the coordinator.
/// Domain crates opt in by populating this field; `None` means no retry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskPolicy {
    /// Retry the same target on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    /// Fallback targets to try if the primary (and all retries) fail.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_targets: Vec<DelegationTarget>,
}

/// How to retry a failed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Backoff between retries.
    pub backoff: BackoffStrategy,
    /// Only retry on failures matching these patterns (empty = retry all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_on: Vec<String>,
}

/// Backoff strategy between retry attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackoffStrategy {
    /// Fixed delay between retries.
    Fixed { delay_ms: u64 },
    /// Exponential backoff: `initial_delay_ms * 2^attempt`, capped at `max_delay_ms`.
    Exponential {
        initial_delay_ms: u64,
        max_delay_ms: u64,
    },
}

impl BackoffStrategy {
    /// Compute the delay for the given attempt (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        match self {
            BackoffStrategy::Fixed { delay_ms } => Duration::from_millis(*delay_ms),
            BackoffStrategy::Exponential {
                initial_delay_ms,
                max_delay_ms,
            } => {
                let delay = initial_delay_ms.saturating_mul(2u64.saturating_pow(attempt));
                Duration::from_millis(delay.min(*max_delay_ms))
            }
        }
    }
}

// ── DelegationTarget ─────────────────────────────────────────────────────────

/// What to delegate to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DelegationTarget {
    /// Another agentic agent (analytics, builder, etc.).
    Agent { agent_id: String },
    /// An automation file.
    // Wire tag stays `workflow` (persisted in agentic_task_queue); only the
    // Rust variant name is the canonical Automation term.
    #[serde(rename = "workflow")]
    Automation { workflow_ref: String },
}

// ── TaskSpec ─────────────────────────────────────────────────────────────────

/// Describes a unit of work the coordinator assigns to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskSpec {
    /// Start a fresh agent run.
    Agent {
        agent_id: String,
        question: String,
        /// Opaque domain-specific extra params. The runtime carries
        /// this through to the executor without inspection; the
        /// executor deserializes into whatever shape it expects.
        ///
        /// Used by `agentic-automation` to pass the analytics agent's
        /// `output_mode` ("answer" | "sql") into the analytics
        /// pipeline. Kept as `serde_json::Value` so `agentic-core`
        /// doesn't need to know about domain types.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },
    /// Execute an automation.
    // Wire tag stays `workflow` (persisted task-queue contract).
    #[serde(rename = "workflow")]
    Automation {
        workflow_ref: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        variables: Option<Value>,
        /// Prior run id whose step results may be reused on a "resume only
        /// unchanged steps" retry. Present only when the caller explicitly
        /// requested a retry; absent for fresh runs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_from_run_id: Option<String>,
        /// Caller opt-in for hash-based step skipping. The decider only
        /// consults `retry_from_run_id` when this is `true`.
        #[serde(default, skip_serializing_if = "is_false")]
        cache_enabled: bool,
        /// Inline `AutomationConfig` body. When `Some`, the executor uses
        /// this instead of resolving `workflow_ref` off disk. Set by the
        /// coordinator when a `loop_sequential` iteration is fanned out
        /// — each iteration's `{name, tasks}` body becomes a synthetic
        /// sub-automation run so multi-task iteration bodies (including
        /// ones with agent steps) can dispatch through the normal queue.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        /// Initial render context to seed onto the sub-automation's state.
        /// Used in tandem with `body` so a loop iteration's parent results
        /// + the iteration variable (`{step_name}.value` / `.index`) are
        /// visible to inner template references like `{{ schedules.value }}`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_render_context: Option<Value>,
    },
    /// Resume a suspended run with an answer.
    Resume {
        run_id: String,
        resume_data: SuspendedRunData,
        answer: String,
    },
    /// Execute a single automation step (SQL, semantic query, etc.).
    ///
    /// The step worker deserializes the config, builds a renderer from the
    /// render context, executes the step, and returns the `OutputContainer`
    /// as the answer string.
    // Wire tag stays `workflow_step` (persisted task-queue contract).
    #[serde(rename = "workflow_step")]
    AutomationStep {
        /// Serialized step config (the Task YAML parsed into JSON).
        step_config: Value,
        /// Accumulated render context from prior steps (`{{ step_name.field }}`).
        render_context: Value,
        /// Automation-level config (workspace path, database configs, globals).
        workflow_context: Value,
    },
    /// Stateless "decision task" for an automation (Temporal-inspired).
    ///
    /// The worker loads the automation state snapshot (from the automation domain's
    /// extension table), folds any `pending_child_answer` into the state, runs
    /// the pure `AutomationDecider::decide()` function to compute the next action,
    /// persists the new state, and exits. No in-memory channels span decision
    /// task boundaries — everything is in the DB.
    // Wire tag stays `workflow_decision` (persisted task-queue contract).
    #[serde(rename = "workflow_decision")]
    AutomationDecision {
        /// The automation run_id (also the PK of the automation-state table).
        run_id: String,
        /// Latest child completion to fold into state before deciding.
        /// `None` on the initial decision (automation just started) and on
        /// inline-chain decisions (an inline step produced an output but the
        /// automation isn't done — chain into the next decision).
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_child_answer: Option<ChildCompletion>,
    },
    /// Generic escape hatch for background job types (e.g. `"preagg_cycle"`).
    /// The `kind` field becomes the run's `source_type` in the DB.
    Custom {
        kind: String,
        payload: serde_json::Value,
    },
    /// Run an airway ELT pipeline end-to-end (extract → normalize → load).
    ///
    /// Unlike [`TaskSpec::Automation`], airway runs are atomic from the queue's
    /// perspective — no per-step decisions, no fan-out at the coordinator.
    /// The fan-out across resources happens inside the airway engine itself
    /// (see `airway::extract_parallel`).
    Airway {
        /// Path or identifier for the pipeline spec. The pipeline crate loads
        /// + parses this into an `AirwayPipelineSpec`.
        pipeline_ref: String,
        /// Variables to render into the YAML at run time. Same shape as
        /// [`TaskSpec::Automation::variables`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variables: Option<Value>,
        /// Explicit subset of resources (tables) to run, overriding the
        /// spec's `resources`. Used by "retry failed tables" to re-run only
        /// the streams that failed. Empty = run the whole spec. Old queued
        /// rows without this key deserialize to empty (backward-compatible).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resources: Vec<String>,
        /// Bounded-backfill window `[from, to)` as RFC3339 strings, applied
        /// to the date-windowed sources (toast, quickbooks, sp_api). Set only
        /// by the backfill path; absent for normal runs. Carried as strings so
        /// the runtime queue stays chrono-free; the source factory parses them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backfill_from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backfill_to: Option<String>,
        /// Contract policy this run is admitted under (`permissive` |
        /// `require_declared` | `forbid_opaque`). Carried as a string because
        /// `airway::connector::ContractPolicy` implements `FromStr` but not
        /// `Serialize`; `agentic_airway::AirwayAdmission` parses it. Absent =
        /// the airway default (`permissive`), so rows queued before this key
        /// existed keep their behaviour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contract_policy: Option<String>,
        /// Vendor environment (`production` | `sandbox`). Same carrying
        /// rationale and same absent-means-default rule as `contract_policy`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<String>,
    },
    /// Walk a workspace and write a compile-boundary revision (rows in
    /// `revisions` + per-entity tables; optionally promotes
    /// `workspaces.current_revision_id`). Driven by `oxy-compile`.
    ///
    /// Atomic from the queue's perspective: one TaskSpec, one revision.
    /// Webhook-triggered compiles set `promote = true`; a dry-run
    /// compile leaves it `false`, writing rows without moving what
    /// the runtime reads.
    Compile {
        /// Workspace UUID whose source to compile.
        workspace_id: Uuid,
        /// Git SHA recorded on the revision. When `None`, the worker
        /// records the literal "local" — useful for working-copy
        /// invocations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_sha: Option<String>,
        /// Optional branch name (e.g. `main`, `feature/x`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// When true AND the compile succeeds AND `kind == "main"`,
        /// atomically updates `workspaces.current_revision_id` inside
        /// the finalisation transaction.
        #[serde(default, skip_serializing_if = "is_false")]
        promote: bool,
        /// `main` (default) | `draft`. Drafts are scoped to a single
        /// `owner_user_id` and never promote `current_revision_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Required when `kind == "draft"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_user_id: Option<Uuid>,
        /// When true, `git_sha` names a real commit and the worker fetches
        /// it from the workspace's remote instead of reading a working copy,
        /// so a pod with no checkout can run the task. Its run is stamped
        /// `compile_git` rather than `compile`, which is what selection keys
        /// on. Absent = false, so a row queued before this key existed is a
        /// working-copy compile, as it always was.
        #[serde(default, skip_serializing_if = "is_false")]
        from_git: bool,
    },
}

/// The two admission strings for one airway run, as stored in
/// `airway_source_config` and as carried on [`TaskSpec::Airway`].
///
/// Lives in core — beside the `TaskSpec::Airway` fields it fills — because
/// every layer that *builds* an airway spec needs to name the shape: the
/// facade (`agentic-pipeline`, which owns the only merge implementation and
/// the `entity` dependency it needs) and the automation domain, which reaches
/// that implementation through the
/// `agentic_automation::AirwayAdmissionResolver` port. Core carries the type,
/// not the query, so no `sea-orm`/`entity` dependency follows it down here.
///
/// `None` means "unset", which `agentic_airway::AirwayAdmission::from_strings`
/// reads as airway's own default — `permissive` / `production`.
///
/// Deliberately **not** parsed: the strings ride the durable queue payload and
/// are parsed at the worker, so a value that was valid at enqueue and invalid
/// after a downgrade fails the run loudly rather than at submit time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAdmission {
    pub contract_policy: Option<String>,
    pub environment: Option<String>,
}

/// A completed child task's outcome, packaged for folding into an automation
/// decision's input state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildCompletion {
    /// The child task_id (e.g. `"<run_id>.3"`).
    pub child_task_id: String,
    /// Which automation step this child was spawned for.
    pub step_index: usize,
    /// The step's name from the automation config.
    pub step_name: String,
    /// `"done"` | `"failed"` | `"cancelled"` | `"timed_out"`.
    pub status: String,
    /// The child's answer (for done) or error message (for failed).
    pub answer: String,
}

// ── TaskAssignment ───────────────────────────────────────────────────────────

/// A task assigned by the coordinator to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task_id: String,
    /// If this is a child task, the parent's task_id.
    pub parent_task_id: Option<String>,
    /// The run_id to use for DB persistence (coordinator assigns).
    pub run_id: String,
    /// What to do.
    pub spec: TaskSpec,
    /// Optional retry/fallback policy enforced by the coordinator.
    pub policy: Option<TaskPolicy>,
}

// ── TaskOutcome ──────────────────────────────────────────────────────────────

/// Outcome of a task, reported by the worker back to the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskOutcome {
    /// Task completed with an answer.
    Done {
        answer: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    /// Task suspended — coordinator decides how to fulfil it.
    Suspended {
        reason: SuspendReason,
        resume_data: SuspendedRunData,
        trace_id: String,
    },
    /// Task failed.
    Failed(String),
    /// Task was cancelled.
    Cancelled,
    /// The task could not run **yet** and wants to be retried later.
    ///
    /// Not a failure and not an attempt: the executor looked at the task,
    /// found a precondition unmet (a contended single-flight lease, a
    /// rate limit), and is handing it back. The worker translates this into
    /// [`crate::transport::WorkerMessage::Defer`] rather than an `Outcome`,
    /// so it is never recorded as a result — the coordinator's view is that
    /// nothing happened, which is the truth.
    Deferred {
        /// How long to withhold the task before it may be claimed again.
        delay_secs: u64,
        /// Total wall-clock the task may spend waiting across consecutive
        /// deferrals before it is dead-lettered. The DOMAIN sets this: only it
        /// knows how long its work can legitimately be blocked. The queue
        /// enforces it but has no opinion on the value.
        max_wait_secs: u64,
        /// Operator-facing reason, for logs.
        reason: String,
    },
}
