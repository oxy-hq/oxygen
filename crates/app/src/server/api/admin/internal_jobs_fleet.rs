//! `/admin/internal-jobs/workers`, `/scheduled`, `/run-reaper`, `/run-retention` —
//! the worker fleet, the registry of periodic system jobs, and the two fleet-wide
//! actions an operator can run on demand.
//!
//! Split out of `internal_jobs.rs` by responsibility; the routes are mounted there.

use axum::Json;
use axum::response::Response;
use chrono::{DateTime, FixedOffset};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde::Serialize;

use super::internal_jobs::{connect, db_err};
use super::internal_jobs_reach::{deny_out_of_scope_fleet, listing_scope, run_scope_clause};

// Worker fleet

#[derive(Serialize, Debug)]
pub struct WorkerDto {
    pub worker_id: String,
    pub last_claim_at: Option<DateTime<FixedOffset>>,
    pub inflight_count: i64,
}

#[derive(Serialize, Debug)]
pub struct WorkersResponse {
    pub supported: bool,
    pub workers: Vec<WorkerDto>,
}

#[derive(Debug, FromQueryResult)]
struct WorkerRow {
    worker_id: String,
    last_claim_at: Option<DateTime<FixedOffset>>,
    inflight_count: i64,
}

pub async fn list_workers(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<WorkersResponse>, Response> {
    let db = connect().await?;
    let scope = listing_scope(&db, &actor).await?;
    // Aggregate by worker_id over rows the worker has touched in the last
    // 24h or that are still claimed. Inflight = currently `claimed` by them.
    // A bounded grant sees the workers that touched ITS orgs' tasks, counted
    // over those tasks only — not the fleet's whole in-flight load.
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let in_scope = run_scope_clause("run_id", scope.as_deref(), &mut values);
    let rows = WorkerRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT \
                 worker_id, \
                 MAX(claimed_at) AS last_claim_at, \
                 COUNT(*) FILTER (WHERE queue_status = 'claimed') AS inflight_count \
             FROM agentic_task_queue \
             WHERE worker_id IS NOT NULL \
               AND (queue_status = 'claimed' OR claimed_at > now() - interval '24 hours')\
               {in_scope} \
             GROUP BY worker_id \
             ORDER BY MAX(claimed_at) DESC NULLS LAST"
        ),
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    Ok(Json(WorkersResponse {
        supported: true,
        workers: rows
            .into_iter()
            .map(|r| WorkerDto {
                worker_id: r.worker_id,
                last_claim_at: r.last_claim_at,
                inflight_count: r.inflight_count,
            })
            .collect(),
    }))
}

// Scheduled jobs registry (static for now)

#[derive(Serialize, Debug)]
pub struct ScheduledJobDto {
    pub name: &'static str,
    pub interval_secs: u64,
    pub description: &'static str,
    pub last_known_run_at: Option<DateTime<FixedOffset>>,
    pub trigger_path: Option<&'static str>,
}

pub(super) async fn list_scheduled() -> Json<Vec<ScheduledJobDto>> {
    Json(scheduled_jobs())
}

/// Static registry — wiring real `last_known_run_at` is future work; for
/// now this lets the UI surface the periodic loops that exist in the
/// runtime so on-call knows what's supposed to be running.
pub(crate) fn scheduled_jobs() -> Vec<ScheduledJobDto> {
    vec![
        ScheduledJobDto {
            name: "reaper",
            interval_secs: 30,
            description: "Resets stale claims so dead workers' work can be re-claimed (or dead-lettered when claim_count >= max_claims).",
            last_known_run_at: None,
            trigger_path: Some("/api/admin/internal-jobs/run-reaper"),
        },
        ScheduledJobDto {
            name: "matcher_health_probe",
            interval_secs: 60,
            description: "Self-NOTIFYs on the task router's health channel so listeners can observe LISTEN/NOTIFY stalls.",
            last_known_run_at: None,
            trigger_path: None,
        },
        ScheduledJobDto {
            name: "worker_recovery_loop",
            interval_secs: 30,
            description: "Pre-pass reaper tick from the standalone `oxy worker` process (in addition to the in-server reaper).",
            last_known_run_at: None,
            trigger_path: None,
        },
        ScheduledJobDto {
            name: "task_queue_retention",
            // Reflect the live (env-tunable) sweep cadence so the operator sees
            // the real interval, not a hardcoded guess.
            interval_secs: agentic_runtime::background::RetentionConfig::from_env()
                .interval
                .as_secs(),
            description: "Prunes old terminal task-queue rows (completed/cancelled after OXY_TASK_QUEUE_RETENTION_DAYS, failed/dead after OXY_TASK_QUEUE_DEAD_RETENTION_DAYS) so internal-job history doesn't clutter the DB.",
            last_known_run_at: None,
            trigger_path: Some("/api/admin/internal-jobs/run-retention"),
        },
    ]
}

// Run reaper now

#[derive(Serialize, Debug)]
pub struct RunReaperResponse {
    pub rows_affected: u64,
}

pub async fn run_reaper(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<RunReaperResponse>, Response> {
    let db = connect().await?;
    deny_out_of_scope_fleet(&db, &actor).await?;
    let transport = agentic_runtime::transport::DurableTransport::new(db);
    let rows_affected = transport.run_reaper().await.total();
    Ok(Json(RunReaperResponse { rows_affected }))
}

// Run retention prune now

#[derive(Serialize, Debug)]
pub struct RunRetentionResponse {
    pub rows_deleted: u64,
}

/// Manually sweep old terminal task-queue rows using the same retention
/// windows the periodic loop uses (read from the environment). Lets an
/// operator reclaim space immediately instead of waiting for the next cycle.
pub async fn run_retention(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<RunRetentionResponse>, Response> {
    let db = connect().await?;
    deny_out_of_scope_fleet(&db, &actor).await?;
    let cfg = agentic_runtime::background::RetentionConfig::from_env();
    let transport = agentic_runtime::transport::DurableTransport::new(db);
    let rows_deleted = transport
        .run_retention(cfg.completed_ttl, cfg.dead_ttl)
        .await;
    Ok(Json(RunRetentionResponse { rows_deleted }))
}
