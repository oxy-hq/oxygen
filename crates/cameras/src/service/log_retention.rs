//! Periodic sweep of `oxy_cam_device_logs` (IoT Phase 6a).
//!
//! Phase 4b shipped log streaming with **no** retention, on the
//! grounds that we'd rather get the operator-visible signal in
//! first and let the rows pile up until DuckLake quota tells us
//! to add a sweeper. This is that sweeper.
//!
//! ## Policy
//!
//! Two windows, matching what the IoT migration doc spec'd:
//!
//!   - debug / info: 7 days
//!   - warn / warning / error: 30 days
//!
//! Both are env-tunable (`OXY_CAMERA_LOG_RETENTION_INFO_DAYS` /
//! `..._WARN_DAYS`). Customers on bigger DuckLake quotas can
//! lengthen the windows without code changes; constrained
//! deployments can shorten them.
//!
//! ## Where the sweep runs
//!
//! Two surfaces, same service code underneath:
//!
//!   - **In-process background loop** kicked from `oxy serve`.
//!     Default interval 6 hours; set to 0 to disable.
//!   - **CLI subcommand** (`oxy cameras sweep-logs`) for manual
//!     runs and cron-based deployments that want exact timing.
//!
//! ## Failure semantics
//!
//! Per-workspace sweeps never short-circuit the loop. A
//! workspace whose Airhouse tenant is unreachable logs a warning
//! and the iteration continues to the next workspace. The
//! "table doesn't exist" path is treated as zero-rows-deleted.
//!
//! A workspace without an Airhouse tenant at all (e.g. the
//! deployment hasn't configured Airhouse) collapses to an empty
//! result via the `Disabled` variant — same defensive pattern
//! as the log reader.

use crate::entities::{edge_boxes, sites};
use crate::service::{ServiceError, ServiceResult};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sea_orm::{DatabaseConnection, EntityTrait, QuerySelect};
use serde::Serialize;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Default retention windows per the IoT spec. Override via env.
pub const DEFAULT_INFO_DAYS: i64 = 7;
pub const DEFAULT_WARN_DAYS: i64 = 30;

/// Default in-process loop cadence. 6 hours is rare enough not
/// to load the system, frequent enough that a single missed tick
/// (process restart) doesn't matter.
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Per-workspace sweep result — surfaces in both the CLI output
/// and the loop's tracing line.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SweepStats {
    /// Rows deleted from the info/debug bucket.
    pub info_deleted: u64,
    /// Rows deleted from the warn/warning/error bucket.
    pub warn_deleted: u64,
}

impl SweepStats {
    pub fn total(&self) -> u64 {
        self.info_deleted + self.warn_deleted
    }
}

/// Retention windows. Built from env or defaults via [`from_env`].
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub info_days: i64,
    pub warn_days: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            info_days: DEFAULT_INFO_DAYS,
            warn_days: DEFAULT_WARN_DAYS,
        }
    }
}

impl Policy {
    /// Parse from env, defaulting on absence or invalid input.
    /// A non-numeric value logs a warning and falls back to the
    /// default — we'd rather sweep on the spec window than 503
    /// the loop on a bad env.
    pub fn from_env() -> Self {
        let info_days = parse_env_days("OXY_CAMERA_LOG_RETENTION_INFO_DAYS", DEFAULT_INFO_DAYS);
        let warn_days = parse_env_days("OXY_CAMERA_LOG_RETENTION_WARN_DAYS", DEFAULT_WARN_DAYS);
        Self {
            info_days,
            warn_days,
        }
    }
}

fn parse_env_days(name: &str, default: i64) -> i64 {
    match std::env::var(name) {
        Ok(v) => v.trim().parse::<i64>().unwrap_or_else(|_| {
            warn!(
                env = name,
                value = v,
                default,
                "log_retention: invalid env value, using default"
            );
            default
        }),
        Err(_) => default,
    }
}

/// DELETE old rows for one workspace.
///
/// Two DELETEs (one per bucket) because the cutoff is different;
/// running them in sequence keeps the SQL flat and lets us
/// report per-bucket counts. `received_at` is the source of
/// truth — `ts` is what the device claims and can be skewed if
/// its clock is off.
pub async fn sweep_workspace(workspace_id: Uuid, policy: Policy) -> ServiceResult<SweepStats> {
    let now = Utc::now();
    let info_cutoff = now - ChronoDuration::days(policy.info_days);
    let warn_cutoff = now - ChronoDuration::days(policy.warn_days);

    // Dedicated one-shot connection (not the persistent ingest one) so a large
    // retention DELETE can't head-of-line-block live ingest for this tenant.
    let client = match crate::airhouse::connect_for_retention(workspace_id).await {
        Ok(c) => c,
        Err(crate::airhouse::AirhouseError::Disabled) => {
            tracing::debug!(
                workspace_id = %workspace_id,
                "log_retention.sweep_workspace: airhouse disabled, skipping"
            );
            return Ok(SweepStats::default());
        }
        Err(e) => return Err(ServiceError::Airhouse(e)),
    };

    let info_deleted = run_delete(
        &client,
        workspace_id,
        "debug/info",
        &info_bucket_sql(info_cutoff),
    )
    .await?;
    let warn_deleted = run_delete(
        &client,
        workspace_id,
        "warn/error",
        &warn_bucket_sql(warn_cutoff),
    )
    .await?;

    Ok(SweepStats {
        info_deleted,
        warn_deleted,
    })
}

/// Iterate every workspace that owns at least one edge_box and
/// sweep each one. A failing workspace logs a warning and
/// iteration continues — partial completion is strictly better
/// than aborting after the first error.
///
/// Returns the aggregate stats across all workspaces that
/// completed cleanly. The per-workspace breakdown is in the log
/// stream rather than the return value because the loop caller
/// (background or CLI) just needs a top-level number for its
/// operator-facing line.
pub async fn sweep_all(db: &DatabaseConnection, policy: Policy) -> ServiceResult<SweepStats> {
    let workspace_ids = workspaces_with_edge_boxes(db).await?;
    if workspace_ids.is_empty() {
        return Ok(SweepStats::default());
    }
    let mut agg = SweepStats::default();
    for wid in workspace_ids {
        match sweep_workspace(wid, policy).await {
            Ok(s) => {
                if s.total() > 0 {
                    info!(
                        workspace_id = %wid,
                        info_deleted = s.info_deleted,
                        warn_deleted = s.warn_deleted,
                        "log_retention: swept"
                    );
                }
                agg.info_deleted += s.info_deleted;
                agg.warn_deleted += s.warn_deleted;
            }
            Err(e) => {
                warn!(
                    workspace_id = %wid,
                    error = %e,
                    "log_retention: workspace sweep failed; continuing"
                );
            }
        }
    }
    Ok(agg)
}

/// Spawn the in-process background loop. Honors:
///
///   - `OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS` — tick cadence; `0`
///     disables the loop (operators using the CLI on cron want
///     this).
///   - `OXY_CAMERA_LOG_RETENTION_INFO_DAYS` and
///     `..._WARN_DAYS` — per-bucket windows.
///
/// Exits cleanly on `shutdown` cancellation, same contract as
/// [`crate::service::stale::spawn`].
pub fn spawn(db: DatabaseConnection, shutdown: CancellationToken) {
    let interval = parse_env_hours(
        "OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS",
        DEFAULT_SWEEP_INTERVAL,
    );
    if interval.is_zero() {
        info!("log_retention: in-process loop disabled (interval=0)");
        return;
    }
    let policy = Policy::from_env();
    tokio::spawn(async move {
        info!(
            interval_secs = interval.as_secs(),
            info_days = policy.info_days,
            warn_days = policy.warn_days,
            "log_retention: started"
        );
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate tick — fresh boots shouldn't sweep
        // before any logs have had a chance to flow.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("log_retention: shutdown");
                    return;
                }
                _ = ticker.tick() => {
                    match sweep_all(&db, policy).await {
                        Ok(s) if s.total() > 0 => info!(
                            info_deleted = s.info_deleted,
                            warn_deleted = s.warn_deleted,
                            "log_retention: tick complete"
                        ),
                        Ok(_) => tracing::debug!("log_retention: tick complete (no rows)"),
                        Err(e) => warn!(error = %e, "log_retention: tick failed"),
                    }
                }
            }
        }
    });
}

// ── Internals ───────────────────────────────────────────────────────────────

fn info_bucket_sql(cutoff: DateTime<Utc>) -> String {
    format!(
        "DELETE FROM oxy_cam_device_logs \
         WHERE severity IN ('debug', 'info') AND received_at < {}",
        crate::airhouse::escape::sql_ts(cutoff),
    )
}

fn warn_bucket_sql(cutoff: DateTime<Utc>) -> String {
    format!(
        "DELETE FROM oxy_cam_device_logs \
         WHERE severity IN ('warn', 'warning', 'error') AND received_at < {}",
        crate::airhouse::escape::sql_ts(cutoff),
    )
}

async fn run_delete(
    client: &tokio_postgres::Client,
    workspace_id: Uuid,
    bucket: &str,
    sql: &str,
) -> ServiceResult<u64> {
    let msgs = match client.simple_query(sql).await {
        Ok(m) => m,
        Err(e) => {
            let s = crate::airhouse::pg_error_text(&e);
            if crate::airhouse::is_missing_table(&s, "oxy_cam_device_logs") {
                // Fresh workspace, no device has ever shipped a log —
                // nothing to retain, nothing to delete.
                tracing::debug!(
                    workspace_id = %workspace_id,
                    bucket,
                    "log_retention: table absent, treating as zero rows"
                );
                return Ok(0);
            }
            return Err(ServiceError::Airhouse(
                crate::airhouse::AirhouseError::Insert(format!("DELETE {bucket}: {s}")),
            ));
        }
    };
    // DuckLake/pgwire reports `CommandComplete("DELETE n")` for a
    // successful DELETE. Parse the count out of the tag; if the
    // shape changes (newer wire protocol versions, etc.) we
    // safely fall back to 0 rather than crashing the sweep.
    for m in msgs {
        if let tokio_postgres::SimpleQueryMessage::CommandComplete(rows) = m {
            return Ok(rows);
        }
    }
    Ok(0)
}

async fn workspaces_with_edge_boxes(db: &DatabaseConnection) -> ServiceResult<Vec<Uuid>> {
    let ids: Vec<Uuid> = edge_boxes::Entity::find()
        .inner_join(sites::Entity)
        .select_only()
        .column(sites::Column::WorkspaceId)
        .distinct()
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    Ok(ids)
}

fn parse_env_hours(name: &str, default: Duration) -> Duration {
    match std::env::var(name) {
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(h) => Duration::from_secs(h.saturating_mul(60 * 60)),
            Err(_) => {
                warn!(
                    env = name,
                    value = v,
                    default_secs = default.as_secs(),
                    "log_retention: invalid env value, using default"
                );
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[serial]
    #[test]
    fn policy_from_env_uses_defaults_when_unset() {
        // SAFETY: tests run single-threaded under nextest's default.
        // Removing the env vars in case a previous test left them.
        unsafe {
            std::env::remove_var("OXY_CAMERA_LOG_RETENTION_INFO_DAYS");
            std::env::remove_var("OXY_CAMERA_LOG_RETENTION_WARN_DAYS");
        }
        let p = Policy::from_env();
        assert_eq!(p.info_days, DEFAULT_INFO_DAYS);
        assert_eq!(p.warn_days, DEFAULT_WARN_DAYS);
    }

    #[serial]
    #[test]
    fn policy_from_env_parses_overrides() {
        // SAFETY: see above.
        unsafe {
            std::env::set_var("OXY_CAMERA_LOG_RETENTION_INFO_DAYS", "3");
            std::env::set_var("OXY_CAMERA_LOG_RETENTION_WARN_DAYS", "14");
        }
        let p = Policy::from_env();
        assert_eq!(p.info_days, 3);
        assert_eq!(p.warn_days, 14);
        unsafe {
            std::env::remove_var("OXY_CAMERA_LOG_RETENTION_INFO_DAYS");
            std::env::remove_var("OXY_CAMERA_LOG_RETENTION_WARN_DAYS");
        }
    }

    #[test]
    fn info_bucket_sql_includes_both_severities() {
        let cutoff = Utc::now() - ChronoDuration::days(7);
        let sql = info_bucket_sql(cutoff);
        assert!(sql.contains("'debug'"));
        assert!(sql.contains("'info'"));
        assert!(!sql.contains("'warn'"));
        assert!(sql.contains("DELETE FROM oxy_cam_device_logs"));
    }

    #[test]
    fn warn_bucket_sql_includes_all_three_severities() {
        let cutoff = Utc::now() - ChronoDuration::days(30);
        let sql = warn_bucket_sql(cutoff);
        // Both spellings of "warn" the worker may send.
        assert!(sql.contains("'warn'"));
        assert!(sql.contains("'warning'"));
        assert!(sql.contains("'error'"));
        assert!(!sql.contains("'info'"));
    }

    #[test]
    fn sweep_stats_total_sums_buckets() {
        let s = SweepStats {
            info_deleted: 5,
            warn_deleted: 3,
        };
        assert_eq!(s.total(), 8);
    }
}
