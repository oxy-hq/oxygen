//! Read-side queries over runs, events, and thread history.

use std::collections::HashMap;

use sea_orm::{
    ColumnTrait, Condition, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, Statement,
};
use serde_json::Value;
use uuid::Uuid;

use crate::lifecycle::entity::{run, run_event};

use super::user_facing_status;
use super::visibility::{in_customer_feed, in_production, in_production_sql};

/// Run source_types that are background daemons rather than user-scheduled
/// work. The coordinator dashboard hides them from the main feed by
/// default — they'd otherwise flood the run list at the worker's
/// heartbeat cadence (preagg_cycle fires every 30s by default).
/// Must stay in sync with the frontend constant of the same name.
///
/// Not the same list as `NON_WORKSPACE_RUN_SOURCES` in
/// `oxy-app`'s `server::api::admin::workspace_health::queries`, and not a
/// superset of it: this one answers "hide from the run feed?", that one answers
/// "does a failure here indict the tenant?". `health_eval_workspace` is excluded
/// there but stays visible here on purpose. A new daemon `source_type` should be
/// considered for both.
///
/// `preview_analyze` is the staff-only Airway change check of a previewed
/// branch (`oxy-app` `server::previews::analyze`), not the customer's work;
/// `preview_schema_drop` is the TTL drop of a preview's Airhouse schemas
/// (`server::previews::drop`), queued by a maintenance sweep; `preview_compare`
/// compares a preview's transform build with live (`server::previews::compare`);
/// `preview_airway_sample` is staff sampling a branch's Airway pipeline into
/// the preview's own schemas (`server::previews::sample`).
/// `custom_app_staging_migrations` is a custom app's staging migrations, queued
/// by every publish (`server::api::custom_apps_nonproduction::staging_task`).
/// `custom_app_sandbox_migrations`, `custom_app_sandbox_oltp` and
/// `custom_app_sandbox_teardown` are a custom-app sandbox's Airhouse
/// migrations and its own OLTP schema (each queued by a publish to it) and its
/// teardown (queued by a delete or an expiry) —
/// `server::api::custom_apps_sandboxes`: staff work on a staff surface.
pub const SYSTEM_SOURCE_TYPES: &[&str] = &[
    "preagg_cycle",
    "preview_analyze",
    "preview_schema_drop",
    "preview_compare",
    "preview_airway_sample",
    "custom_app_staging_migrations",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_oltp",
    "custom_app_sandbox_teardown",
];

pub struct ToolExchangeRow {
    pub name: String,
    pub input: String,
    pub output: String,
}

pub struct ThreadHistoryTurn {
    pub question: String,
    pub answer: String,
    /// Full run metadata — callers extract domain-specific fields.
    pub metadata: Option<Value>,
}

pub async fn get_run(db: &DatabaseConnection, run_id: &str) -> Result<Option<run::Model>, DbErr> {
    run::Entity::find_by_id(run_id.to_string()).one(db).await
}

pub async fn get_run_by_thread(
    db: &DatabaseConnection,
    thread_id: Uuid,
) -> Result<Option<run::Model>, DbErr> {
    run::Entity::find()
        .filter(run::Column::ThreadId.eq(thread_id))
        .order_by_desc(run::Column::CreatedAt)
        .one(db)
        .await
}

pub async fn get_runs_by_thread(
    db: &DatabaseConnection,
    thread_id: Uuid,
) -> Result<Vec<run::Model>, DbErr> {
    run::Entity::find()
        .filter(run::Column::ThreadId.eq(thread_id))
        .order_by_asc(run::Column::CreatedAt)
        .all(db)
        .await
}

/// Given a candidate set of run ids, return the subset that belongs to
/// `workspace_id`. Used by the live SSE poll to filter the in-memory
/// status snapshot — the in-memory map is global and doesn't know about
/// workspaces, but each run id resolves to exactly one workspace via DB.
/// A check run queued outside production is staff's, not the workspace's
/// members', and is left out (see `visibility`).
pub async fn runs_in_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    run_ids: &[String],
) -> Result<std::collections::HashSet<String>, DbErr> {
    if run_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    #[derive(FromQueryResult)]
    struct RunId {
        id: String,
    }
    let rows = RunId::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT id FROM agentic_runs WHERE id = ANY($1) AND workspace_id = $2 AND {}",
            in_production_sql()
        ),
        [run_ids.to_vec().into(), workspace_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// List recent runs across all threads, ordered newest-first. A check run
/// queued outside production is never listed (see `visibility`).
pub async fn list_recent_runs(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    limit: u64,
) -> Result<Vec<run::Model>, DbErr> {
    use sea_orm::QuerySelect;
    run::Entity::find()
        .filter(run::Column::WorkspaceId.eq(workspace_id))
        .filter(in_production())
        .order_by_desc(run::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
}

/// List root runs with optional filters, paginated. Returns (runs, total_count).
///
/// **Workspace-scoped.** Hits the `(workspace_id, task_status)` index
/// and never crosses tenants — cross-workspace listings would leak
/// runs between organizations on a multi-tenant deployment.
/// `schedule_id_filter` narrows the list to runs seeded by a specific job;
/// hits the `(schedule_id, created_at desc)` index for cheap per-job history.
/// `include_system` lets background-daemon runs (preagg_cycle, etc.) into
/// the result — default `false` keeps the operator's feed clean. A preview
/// dry run and a check run queued outside production are never listed (see
/// `visibility::in_customer_feed`).
#[allow(clippy::too_many_arguments)]
pub async fn list_runs_filtered(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    status_filter: Option<&[&str]>,
    source_type_filter: Option<&str>,
    schedule_id_filter: Option<&str>,
    include_system: bool,
    offset: u64,
    limit: u64,
) -> Result<(Vec<run::Model>, u64), DbErr> {
    use sea_orm::{PaginatorTrait, QuerySelect};

    let mut query = run::Entity::find()
        .filter(run::Column::WorkspaceId.eq(workspace_id))
        .filter(run::Column::ParentRunId.is_null())
        .filter(in_customer_feed());

    if let Some(statuses) = status_filter {
        // Map user-facing statuses to internal task_status values.
        let mut cond = Condition::any();
        for s in statuses {
            match *s {
                "running" => {
                    cond = cond
                        .add(run::Column::TaskStatus.eq("running"))
                        .add(run::Column::TaskStatus.eq("delegating"))
                        .add(run::Column::TaskStatus.eq("waiting_on_child"))
                        .add(run::Column::TaskStatus.eq("waiting_on_children"));
                }
                "suspended" => {
                    cond = cond.add(run::Column::TaskStatus.eq("awaiting_input"));
                }
                "done" => {
                    cond = cond.add(run::Column::TaskStatus.eq("done"));
                }
                "failed" => {
                    cond = cond
                        .add(run::Column::TaskStatus.eq("failed"))
                        .add(run::Column::TaskStatus.eq("timed_out"));
                }
                "cancelled" => {
                    cond = cond.add(run::Column::TaskStatus.eq("cancelled"));
                }
                _ => {}
            }
        }
        query = query.filter(cond);
    }

    if let Some(src) = source_type_filter {
        query = query.filter(run::Column::SourceType.eq(src));
    }

    if let Some(sched) = schedule_id_filter {
        query = query.filter(run::Column::ScheduleId.eq(sched));
    }

    if !include_system {
        query =
            query.filter(run::Column::SourceType.is_not_in(SYSTEM_SOURCE_TYPES.iter().copied()));
    }

    let total = query.clone().count(db).await?;
    let runs = query
        .order_by_desc(run::Column::CreatedAt)
        .offset(offset)
        .limit(limit)
        .all(db)
        .await?;

    Ok((runs, total))
}

/// Per-schedule duration baseline used by the anomaly detector. `p50` is
/// Postgres `PERCENTILE_CONT(0.5)` over the last 30 days of `done` runs for
/// the schedule — a single SQL trip computes it for every schedule the
/// caller cares about at once.
#[derive(Debug, Clone, FromQueryResult)]
pub struct ScheduleDurationBaseline {
    pub schedule_id: String,
    pub p50_duration_ms: f64,
    pub sample_count: i64,
}

/// Fetch median run-duration per schedule for `done` runs in the last 30
/// days. Returns one row per schedule that has at least one matching run;
/// the caller decides whether the sample count is high enough to trust.
pub async fn fetch_duration_baselines(
    db: &DatabaseConnection,
    schedule_ids: &[String],
) -> Result<HashMap<String, ScheduleDurationBaseline>, DbErr> {
    if schedule_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = ScheduleDurationBaseline::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // `PERCENTILE_CONT` returns an interval; convert via EPOCH to ms so
        // the application side does plain f64 math against actual run
        // durations. 30-day window keeps the baseline responsive without
        // sliding it on every page render.
        "SELECT schedule_id, \
                EXTRACT(EPOCH FROM PERCENTILE_CONT(0.5) \
                  WITHIN GROUP (ORDER BY (updated_at - created_at))) * 1000.0 \
                  AS p50_duration_ms, \
                COUNT(*) AS sample_count \
         FROM agentic_runs \
         WHERE schedule_id = ANY($1) \
           AND task_status = 'done' \
           AND created_at > NOW() - INTERVAL '30 days' \
         GROUP BY schedule_id",
        [schedule_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.schedule_id.clone(), r))
        .collect())
}

/// Aggregated LLM token usage for one run, summed across every
/// `llm_start` / `llm_end` event row in `agentic_run_events`. The events
/// already carry per-round counts (input, output, cache); this is just
/// the sum + the distinct set of models the run used. Pricing is layered
/// on at a higher layer (this crate doesn't depend on `agentic-llm`).
#[derive(Debug, Clone, FromQueryResult)]
pub struct LlmTokenSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    /// Distinct LLM models seen in the `llm_end` events for this run,
    /// comma-separated (Postgres `string_agg`). A single-model run is
    /// the common case.
    pub models: Option<String>,
    /// Count of `llm_end` events — i.e. completed LLM HTTP rounds.
    pub call_count: i64,
}

/// Same shape as [`LlmTokenSummary`] but with the owning run id so the
/// batched variant can return many rows in one trip.
#[derive(Debug, Clone, FromQueryResult)]
pub struct LlmTokenSummaryByRun {
    pub run_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub models: Option<String>,
    pub call_count: i64,
}

/// Batched variant of [`llm_usage_for_run`] — one SQL trip for an entire
/// page of runs. Used by the run-list endpoint so we can show a cost
/// hint per row without N+1 queries.
pub async fn llm_usage_for_runs(
    db: &DatabaseConnection,
    run_ids: &[String],
) -> Result<std::collections::HashMap<String, LlmTokenSummaryByRun>, DbErr> {
    if run_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    // PostgreSQL's `SUM(bigint)` widens to `numeric` to avoid overflow,
    // which Sea-ORM cannot deserialize into `i64` — that previously
    // turned the entire row into a silent `DbErr` and the dashboard
    // saw "no llm events". Cast every SUM back to bigint explicitly.
    let rows = LlmTokenSummaryByRun::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT \
            run_id, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_start' \
                              THEN (payload->>'prompt_tokens')::bigint \
                              ELSE 0 END), 0)::bigint AS input_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN (payload->>'output_tokens')::bigint \
                              ELSE 0 END), 0)::bigint AS output_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN COALESCE((payload->>'cache_creation_input_tokens')::bigint, 0) \
                              ELSE 0 END), 0)::bigint AS cache_creation_input_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN COALESCE((payload->>'cache_read_input_tokens')::bigint, 0) \
                              ELSE 0 END), 0)::bigint AS cache_read_input_tokens, \
            string_agg(DISTINCT payload->>'model', ',') \
                FILTER (WHERE event_type = 'llm_end' AND payload->>'model' IS NOT NULL) AS models, \
            COUNT(*) FILTER (WHERE event_type = 'llm_end') AS call_count \
         FROM agentic_run_events \
         WHERE run_id = ANY($1) AND event_type IN ('llm_start', 'llm_end') \
         GROUP BY run_id \
         HAVING COUNT(*) FILTER (WHERE event_type = 'llm_end') > 0",
        [run_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.run_id.clone(), r)).collect())
}

/// Sum LLM token usage for one run from its persisted events. Returns
/// `None` when the run has no llm events (e.g. an automation or airway run,
/// or an agent run that never called the LLM).
pub async fn llm_usage_for_run(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<LlmTokenSummary>, DbErr> {
    let row = LlmTokenSummary::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // `llm_start` carries `prompt_tokens` (input); `llm_end` carries
        // output + cache counts + the model id. Summing both event types
        // in one pass keeps this to a single query per run.
        //
        // Each SUM is cast back to bigint — Postgres widens `SUM(bigint)`
        // to numeric to avoid overflow, which Sea-ORM cannot deserialize
        // into i64 (silent DbErr, swallowed by the caller's `if let Ok`).
        "SELECT \
            COALESCE(SUM(CASE WHEN event_type = 'llm_start' \
                              THEN (payload->>'prompt_tokens')::bigint \
                              ELSE 0 END), 0)::bigint AS input_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN (payload->>'output_tokens')::bigint \
                              ELSE 0 END), 0)::bigint AS output_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN COALESCE((payload->>'cache_creation_input_tokens')::bigint, 0) \
                              ELSE 0 END), 0)::bigint AS cache_creation_input_tokens, \
            COALESCE(SUM(CASE WHEN event_type = 'llm_end' \
                              THEN COALESCE((payload->>'cache_read_input_tokens')::bigint, 0) \
                              ELSE 0 END), 0)::bigint AS cache_read_input_tokens, \
            string_agg(DISTINCT payload->>'model', ',') \
                FILTER (WHERE event_type = 'llm_end' AND payload->>'model' IS NOT NULL) AS models, \
            COUNT(*) FILTER (WHERE event_type = 'llm_end') AS call_count \
         FROM agentic_run_events \
         WHERE run_id = $1 AND event_type IN ('llm_start', 'llm_end')",
        [run_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.filter(|s| s.call_count > 0))
}

/// One row of the per-step automation timing table — the load-bearing
/// metric for the DAG debugging unit. Reconstructed at query time from
/// the `subrun_step_started` / `subrun_step_completed` event pair: no
/// extension table or new column needed.
#[derive(Debug, Clone, FromQueryResult, serde::Serialize)]
pub struct AutomationStepSummary {
    /// Step name as it appears in the automation YAML.
    pub step_name: String,
    /// `succeeded` | `failed` | `cached` | `running` — derived from the
    /// completed event's payload (or `running` if no completed event
    /// exists yet).
    pub status: String,
    pub started_at: sea_orm::prelude::DateTimeWithTimeZone,
    /// `None` while the step is still in flight.
    pub completed_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    /// Wall-clock duration in ms. `None` while in flight; the frontend
    /// can compute `now() - started_at` for the live case.
    pub duration_ms: Option<i64>,
    /// Error message from the completed event, when status is `failed`.
    pub error: Option<String>,
    /// True when the completed event carried `cached: true` (cache hit).
    pub cached: bool,
}

/// Aggregate per-step timings + status for an automation run from its
/// persisted events. Returns one row per `subrun_step_started` event;
/// steps that haven't completed yet have `status = "running"` and
/// `duration_ms = None`.
pub async fn automation_step_summary_for_run(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Vec<AutomationStepSummary>, DbErr> {
    // For each `started` row, `DISTINCT ON (s.seq)` picks the very next
    // `completed` row (ordered by completed.seq ASC). This pairs each
    // start to its own completion even when the same step name fires
    // multiple times across a loop's iterations.
    let rows = AutomationStepSummary::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT DISTINCT ON (s.seq) \
            (s.payload->>'step') AS step_name, \
            CASE \
              WHEN c.created_at IS NULL THEN 'running' \
              WHEN (c.payload->>'cached')::boolean IS TRUE THEN 'cached' \
              WHEN (c.payload->>'success')::boolean IS TRUE THEN 'succeeded' \
              ELSE 'failed' \
            END AS status, \
            s.created_at AS started_at, \
            c.created_at AS completed_at, \
            CASE WHEN c.created_at IS NULL THEN NULL \
                 ELSE (EXTRACT(EPOCH FROM (c.created_at - s.created_at)) * 1000)::bigint \
            END AS duration_ms, \
            c.payload->>'error' AS error, \
            COALESCE((c.payload->>'cached')::boolean, FALSE) AS cached \
         FROM agentic_run_events s \
         LEFT JOIN agentic_run_events c \
           ON c.run_id = s.run_id \
          AND c.event_type = 'subrun_step_completed' \
          AND c.payload->>'step' = s.payload->>'step' \
          AND c.seq > s.seq \
         WHERE s.run_id = $1 \
           AND s.event_type = 'subrun_step_started' \
         ORDER BY s.seq, c.seq ASC",
        [run_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows)
}

/// Per-table summary of an airway (ELT) run — rows in / rows out plus
/// the phase timestamps that let the dashboard spot a slow extract or a
/// slow load. Reconstructed at query time from the `extract_*` /
/// `table_loaded` events the airway worker already forwards; no
/// instrumentation work needed, no extension table.
#[derive(Debug, Clone, FromQueryResult, serde::Serialize)]
pub struct AirwayTableSummary {
    pub table_name: String,
    /// Final extracted row count from the source connector.
    pub rows_extracted: Option<i64>,
    /// Final loaded row count on the destination.
    pub rows_loaded: Option<i64>,
    /// `extracting` | `extracted` | `loading` | `loaded` | `failed` —
    /// derived from which event types have fired so far.
    pub status: String,
    pub extract_started_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    pub extract_completed_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    pub loaded_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

/// One row per table touched by the run. Walks the per-table airway
/// events for the given run id and rolls them up — the same data the
/// existing `/pages/airway/` UI consumes, but aggregated server-side so
/// the coordinator's polymorphic run-detail can render it without
/// re-implementing the airway reducer.
pub async fn airway_table_summary_for_run(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Vec<AirwayTableSummary>, DbErr> {
    let rows = AirwayTableSummary::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // `MIN(seq)` ordering preserves the natural extract order rather
        // than alphabetical, which makes the card read like the timeline
        // it summarises. The status enum collapses presence of later-
        // stage events back to a single string for the UI.
        "SELECT \
            payload->>'table' AS table_name, \
            MAX(CASE WHEN event_type = 'extract_completed' \
                     THEN (payload->>'rows_extracted')::bigint END) AS rows_extracted, \
            MAX(CASE WHEN event_type = 'table_loaded' \
                     THEN (payload->>'rows')::bigint END) AS rows_loaded, \
            CASE \
              WHEN COUNT(*) FILTER (WHERE event_type = 'resource_failed') > 0 THEN 'failed' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'table_loaded') > 0 THEN 'loaded' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'table_load_started') > 0 THEN 'loading' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'extract_completed') > 0 THEN 'extracted' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'extract_started') > 0 THEN 'extracting' \
              ELSE 'pending' \
            END AS status, \
            MIN(CASE WHEN event_type = 'extract_started' \
                     THEN created_at END) AS extract_started_at, \
            MAX(CASE WHEN event_type = 'extract_completed' \
                     THEN created_at END) AS extract_completed_at, \
            MAX(CASE WHEN event_type = 'table_loaded' \
                     THEN created_at END) AS loaded_at \
         FROM agentic_run_events \
         WHERE run_id = $1 \
           AND payload->>'table' IS NOT NULL \
           AND event_type IN ( \
             'extract_started', 'extract_completed', \
             'table_load_started', 'table_loaded', \
             'resource_failed' \
           ) \
         GROUP BY payload->>'table' \
         ORDER BY MIN(seq)",
        [run_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows)
}

/// List root runs that are currently active (not in a terminal state).
/// Used by the coordinator dashboard to show in-flight pipelines.
/// **Workspace-scoped** so the live feed never crosses tenants.
/// `include_system` lets background-daemon runs through; default-off so
/// the live feed stays focused on user work. A preview dry run and a check
/// run queued outside production are never listed (see
/// `visibility::in_customer_feed`).
pub async fn list_active_runs(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    include_system: bool,
) -> Result<Vec<run::Model>, DbErr> {
    let mut query = run::Entity::find()
        .filter(run::Column::WorkspaceId.eq(workspace_id))
        .filter(run::Column::ParentRunId.is_null())
        .filter(in_customer_feed())
        .filter(
            Condition::any()
                .add(run::Column::TaskStatus.eq("running"))
                .add(run::Column::TaskStatus.eq("delegating"))
                .add(run::Column::TaskStatus.eq("awaiting_input"))
                .add(run::Column::TaskStatus.eq("waiting_on_child"))
                .add(run::Column::TaskStatus.eq("waiting_on_children"))
                .add(run::Column::TaskStatus.eq("needs_resume"))
                .add(run::Column::TaskStatus.eq("shutdown")),
        );
    if !include_system {
        query =
            query.filter(run::Column::SourceType.is_not_in(SYSTEM_SOURCE_TYPES.iter().copied()));
    }
    query.order_by_desc(run::Column::UpdatedAt).all(db).await
}

async fn get_last_run_event(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<run_event::Model>, DbErr> {
    run_event::Entity::find()
        .filter(run_event::Column::RunId.eq(run_id))
        .order_by_desc(run_event::Column::Seq)
        .one(db)
        .await
}

fn terminal_error_message(
    event: Option<&run_event::Model>,
    fallback: Option<&str>,
) -> Option<String> {
    event
        .and_then(|row| row.payload["message"].as_str())
        .map(ToOwned::to_owned)
        .or_else(|| fallback.map(ToOwned::to_owned))
}

/// Compute the effective run state from an already-fetched last event, avoiding a DB round-trip.
fn effective_run_state_from_last_event(
    run: &run::Model,
    last_event: Option<&run_event::Model>,
) -> (String, Option<String>) {
    if run.answer.is_some() {
        return ("done".to_string(), None);
    }
    match last_event.map(|e| e.event_type.as_str()) {
        Some("done") => ("done".to_string(), None),
        Some("error") => (
            "failed".to_string(),
            terminal_error_message(last_event, run.error_message.as_deref()),
        ),
        _ => (
            user_facing_status(run.task_status.as_deref()).to_string(),
            run.error_message.clone(),
        ),
    }
}

pub async fn get_effective_run_state(
    db: &DatabaseConnection,
    run: &run::Model,
) -> Result<(String, Option<String>), DbErr> {
    if run.answer.is_some() {
        return Ok(("done".to_string(), None));
    }
    let last_event = get_last_run_event(db, &run.id).await?;
    Ok(effective_run_state_from_last_event(
        run,
        last_event.as_ref(),
    ))
}

pub async fn get_thread_history(
    db: &DatabaseConnection,
    thread_id: Uuid,
    limit: u64,
) -> Result<Vec<ThreadHistoryTurn>, DbErr> {
    use sea_orm::QuerySelect;
    let models = run::Entity::find()
        .filter(run::Column::ThreadId.eq(thread_id))
        .filter(run::Column::TaskStatus.is_in(["done", "failed", "cancelled", "timed_out"]))
        .order_by_asc(run::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await?;
    Ok(models
        .into_iter()
        .filter_map(|m| {
            let answer = turn_answer(m.task_status.as_deref(), &m.answer, &m.error_message)?;
            Some(ThreadHistoryTurn {
                question: m.question,
                answer,
                metadata: m.metadata,
            })
        })
        .collect())
}

/// Render the conversation-history answer text for a terminal run.
///
/// Returns `None` only when the status is non-terminal or has no useful
/// content to surface. `cancelled` and `timed_out` runs without an explicit
/// answer/error_message still yield a synthetic marker so follow-up turns
/// know the prior run did not complete.
fn turn_answer(
    task_status: Option<&str>,
    answer: &Option<String>,
    error_message: &Option<String>,
) -> Option<String> {
    if let Some(ans) = answer {
        return Some(ans.clone());
    }
    match task_status {
        Some("failed") | Some("timed_out") => Some(format!(
            "Error: {}",
            error_message.as_deref().unwrap_or("run failed")
        )),
        Some("cancelled") => Some(
            error_message
                .as_deref()
                .map(|m| format!("Cancelled: {m}"))
                .unwrap_or_else(|| "Cancelled by user".to_string()),
        ),
        Some("done") => error_message.as_deref().map(|e| format!("Error: {e}")),
        _ => None,
    }
}

pub async fn get_thread_history_with_events(
    db: &DatabaseConnection,
    thread_id: Uuid,
    limit: u64,
) -> Result<Vec<(String, String, Vec<ToolExchangeRow>)>, DbErr> {
    use sea_orm::QuerySelect;
    use std::collections::HashMap;

    let runs = run::Entity::find()
        .filter(run::Column::ThreadId.eq(thread_id))
        .filter(run::Column::TaskStatus.is_in(["done", "failed", "cancelled", "timed_out"]))
        .order_by_asc(run::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await?;

    if runs.is_empty() {
        return Ok(vec![]);
    }

    // Single batch query for all events across every run in this page.
    let run_ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
    let all_events = run_event::Entity::find()
        .filter(run_event::Column::RunId.is_in(run_ids))
        .order_by_asc(run_event::Column::RunId)
        .order_by_asc(run_event::Column::Seq)
        .all(db)
        .await?;

    // Group events by run_id, preserving ascending seq order from the query.
    let mut events_by_run: HashMap<String, Vec<run_event::Model>> = HashMap::new();
    for event in all_events {
        events_by_run
            .entry(event.run_id.clone())
            .or_default()
            .push(event);
    }

    let mut result = Vec::new();
    for r in runs {
        let run_events = events_by_run.remove(&r.id).unwrap_or_default();
        // Derive state from the in-memory last event — no extra DB round-trip.
        let last_event = run_events.last();
        let (status, error_message) = effective_run_state_from_last_event(&r, last_event);

        let answer = match (
            status.as_str(),
            r.answer.as_deref(),
            error_message.as_deref(),
        ) {
            ("done", Some(ans), _) => ans.to_string(),
            ("done", None, Some(error)) => format!("Error: {}", error),
            ("failed", _, Some(error)) => format!("Error: {}", error),
            ("failed", _, None) => "Error: run failed".to_string(),
            ("cancelled", _, Some(msg)) => format!("Cancelled: {msg}"),
            ("cancelled", _, None) => "Cancelled by user".to_string(),
            _ => continue,
        };

        let mut exchanges: Vec<ToolExchangeRow> = Vec::new();
        let mut pending_call: Option<(String, String)> = None;
        for event in run_events {
            match event.event_type.as_str() {
                "tool_call" => {
                    let name = event.payload["name"].as_str().unwrap_or("").to_string();
                    let input = event.payload["input"].as_str().unwrap_or("{}").to_string();
                    pending_call = Some((name, input));
                }
                "tool_result" => {
                    if let Some((name, input)) = pending_call.take() {
                        let output = event.payload["output"].as_str().unwrap_or("").to_string();
                        exchanges.push(ToolExchangeRow {
                            name,
                            input,
                            output,
                        });
                    }
                }
                _ => {}
            }
        }

        result.push((r.question, answer, exchanges));
    }
    Ok(result)
}
