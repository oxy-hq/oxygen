//! Coordinator dashboard routes.
//!
//! GET  /coordinator/active-runs      — list currently active (non-terminal) root runs
//! GET  /coordinator/runs             — list recent runs (paginated)
//! GET  /coordinator/runs/:id/tree    — full task tree for a run
//! POST /coordinator/runs/:id/retry   — clone a terminal-failed run into a fresh one
//! GET  /coordinator/recovery         — recovery & reliability stats
//! GET  /coordinator/queue            — task queue health
//! GET  /coordinator/live             — SSE stream of run status changes

use std::collections::HashMap;
use std::sync::Arc;

use agentic_pipeline::WorkflowWorkspaceContext;
use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::retry::{RetryError, retry_run as pipeline_retry_run};
use agentic_pipeline::usage::{LlmUsageReport, usage_report_for_run, usage_reports_for_runs};
use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    db,
    state::{AgenticState, RunStatus},
};

// ── Response types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ActiveRunEntry {
    pub run_id: String,
    pub status: String,
    pub question: String,
    pub agent_id: String,
    pub source_type: String,
    pub attempt: i32,
    /// Set when this run was seeded by a scheduler fire (or `run_now`).
    pub schedule_id: Option<String>,
    /// `"scheduled"` | `"manual"` | `"backfill"` | `"retry"` | `"promote"` — extracted from
    /// `metadata.trigger`. `None` for legacy runs predating the tag.
    pub trigger: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct ActiveRunsResponse {
    pub runs: Vec<ActiveRunEntry>,
    pub total: usize,
}

#[derive(Deserialize)]
pub struct ListRunsQuery {
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
    /// Comma-separated user-facing statuses: running, suspended, done, failed, cancelled
    #[serde(default)]
    pub status: Option<String>,
    /// Filter by source_type: analytics, builder
    #[serde(default)]
    pub source_type: Option<String>,
    /// Narrow to runs seeded by a specific job (schedule). Hits the
    /// `(schedule_id, created_at desc)` index for per-job run history.
    #[serde(default)]
    pub schedule_id: Option<String>,
    /// When true, system-managed daemon runs (preagg_cycle, etc.) are
    /// included. Default false — they're filtered out by SQL.
    #[serde(default)]
    pub include_system: bool,
}

fn default_limit() -> u64 {
    50
}

#[derive(Serialize)]
pub struct RunHistoryEntry {
    pub run_id: String,
    pub status: String,
    pub question: String,
    pub agent_id: String,
    pub source_type: String,
    pub answer: Option<String>,
    pub error_message: Option<String>,
    pub attempt: i32,
    /// Set when this run was seeded by a scheduler fire (or `run_now`).
    pub schedule_id: Option<String>,
    /// `"scheduled"` | `"manual"` | `"backfill"` | `"retry"` | `"promote"` — extracted from
    /// `metadata.trigger`. `None` for legacy runs predating the tag.
    pub trigger: Option<String>,
    /// Flagged after the fact when a heuristic catches a "healthy but
    /// weird" run: duration spike, cost spike, row drop. Distinct axis
    /// from `status` — a `done` run can still be anomalous.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anomaly: Option<AnomalyInfo>,
    /// USD cost of the run's LLM calls, batched-aggregated at list time.
    /// Estimated — derived from token counts × per-million rates rather
    /// than persisted at write time, so pricing changes after the fact
    /// shift historical numbers slightly. `None` for non-LLM runs
    /// (automation / airway) and for runs whose every model is missing
    /// from the pricing table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Total token count for the run (input + output + cache creation +
    /// cache reads). Surfaced alongside `cost_usd` so a run whose model
    /// is missing from the pricing table still shows a usage signal,
    /// and so cost can be cross-checked against raw activity. `None`
    /// for non-LLM runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_total: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

/// A run-level anomaly flag. The dashboard treats this as a first-class
/// item alongside hard failures so a slow-but-green run isn't silently
/// ignored.
#[derive(Serialize, Clone)]
pub struct AnomalyInfo {
    /// Machine-readable bucket: `"duration_spike"` (only one today; cost
    /// and row-count buckets land with per-type metrics).
    pub kind: String,
    /// Human-readable summary, e.g. `"12m 23s vs p50=4m 11s"`.
    pub detail: String,
    /// `"warning"` (≥ 2× baseline) or `"critical"` (≥ 5× baseline).
    pub severity: String,
}

#[derive(Serialize)]
pub struct RunHistoryResponse {
    pub runs: Vec<RunHistoryEntry>,
    pub total: usize,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn extract_agent_id(metadata: &Option<serde_json::Value>) -> String {
    metadata
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Whitelist of event types kept in the agent-run event log when the
/// frontend waterfall view loads. Token-level chatter
/// (`llm_token`/`thinking_token`) and noisy validators are dropped so
/// the response stays bounded on long runs — the waterfall reconstructs
/// phase boundaries, LLM rounds, tool calls, SQL executions, and
/// automation steps from these alone.
fn is_waterfall_event(event_type: &str) -> bool {
    matches!(
        event_type,
        // Core FSM lifecycle.
        "state_enter"
            | "state_exit"
            | "back_edge"
            | "llm_start"
            | "llm_end"
            | "thinking_start"
            | "thinking_end"
            | "tool_call"
            | "tool_result"
            | "validation_pass"
            | "validation_fail"
            | "fan_out"
            | "sub_spec_start"
            | "sub_spec_end"
            | "awaiting_human_input"
            | "input_resolved"
            | "delegation_started"
            | "delegation_event"
            | "delegation_completed"
            | "done"
            | "error"
            // Analytics domain events that carry the *what* of the Executing
            // phase: the SQL that ran, its row count, and per-step
            // progress for delegated automation runs.
            | "query_generated"
            | "query_executed"
            | "execution_failed"
            | "analytics_validation_failed"
            | "subrun_started"
            | "subrun_step_started"
            | "subrun_step_completed"
            | "subrun_completed"
            // Airway (ELT) domain events — pipeline plan, per-table
            // extract/normalize/load phase markers, schema evolution.
            // Mid-phase progress (`extract_progress`, `load_progress`)
            // is filtered out so the response stays bounded on long
            // streaming syncs.
            | "load_started"
            | "pipeline_plan"
            | "extract_started"
            | "extract_completed"
            | "normalize_started"
            | "normalize_completed"
            | "destination_load_started"
            | "table_load_started"
            | "table_loaded"
            | "table_load_failed"
            | "load_completed"
            | "schema_evolved"
            | "resource_failed"
            | "pipeline_error"
            // Customer-app function jobs — the run's start marker, its persisted
            // `console.*`/`ctx.log` output, and the terminal status/duration.
            // See internal-docs/customer-apps-functions.md.
            | "app_function_started"
            | "function_log"
            | "app_function_completed"
            | "cancelled"
    )
}

/// Pull an arbitrary string out of a run's `metadata` JSON. `None` when
/// the key is missing or the value isn't a string. Used for the airway
/// lineage fields (`source_kind`, `destination_label`, `pipeline_name`)
/// stamped at run-start so the dashboard can label cards before the
/// `pipeline_plan` event fires.
fn extract_metadata_string(metadata: &Option<serde_json::Value>, key: &str) -> Option<String> {
    metadata
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Pull `metadata.trigger` out of a run row. `None` for runs predating the
/// scheduler stamping the field, or for runs not seeded via the schedule
/// fire paths (e.g. ad-hoc thread runs).
fn extract_trigger(metadata: &Option<serde_json::Value>) -> Option<String> {
    metadata
        .as_ref()
        .and_then(|m| m.get("trigger"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Minimum baseline sample size before a duration-spike claim is meaningful.
/// Under this, ratios are too noisy to act on — three slow runs out of three
/// shouldn't all read as "anomalous".
const ANOMALY_MIN_BASELINE_SAMPLES: i64 = 5;
/// Slow-run ratio threshold for the warning band. 2× the median catches the
/// genuine outliers without flagging routine variance.
const ANOMALY_RATIO_WARNING: f64 = 2.0;
/// Promote to "critical" past this ratio so the operator can scan
/// severities in the feed.
const ANOMALY_RATIO_CRITICAL: f64 = 5.0;

/// Flag a run as anomalous if its duration exceeds the per-schedule p50
/// baseline by a meaningful margin. Returns `None` for runs without a
/// schedule, non-`done` runs, missing baselines, or thin baselines.
fn compute_duration_anomaly(
    r: &agentic_runtime::entity::run::Model,
    baselines: &HashMap<String, db::ScheduleDurationBaseline>,
) -> Option<AnomalyInfo> {
    let schedule_id = r.schedule_id.as_deref()?;
    if r.task_status.as_deref() != Some("done") {
        return None;
    }
    let baseline = baselines.get(schedule_id)?;
    if baseline.sample_count < ANOMALY_MIN_BASELINE_SAMPLES || baseline.p50_duration_ms <= 0.0 {
        return None;
    }
    let duration_ms = (r.updated_at - r.created_at).num_milliseconds() as f64;
    if duration_ms <= 0.0 {
        return None;
    }
    let ratio = duration_ms / baseline.p50_duration_ms;
    if ratio < ANOMALY_RATIO_WARNING {
        return None;
    }
    let severity = if ratio >= ANOMALY_RATIO_CRITICAL {
        "critical"
    } else {
        "warning"
    };
    Some(AnomalyInfo {
        kind: "duration_spike".to_string(),
        detail: format!(
            "{} vs p50={}",
            format_duration_ms(duration_ms),
            format_duration_ms(baseline.p50_duration_ms)
        ),
        severity: severity.to_string(),
    })
}

/// Compact duration formatter for anomaly detail strings — keeps the feed
/// rows short (`"12m 23s"`, `"1h 4m"`) so a long detail line doesn't push
/// the chevron off the right edge of the row.
fn format_duration_ms(ms: f64) -> String {
    let secs = (ms / 1000.0).max(0.0) as u64;
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m {}s", mins, secs % 60);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h {}m", hours, mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

// ── GET /coordinator/active-runs ─────────────────────────────────────────────

/// Shared query shape for the active-runs and run-history endpoints —
/// just the system-runs visibility toggle today. Pulled out as its own
/// struct so the active-runs handler can take typed query params.
#[derive(Deserialize, Default)]
pub struct ActiveRunsQuery {
    /// When true, include system-managed daemons (e.g. preagg_cycle).
    /// Default false — system runs flood the live feed at daemon
    /// cadence and the dashboard hides them unless explicitly asked.
    #[serde(default)]
    pub include_system: bool,
}

pub async fn list_active_runs(
    Extension(state): Extension<Arc<AgenticState>>,
    Path(workspace_id): Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<ActiveRunsQuery>,
) -> Response {
    let db = state.db.clone();

    let runs = match db::list_active_runs(&db, workspace_id, query.include_system).await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    let entries: Vec<ActiveRunEntry> = runs
        .into_iter()
        .map(|r| {
            // Prefer in-memory status (more up-to-date) over DB status.
            let live_status = state
                .statuses
                .get(&r.id)
                .map(|s| match s.value() {
                    RunStatus::Running => "running",
                    RunStatus::Suspended { .. } => "suspended",
                    RunStatus::Done => "done",
                    RunStatus::Failed(_) => "failed",
                    RunStatus::Cancelled => "cancelled",
                })
                .unwrap_or_else(|| db::user_facing_status(r.task_status.as_deref()));

            ActiveRunEntry {
                run_id: r.id,
                status: live_status.to_string(),
                question: r.question,
                agent_id: extract_agent_id(&r.metadata),
                source_type: r.source_type.unwrap_or_default(),
                attempt: r.attempt,
                schedule_id: r.schedule_id,
                trigger: extract_trigger(&r.metadata),
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
            }
        })
        .collect();

    let total = entries.len();
    Json(ActiveRunsResponse {
        runs: entries,
        total,
    })
    .into_response()
}

// ── GET /coordinator/runs ────────────────────────────────────────────────────

pub async fn list_runs(
    Extension(state): Extension<Arc<AgenticState>>,
    Path(workspace_id): Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<ListRunsQuery>,
) -> Response {
    let db = state.db.clone();

    let limit = query.limit.min(200);

    // Parse comma-separated status filter.
    let status_strings: Vec<String> = query
        .status
        .as_deref()
        .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
        .unwrap_or_default();
    let status_refs: Vec<&str> = status_strings.iter().map(|s| s.as_str()).collect();
    let status_filter = if status_refs.is_empty() {
        None
    } else {
        Some(status_refs.as_slice())
    };

    let (runs, total_count) = match db::list_runs_filtered(
        &db,
        workspace_id,
        status_filter,
        query.source_type.as_deref(),
        query.schedule_id.as_deref(),
        query.include_system,
        query.offset,
        limit,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    // Anomaly enrichment — one batched baseline lookup covers every
    // schedule appearing in the page. Best-effort: if the baseline query
    // fails we just skip anomaly flags rather than failing the request.
    let schedule_ids: Vec<String> = runs
        .iter()
        .filter_map(|r| {
            (r.task_status.as_deref() == Some("done"))
                .then(|| r.schedule_id.clone())
                .flatten()
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let baselines = db::fetch_duration_baselines(&db, &schedule_ids)
        .await
        .unwrap_or_default();

    // LLM cost enrichment — one batched query covers every run on the
    // page. The query's HAVING clause drops runs without llm events, so
    // automation / airway runs are silently absent from the map.
    let run_ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
    let usage_reports = match usage_reports_for_runs(&db, &run_ids).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "usage_reports_for_runs failed; cost column will be empty");
            HashMap::new()
        }
    };

    let mut entries = Vec::with_capacity(runs.len());
    for r in runs {
        let (status, error_message) =
            db::get_effective_run_state(&db, &r)
                .await
                .unwrap_or_else(|_| {
                    (
                        db::user_facing_status(r.task_status.as_deref()).to_string(),
                        r.error_message.clone(),
                    )
                });

        let trigger = extract_trigger(&r.metadata);
        let anomaly = compute_duration_anomaly(&r, &baselines);
        let usage = usage_reports.get(&r.id);
        let cost_usd = usage.and_then(|u| u.cost_usd);
        let tokens_total = usage.map(|u| {
            u.input_tokens
                + u.output_tokens
                + u.cache_creation_input_tokens
                + u.cache_read_input_tokens
        });
        entries.push(RunHistoryEntry {
            run_id: r.id,
            status,
            question: r.question,
            agent_id: extract_agent_id(&r.metadata),
            source_type: r.source_type.unwrap_or_default(),
            answer: r.answer,
            error_message,
            attempt: r.attempt,
            schedule_id: r.schedule_id,
            trigger,
            anomaly,
            cost_usd,
            tokens_total,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        });
    }

    Json(RunHistoryResponse {
        runs: entries,
        total: total_count as usize,
    })
    .into_response()
}

// ── GET /coordinator/runs/:id/tree ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RunIdPath {
    id: String,
}

/// Path extractor for coordinator endpoints that take both the parent
/// `{workspace_id}` and a local `{id}` (run id). Names must match the
/// route params verbatim — axum populates the struct by name.
#[derive(Deserialize)]
pub struct WorkspaceRunPath {
    workspace_id: uuid::Uuid,
    id: String,
}

#[derive(Serialize)]
pub struct RunEventEntry {
    pub seq: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Serialize)]
pub struct TaskTreeNode {
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub status: String,
    pub question: String,
    pub agent_id: String,
    pub source_type: String,
    pub answer: Option<String>,
    pub error_message: Option<String>,
    pub attempt: i32,
    pub task_status: Option<String>,
    /// "scheduled" | "manual" | "backfill" | "retry" | "promote"; from `metadata.trigger`.
    pub trigger: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Outcome as recorded by the parent coordinator (from agentic_task_outcomes).
    pub outcome_status: Option<String>,
    /// Per-event log for supported source types (e.g. preagg_cycle).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub event_log: Vec<RunEventEntry>,
    /// Per-run LLM token aggregate + USD cost (agent runs only). `None`
    /// for non-LLM runs and for non-root nodes — populated only on the
    /// root by `get_run_tree`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_usage: Option<LlmUsageReport>,
    /// Per-step timing + status breakdown (automation / DAG runs only).
    /// Populated on the root node only — one row per
    /// `subrun_step_started` event, joined to its completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_steps: Option<Vec<db::AutomationStepSummary>>,
    /// Per-table row-count summary (airway / ELT runs only). Aggregated
    /// from the `extract_*` / `table_loaded` events the airway worker
    /// forwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elt_tables: Option<Vec<db::AirwayTableSummary>>,
    /// Source / destination lineage labels stamped on the run at
    /// start time (airway runs). Lets the UI label the lineage cards
    /// before the `pipeline_plan` event fires. `None` for non-airway
    /// runs and for airway runs that predated the metadata stamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_name: Option<String>,
    /// File path that authored this run — `metadata.pipeline_ref` for
    /// airway, `metadata.workflow_ref` for automation. Lets the UI link
    /// from a run detail back to the YAML in the IDE file editor.
    /// `None` for runs that don't have a YAML source (analytics agents
    /// addressed by id, builder runs, preagg daemons).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

#[derive(Serialize)]
pub struct TaskTreeResponse {
    pub root_id: String,
    pub nodes: Vec<TaskTreeNode>,
}

pub async fn get_run_tree(
    Path(WorkspaceRunPath {
        workspace_id,
        id: run_id,
    }): Path<WorkspaceRunPath>,
    Extension(state): Extension<Arc<AgenticState>>,
) -> Response {
    let db = state.db.clone();

    let runs = match db::load_task_tree_in_workspace(&db, workspace_id, &run_id).await {
        Ok(r) if r.is_empty() => {
            return (StatusCode::NOT_FOUND, "run not found").into_response();
        }
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    // Collect all task outcomes for runs that have children.
    let parent_ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
    let mut outcome_map: HashMap<String, String> = HashMap::new();
    for pid in &parent_ids {
        if let Ok(outcomes) = db::get_outcomes_for_parent(&db, pid).await {
            for o in outcomes {
                outcome_map.insert(o.child_id, o.status);
            }
        }
    }

    let mut nodes: Vec<TaskTreeNode> = runs
        .into_iter()
        .map(|r| {
            let live_status = state
                .statuses
                .get(&r.id)
                .map(|s| match s.value() {
                    RunStatus::Running => "running",
                    RunStatus::Suspended { .. } => "suspended",
                    RunStatus::Done => "done",
                    RunStatus::Failed(_) => "failed",
                    RunStatus::Cancelled => "cancelled",
                })
                .unwrap_or_else(|| db::user_facing_status(r.task_status.as_deref()));

            let trigger = extract_trigger(&r.metadata);
            let source_kind = extract_metadata_string(&r.metadata, "source_kind");
            let destination_label = extract_metadata_string(&r.metadata, "destination_label");
            let pipeline_name = extract_metadata_string(&r.metadata, "pipeline_name");
            // `pipeline_ref` (airway) or `workflow_ref` (automation) — whichever
            // the seeder stamped. Used by the run detail UI to link
            // back to the source YAML in the IDE editor.
            let source_ref = extract_metadata_string(&r.metadata, "pipeline_ref")
                .or_else(|| extract_metadata_string(&r.metadata, "workflow_ref"));
            TaskTreeNode {
                outcome_status: outcome_map.get(&r.id).cloned(),
                run_id: r.id,
                parent_run_id: r.parent_run_id,
                status: live_status.to_string(),
                question: r.question,
                agent_id: extract_agent_id(&r.metadata),
                source_type: r.source_type.unwrap_or_default(),
                answer: r.answer,
                error_message: r.error_message,
                attempt: r.attempt,
                task_status: r.task_status,
                trigger,
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
                event_log: Vec::new(),
                llm_usage: None,
                dag_steps: None,
                elt_tables: None,
                source_kind,
                destination_label,
                pipeline_name,
                source_ref,
            }
        })
        .collect();

    // Enrich preagg_cycle nodes with their per-rollup event log, and
    // agent runs (analytics / builder) with the structural events the
    // waterfall view needs: state transitions, LLM rounds, tool calls,
    // thinking blocks. Token-level chatter is filtered out so the
    // payload size stays bounded on long runs.
    //
    // Fetch all events in one batched query instead of one per node to
    // avoid N sequential round-trips for large automation trees.
    let enriched_run_ids: Vec<String> = nodes
        .iter()
        .filter(|n| {
            matches!(
                n.source_type.as_str(),
                "preagg_cycle" | "analytics" | "builder" | "workflow" | "airway"
            )
        })
        .map(|n| n.run_id.clone())
        .collect();
    let mut events_by_run = db::get_all_events_for_runs(&db, &enriched_run_ids)
        .await
        .unwrap_or_default();
    for node in &mut nodes {
        let events = match events_by_run.remove(&node.run_id) {
            Some(e) => e,
            None => continue,
        };
        match node.source_type.as_str() {
            "preagg_cycle" => {
                node.event_log = events
                    .into_iter()
                    .filter(|e| e.event_type.starts_with("preagg_rollup"))
                    .map(|e| RunEventEntry {
                        seq: e.seq,
                        event_type: e.event_type,
                        payload: e.payload,
                    })
                    .collect();
            }
            "analytics" | "builder" | "workflow" | "airway" => {
                node.event_log = events
                    .into_iter()
                    .filter(|e| is_waterfall_event(&e.event_type))
                    .map(|e| RunEventEntry {
                        seq: e.seq,
                        event_type: e.event_type,
                        payload: e.payload,
                    })
                    .collect();
            }
            _ => {}
        }
    }

    // Stamp the root with type-specific summaries. Best-effort: a
    // failed lookup leaves the field unset rather than failing the
    // whole request. Only the root carries these — sub-runs roll up.
    if let Some(root) = nodes.iter_mut().find(|n| n.run_id == run_id) {
        match usage_report_for_run(&db, &run_id).await {
            Ok(Some(report)) => root.llm_usage = Some(report),
            Ok(None) => {}
            Err(e) => tracing::warn!(%run_id, error = %e, "usage_report_for_run failed"),
        }
        if root.source_type == "workflow"
            && let Ok(steps) = db::automation_step_summary_for_run(&db, &run_id).await
            && !steps.is_empty()
        {
            root.dag_steps = Some(steps);
        }
        if root.source_type == "airway"
            && let Ok(tables) = db::airway_table_summary_for_run(&db, &run_id).await
            && !tables.is_empty()
        {
            root.elt_tables = Some(tables);
        }
    }

    Json(TaskTreeResponse {
        root_id: run_id,
        nodes,
    })
    .into_response()
}

// ── POST /coordinator/runs/:id/retry ──────────────────────────────────────────

#[derive(Serialize)]
pub struct RetryRunResponse {
    /// `run_id` of the freshly seeded retry. The original stays as-is.
    pub run_id: String,
}

/// Clone-and-reseed a terminal-failed / cancelled / timed-out run. The new
/// run carries the same `schedule_id` and is tagged `trigger="retry"` with
/// `metadata.retry_of` linking back to the original.
pub async fn retry_run(
    Path(WorkspaceRunPath {
        workspace_id,
        id: run_id,
    }): Path<WorkspaceRunPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    match pipeline_retry_run(&state.db, workspace_id, workspace.as_ref(), &run_id).await {
        Ok(new_run_id) => Json(RetryRunResponse { run_id: new_run_id }).into_response(),
        Err(RetryError::NotFound) => (StatusCode::NOT_FOUND, "run not found").into_response(),
        Err(RetryError::NotRetryable(m)) => (StatusCode::BAD_REQUEST, m).into_response(),
        // 409, and logged at info: a lease refusal is the guard working, not a
        // fault. `warn!` + 500 here would page on the common cross-replica
        // cancel-then-retry path.
        Err(RetryError::Conflict(m)) => {
            tracing::info!(%run_id, reason = %m, "retry: refused, run or pipeline still active");
            (StatusCode::CONFLICT, m).into_response()
        }
        Err(RetryError::SeedFailed(m)) => {
            tracing::warn!(%run_id, error = %m, "retry: seed failed");
            (StatusCode::INTERNAL_SERVER_ERROR, m).into_response()
        }
        Err(RetryError::Db(e)) => {
            tracing::error!(%run_id, error = %e, "retry: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ── GET /coordinator/recovery ─────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AgentStats {
    pub agent_id: String,
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub recovered: usize,
}

#[derive(Serialize)]
pub struct RecoveredRunEntry {
    pub run_id: String,
    pub status: String,
    pub question: String,
    pub agent_id: String,
    pub attempt: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct RecoveryResponse {
    /// Total root runs in the window.
    pub total_runs: usize,
    /// Runs that were recovered (attempt > 0).
    pub recovered_count: usize,
    /// Runs that ended in failure.
    pub failed_count: usize,
    /// Runs that ended in cancellation.
    pub cancelled_count: usize,
    /// Runs that completed successfully.
    pub succeeded_count: usize,
    /// Per-agent breakdown.
    pub agents: Vec<AgentStats>,
    /// Recovered runs (attempt > 0), most recent first.
    pub recovered_runs: Vec<RecoveredRunEntry>,
}

pub async fn get_recovery_stats(
    Extension(state): Extension<Arc<AgenticState>>,
    Path(workspace_id): Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<ListRunsQuery>,
) -> Response {
    let db = state.db.clone();

    let limit = query.limit.min(500);
    let runs = match db::list_recent_runs(&db, workspace_id, limit).await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    // Only root runs (no children).
    let root_runs: Vec<_> = runs
        .into_iter()
        .filter(|r| r.parent_run_id.is_none())
        .collect();

    let mut recovered_count = 0usize;
    let mut failed_count = 0usize;
    let mut cancelled_count = 0usize;
    let mut succeeded_count = 0usize;
    let mut agent_map: HashMap<String, (usize, usize, usize, usize)> = HashMap::new(); // (total, ok, fail, recovered)
    let mut recovered_runs = Vec::new();

    for r in &root_runs {
        let agent_id = extract_agent_id(&r.metadata);
        let status = db::user_facing_status(r.task_status.as_deref());

        match status {
            "done" => succeeded_count += 1,
            "failed" => failed_count += 1,
            "cancelled" => cancelled_count += 1,
            _ => {}
        }

        let entry = agent_map.entry(agent_id.clone()).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        if status == "done" {
            entry.1 += 1;
        }
        if status == "failed" {
            entry.2 += 1;
        }

        if r.attempt > 0 {
            recovered_count += 1;
            entry.3 += 1;
            recovered_runs.push(RecoveredRunEntry {
                run_id: r.id.clone(),
                status: status.to_string(),
                question: r.question.clone(),
                agent_id,
                attempt: r.attempt,
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
            });
        }
    }

    let agents: Vec<AgentStats> = agent_map
        .into_iter()
        .map(
            |(agent_id, (total, succeeded, failed, recovered))| AgentStats {
                agent_id,
                total,
                succeeded,
                failed,
                recovered,
            },
        )
        .collect();

    Json(RecoveryResponse {
        total_runs: root_runs.len(),
        recovered_count,
        failed_count,
        cancelled_count,
        succeeded_count,
        agents,
        recovered_runs,
    })
    .into_response()
}

// ── GET /coordinator/queue ────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct QueueTaskEntry {
    pub task_id: String,
    pub run_id: String,
    pub queue_status: String,
    pub worker_id: Option<String>,
    pub claim_count: i32,
    pub max_claims: i32,
    pub last_heartbeat: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct QueueHealthResponse {
    pub queued: u64,
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub dead: u64,
    pub stale_tasks: Vec<QueueTaskEntry>,
    pub dead_tasks: Vec<QueueTaskEntry>,
}

fn queue_row_to_entry(m: db::QueueTaskRow) -> QueueTaskEntry {
    QueueTaskEntry {
        task_id: m.task_id,
        run_id: m.run_id,
        queue_status: m.queue_status,
        worker_id: m.worker_id,
        claim_count: m.claim_count,
        max_claims: m.max_claims,
        last_heartbeat: m.last_heartbeat.map(|t| t.to_rfc3339()),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

pub async fn get_queue_health(
    Extension(state): Extension<Arc<AgenticState>>,
    Path(workspace_id): Path<uuid::Uuid>,
) -> Response {
    let db = state.db.clone();

    let stats = match db::get_queue_stats(&db, workspace_id).await {
        Ok(s) => s,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")).into_response();
        }
    };

    Json(QueueHealthResponse {
        queued: stats.queued,
        claimed: stats.claimed,
        completed: stats.completed,
        failed: stats.failed,
        cancelled: stats.cancelled,
        dead: stats.dead,
        stale_tasks: stats
            .stale_tasks
            .into_iter()
            .map(queue_row_to_entry)
            .collect(),
        dead_tasks: stats
            .dead_tasks
            .into_iter()
            .map(queue_row_to_entry)
            .collect(),
    })
    .into_response()
}

// ── GET /coordinator/live (SSE) ──────────────────────────────────────────────
//
// Streams run status snapshots every time any run's status changes.
// The client receives a periodic snapshot of all in-memory run statuses.

#[derive(Serialize)]
struct LiveStatusEntry {
    run_id: String,
    status: String,
}

pub async fn live_stream(
    Extension(state): Extension<Arc<AgenticState>>,
    Path(workspace_id): Path<uuid::Uuid>,
) -> Response {
    let stream = async_stream::stream! {
        // Send an initial snapshot immediately, filtered to this workspace.
        let snapshot = workspace_snapshot(&state, workspace_id).await;
        let event = SseEvent::default()
            .event("snapshot")
            .data(serde_json::to_string(&snapshot).unwrap_or_default());
        yield Ok::<_, std::convert::Infallible>(event);

        // Poll every 2 seconds for status changes.
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut last_snapshot = snapshot;

        loop {
            tokio::select! {
                _ = interval.tick() => {},
                _ = state.shutdown_token.cancelled() => break,
            }

            let current = workspace_snapshot(&state, workspace_id).await;
            if current != last_snapshot {
                let event = SseEvent::default()
                    .event("snapshot")
                    .data(serde_json::to_string(&current).unwrap_or_default());
                yield Ok(event);
                last_snapshot = current;
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// The in-memory status map is global (one process drives runs across
/// every workspace) so each poll asks the DB which of the current run
/// ids belong to the subscriber's workspace and filters the snapshot.
/// One small SELECT per poll — cheap enough at the 2-second cadence.
async fn workspace_snapshot(
    state: &AgenticState,
    workspace_id: uuid::Uuid,
) -> Vec<LiveStatusEntry> {
    let full = build_snapshot(state);
    if full.is_empty() {
        return full;
    }
    let run_ids: Vec<String> = full.iter().map(|s| s.run_id.clone()).collect();
    let allowed = match db::runs_in_workspace(&state.db, workspace_id, &run_ids).await {
        Ok(set) => set,
        Err(e) => {
            tracing::warn!(error = %e, "live_stream: workspace filter query failed");
            return Vec::new();
        }
    };
    full.into_iter()
        .filter(|e| allowed.contains(&e.run_id))
        .collect()
}

fn build_snapshot(state: &AgenticState) -> Vec<LiveStatusEntry> {
    state
        .statuses
        .iter()
        .map(|entry| {
            let status = match entry.value() {
                RunStatus::Running => "running",
                RunStatus::Suspended { .. } => "suspended",
                RunStatus::Done => "done",
                RunStatus::Failed(_) => "failed",
                RunStatus::Cancelled => "cancelled",
            };
            LiveStatusEntry {
                run_id: entry.key().clone(),
                status: status.to_string(),
            }
        })
        .collect()
}

impl PartialEq for LiveStatusEntry {
    fn eq(&self, other: &Self) -> bool {
        self.run_id == other.run_id && self.status == other.status
    }
}
