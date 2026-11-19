//! The queue row `/admin/internal-jobs/*` returns: the DTO, the two raw shapes it is
//! read from (plain, and joined with its tenant and run context), and the secret
//! redaction every decoded `TaskSpec` passes through on its way out.
//!
//! Split out of `internal_jobs.rs` by responsibility; the routes are mounted there.

use chrono::{DateTime, FixedOffset};
use sea_orm::FromQueryResult;
use serde::Serialize;
use uuid::Uuid;

/// A failed/dead job, enriched with the tenant + run context an operator needs
/// to actually debug it. The enriched fields are LEFT-joined from
/// `agentic_runs → workspaces → organizations` (+ `threads → users`), so they
/// are `Option` — system jobs or orphaned runs render with nulls rather than
/// being dropped.
#[derive(Serialize, Debug)]
pub struct QueueRowDto {
    pub task_id: String,
    pub run_id: String,
    pub queue_status: String,
    pub worker_id: Option<String>,
    pub claim_count: i32,
    pub max_claims: i32,
    pub last_heartbeat: Option<DateTime<FixedOffset>>,
    pub claimed_at: Option<DateTime<FixedOffset>>,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    pub task_type: Option<String>,
    /// Decoded `TaskSpec` JSON so the UI can show agent_id / workflow_ref /
    /// question / variables without a second round-trip.
    pub spec: serde_json::Value,
    // --- enriched tenant + run context (LEFT-joined) ---
    pub workspace_id: Option<Uuid>,
    pub workspace_name: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub run_status: Option<String>,
    pub run_error_message: Option<String>,
    pub originating_user_email: Option<String>,
}

/// Basic queue row (no joins) — used by the single-row re-fetch after
/// re-enqueue, where tenant context is not needed.
#[derive(Debug, FromQueryResult)]
pub(super) struct QueueRowRaw {
    task_id: String,
    run_id: String,
    queue_status: String,
    worker_id: Option<String>,
    claim_count: i32,
    max_claims: i32,
    last_heartbeat: Option<DateTime<FixedOffset>>,
    claimed_at: Option<DateTime<FixedOffset>>,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
    spec: serde_json::Value,
}

impl From<QueueRowRaw> for QueueRowDto {
    fn from(r: QueueRowRaw) -> Self {
        let task_type = extract_task_type(&r.spec);
        Self {
            task_id: r.task_id,
            run_id: r.run_id,
            queue_status: r.queue_status,
            worker_id: r.worker_id,
            claim_count: r.claim_count,
            max_claims: r.max_claims,
            last_heartbeat: r.last_heartbeat,
            claimed_at: r.claimed_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            task_type,
            spec: redact_secrets(r.spec),
            workspace_id: None,
            workspace_name: None,
            org_id: None,
            org_name: None,
            run_status: None,
            run_error_message: None,
            originating_user_email: None,
        }
    }
}

/// Enriched queue row — basic columns plus the joined tenant/run context.
#[derive(Debug, FromQueryResult)]
pub(super) struct EnrichedQueueRowRaw {
    task_id: String,
    run_id: String,
    queue_status: String,
    worker_id: Option<String>,
    claim_count: i32,
    max_claims: i32,
    last_heartbeat: Option<DateTime<FixedOffset>>,
    claimed_at: Option<DateTime<FixedOffset>>,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
    spec: serde_json::Value,
    workspace_id: Option<Uuid>,
    workspace_name: Option<String>,
    org_id: Option<Uuid>,
    org_name: Option<String>,
    run_status: Option<String>,
    run_error_message: Option<String>,
    originating_user_email: Option<String>,
}

impl From<EnrichedQueueRowRaw> for QueueRowDto {
    fn from(r: EnrichedQueueRowRaw) -> Self {
        let task_type = extract_task_type(&r.spec);
        Self {
            task_id: r.task_id,
            run_id: r.run_id,
            queue_status: r.queue_status,
            worker_id: r.worker_id,
            claim_count: r.claim_count,
            max_claims: r.max_claims,
            last_heartbeat: r.last_heartbeat,
            claimed_at: r.claimed_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            task_type,
            spec: redact_secrets(r.spec),
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
            org_id: r.org_id,
            org_name: r.org_name,
            run_status: r.run_status,
            run_error_message: r.run_error_message,
            originating_user_email: r.originating_user_email,
        }
    }
}

/// Shared SELECT + JOIN prefix for the enriched failure/dead-letter feeds.
/// Callers append a `WHERE` clause, `ORDER BY`, and `LIMIT`/`OFFSET`.
/// All joins are LEFT joins so a job whose run/workspace/org is missing still
/// appears (with null context) instead of silently vanishing.
pub(super) const ENRICHED_SELECT: &str = "\
    SELECT q.task_id, q.run_id, q.queue_status, q.worker_id, q.claim_count, \
           q.max_claims, q.last_heartbeat, q.claimed_at, q.created_at, \
           q.updated_at, q.spec, \
           r.task_status AS run_status, r.error_message AS run_error_message, \
           r.workspace_id AS workspace_id, \
           w.name AS workspace_name, w.org_id AS org_id, \
           o.name AS org_name, u.email AS originating_user_email \
    FROM agentic_task_queue q \
    LEFT JOIN agentic_runs r ON q.run_id = r.id \
    LEFT JOIN workspaces w ON r.workspace_id = w.id \
    LEFT JOIN organizations o ON w.org_id = o.id \
    LEFT JOIN threads t ON r.thread_id = t.id \
    LEFT JOIN users u ON t.user_id = u.id ";

/// Best-effort extraction of a "task type" tag from the serialized
/// `TaskSpec` JSON for display in the UI. The spec is a tagged enum so we
/// peek at the top-level keys; if the shape changes, fall back to None
/// (the column is purely informational).
/// Defense-in-depth: blank out obviously secret-shaped values before the
/// decoded `TaskSpec` is sent to the admin debug panel. Credentials are
/// supposed to live in the secret manager, but a misconfigured airway/automation
/// pipeline could inline a token into `variables`/`payload`; this keeps it from
/// surfacing verbatim in the UI. Operator-only guard already bounds exposure —
/// this is the belt to that guard's braces.
pub(super) fn redact_secrets(mut spec: serde_json::Value) -> serde_json::Value {
    redact_in_place(&mut spec);
    spec
}

fn redact_in_place(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if is_secret_key(k) {
                    *val = serde_json::Value::String("***redacted***".to_string());
                } else {
                    redact_in_place(val);
                }
            }
        }
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(redact_in_place),
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "token",
        "credential",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
        "authorization",
    ];
    let k = key.to_lowercase();
    NEEDLES.iter().any(|n| k.contains(n))
}

pub(super) fn extract_task_type(spec: &serde_json::Value) -> Option<String> {
    if let Some(obj) = spec.as_object() {
        // `TaskSpec` is internally tagged (`#[serde(tag = "type")]`), so the
        // variant name lives under the `type` key (e.g. `"agent"`,
        // `"workflow"`, `"airway"`). Fall back to the first key for any
        // legacy/externally-tagged shape.
        if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
            return Some(t.to_string());
        }
        if let Some((k, _)) = obj.iter().next() {
            return Some(k.clone());
        }
    }
    None
}
