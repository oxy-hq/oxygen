//! A wall-clock ceiling on a running preview run.
//!
//! One preview run per workspace runs at a time, so a run that never goes
//! terminal — its platform unresolvable on every tick, or parked on
//! `awaiting_input` with nobody to answer — would hold the workspace's queue
//! forever. The sweep retires a run that has been `running` longer than
//! `OXY_PREVIEW_RUN_MAX_MINUTES` (default 60): its `agentic_runs` row goes
//! `failed` with the reason (what the run detail shows as `error`), and the
//! registry row is finished so the queue moves on.
//!
//! **An Airway sample is waited for.** It writes preview-owned state under a
//! lease (`preview:<key>:<name>`), and a lease whose run went terminal is free
//! for the taking — so retiring a sample whose engine is still loading would
//! let the next sample of that pipeline run beside it. A sample past the
//! ceiling that still holds a live lease is only asked to cancel, and retired
//! once its engine has let the lease go, or [`WIND_DOWN_MINUTES`] later at
//! the most (a worker that died holding it). Samples cut themselves off before
//! the ceiling (`previews::sample::deadline`), so this is the backstop.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

pub const MAX_MINUTES_ENV: &str = "OXY_PREVIEW_RUN_MAX_MINUTES";
const DEFAULT_MAX_MINUTES: i64 = 60;

/// The ceiling in minutes: the env value when it is a positive integer.
pub fn max_minutes() -> i64 {
    std::env::var(MAX_MINUTES_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&m| m > 0)
        .unwrap_or(DEFAULT_MAX_MINUTES)
}

/// How long past the ceiling a sample still holding its lease is left to wind
/// down before it is retired anyway.
pub const WIND_DOWN_MINUTES: i64 = 15;

const OVERDUE_SQL: &str = "\
    SELECT p.run_id, \
      (p.kind = 'airway_sample' \
       AND p.started_at >= now() - make_interval(mins => $1 + $2) \
       AND EXISTS (SELECT 1 FROM airway_pipeline_leases l \
                   WHERE l.run_id = p.run_id AND l.expires_at > now())) AS winding_down \
    FROM workspace_preview_runs p \
    WHERE p.state = 'running' AND p.kind IN ('procedure','transform_build','airway_sample') \
      AND p.started_at < now() - make_interval(mins => $1)";

/// Retire and finish every run past the ceiling. The run ids retired.
pub async fn retire_overdue(
    db: &DatabaseConnection,
    max_minutes: i64,
) -> Result<Vec<String>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            OVERDUE_SQL,
            [
                (max_minutes as i32).into(),
                (WIND_DOWN_MINUTES as i32).into(),
            ],
        ))
        .await?;
    let mut retired = Vec::new();
    for row in rows {
        let run_id: String = row.try_get("", "run_id")?;
        if row.try_get::<bool>("", "winding_down")? {
            // Ask again each tick; retire once the lease is gone.
            if let Err(e) = agentic_runtime::crud::request_cancel(db, &run_id).await {
                tracing::debug!(target: "preview", %run_id, error = %e, "cancel request not recorded");
            }
            tracing::info!(target: "preview", %run_id,
                "an overdue Airway sample still holds its lease; waiting for it to wind down");
            continue;
        }
        let reason = format!("the preview run exceeded its {max_minutes}-minute ceiling");
        // A live driver notices the cancel and winds down; the retire makes
        // the run terminal whether or not anything is driving it.
        //
        // **Not strict serialization.** A run that is slow but still driven
        // goes terminal here while its in-flight task finishes, and the
        // finish below lets the workspace's next run start — so the two can
        // briefly overlap. Safe for procedure runs and builds, whose writes
        // are held or land through per-step reviews; an Airway sample, which
        // loads under a lease, is waited for above until that lease drops.
        if let Err(e) = agentic_runtime::crud::request_cancel(db, &run_id).await {
            tracing::debug!(target: "preview", %run_id, error = %e, "cancel request not recorded");
        }
        if let Err(e) = agentic_runtime::crud::retire_run(db, &run_id, &reason).await {
            tracing::warn!(target: "preview", %run_id, error = %e,
                "could not retire an overdue preview run; next tick retries");
            continue;
        }
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE workspace_preview_runs SET state = 'finished', finished_at = now() \
             WHERE run_id = $1 AND state = 'running'",
            [run_id.clone().into()],
        ))
        .await?;
        tracing::warn!(target: "preview", %run_id, max_minutes, "retired an overdue preview run");
        retired.push(run_id);
    }
    Ok(retired)
}
