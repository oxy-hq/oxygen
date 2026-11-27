//! [`StartAirwayRequest`]: the inputs to start an airway run.
//!
//! A plain serde struct, defined here rather than beside
//! `agentic_pipeline::airway_run::start_airway_run` (which re-exports it at
//! its old path) so the platform's function seam can name it without the
//! facade (`internal-docs/crate-tiers.md`, stage B).

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// Inputs for `agentic_pipeline::airway_run::start_airway_run`. Doubles as
/// the HTTP request body for `POST /agentic-airway/runs`.
#[derive(Debug, Clone, Deserialize)]
pub struct StartAirwayRequest {
    /// Path to a `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Optional variables to render into the pipeline YAML at run
    /// time. Carried on the queue spec; not yet applied (templating
    /// is a follow-up).
    #[serde(default)]
    pub variables: Option<Value>,
    /// Conversation thread to associate this run with, if any.
    #[serde(default)]
    pub thread_id: Option<Uuid>,
    /// Explicit subset of resources (tables) to run, overriding the
    /// spec's `resources`. Caller-settable (HTTP/CLI) — used by "retry
    /// failed tables" to re-run only the streams that failed. Empty =
    /// run the whole spec.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Soft FK → `agentic_schedules.id`. Internal-only — only the scheduler
    /// fire path sets this; HTTP/CLI input cannot, so callers can't spoof
    /// which schedule a run "came from".
    #[serde(skip_deserializing, default)]
    pub schedule_id: Option<String>,
    /// How this run was triggered: `"scheduled"`, `"manual"`, `"backfill"`.
    /// Internal-only — stamped onto `agentic_runs.metadata.trigger`.
    #[serde(skip_deserializing, default)]
    pub trigger: Option<String>,
    /// The cron-scheduled time this run is replaying (UTC). Set by the
    /// backfill path; stamped onto `agentic_runs.metadata.logical_date`.
    #[serde(skip_deserializing, default)]
    pub logical_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Run id this run is a retry of. Set by `retry_run`; stamped onto
    /// `agentic_runs.metadata.retry_of`.
    #[serde(skip_deserializing, default)]
    pub retry_of: Option<String>,
    /// Bounded-backfill window `[from, to)` (RFC3339), set by the backfill
    /// endpoint. Threaded onto the queue spec and applied to the
    /// date-windowed sources (toast, quickbooks, sp_api) — the list lives in
    /// `executor::WINDOWED_BACKFILL_KINDS`. Internal-only — the public `/runs`
    /// body cannot set it (only `/backfill` does).
    #[serde(skip_deserializing, default)]
    pub backfill_from: Option<String>,
    #[serde(skip_deserializing, default)]
    pub backfill_to: Option<String>,
}
