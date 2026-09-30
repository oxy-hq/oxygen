//! Periodic maintenance for the compile boundary's `revisions` table.
//!
//! Two jobs, both pure DB sweeps (no working copy needed, so this is safe to
//! run on any node — serve or worker — and is idempotent across replicas):
//!
//!   1. **Reaper** — a compile that crashed / OOM'd / was SIGKILL'd mid-run
//!      leaves its `revisions` row stuck at `status = 'compiling'` forever.
//!      The task queue re-delivers the *work*, but nothing reconciles the
//!      orphaned row, so without this sweep every crash-during-compile leaks
//!      a permanent `compiling` ghost. Mark rows stuck past a timeout
//!      `failed` so the timeline is accurate and "is a compile in flight?"
//!      checks stay trustworthy.
//!
//!   2. **Retention** — every compile inserts a new `revisions` row plus a
//!      full copy of every entity row; superseded revisions are otherwise
//!      never deleted, so the `*_definitions` tables grow without bound.
//!      Delete non-current revisions finished longer ago than the retention
//!      window (cascades to all `*_definitions` child rows via the schema's
//!      `on_delete = Cascade`). The current revision of every workspace and
//!      anything inside the window are kept — the window doubles as the
//!      rollback horizon for `/admin/compiles/{revision_id}/promote`.

use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

const STUCK_TIMEOUT_ENV: &str = "OXY_COMPILE_STUCK_TIMEOUT_SECS";
const RETENTION_DAYS_ENV: &str = "OXY_COMPILE_RETENTION_DAYS";
const INTERVAL_ENV: &str = "OXY_COMPILE_MAINTENANCE_INTERVAL_SECS";

const DEFAULT_STUCK_TIMEOUT_SECS: u64 = 900; // 15 min — well above the slowest expected compile.
const DEFAULT_RETENTION_DAYS: u64 = 30;
const DEFAULT_INTERVAL_SECS: u64 = 300; // 5 min
/// Cap per retention cycle so a large backlog can't hold a long lock; the next
/// tick continues where this one stopped.
const DELETE_BATCH: i64 = 1000;

#[derive(Clone, Copy, Debug)]
pub struct CompileMaintenanceConfig {
    pub interval: Duration,
    pub stuck_timeout_secs: u64,
    /// `0` disables retention (reaper still runs).
    pub retention_days: u64,
}

impl CompileMaintenanceConfig {
    pub fn from_env() -> Self {
        fn parse(key: &str, default: u64) -> u64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        Self {
            interval: Duration::from_secs(parse(INTERVAL_ENV, DEFAULT_INTERVAL_SECS).max(30)),
            stuck_timeout_secs: parse(STUCK_TIMEOUT_ENV, DEFAULT_STUCK_TIMEOUT_SECS).max(60),
            retention_days: parse(RETENTION_DAYS_ENV, DEFAULT_RETENTION_DAYS),
        }
    }
}

/// Spawn the detached maintenance loop. Both the reaper `UPDATE` and the
/// retention `DELETE` are guarded so concurrent runs on multiple replicas
/// converge without any coordination.
pub fn spawn_compile_maintenance(config: CompileMaintenanceConfig) {
    tokio::spawn(async move {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "compile_maintenance: DB connect failed; loop not started"
                );
                return;
            }
        };
        tracing::info!(
            interval_secs = config.interval.as_secs(),
            stuck_timeout_secs = config.stuck_timeout_secs,
            retention_days = config.retention_days,
            "compile_maintenance: started"
        );
        let mut tick = tokio::time::interval(config.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // drain the immediate first tick — skip the startup storm.
        loop {
            tick.tick().await;
            reap_stuck_compiles(&db, config.stuck_timeout_secs).await;
            if config.retention_days > 0 {
                prune_old_revisions(&db, config.retention_days).await;
            }
        }
    });
}

/// Mark `compiling` rows older than the timeout as `failed`. Uses DB `now()`
/// (not app-side time) so it's robust to clock skew between app and DB.
async fn reap_stuck_compiles(db: &DatabaseConnection, timeout_secs: u64) {
    let sql = "UPDATE revisions \
               SET status = 'failed', finished_at = now(), \
                   error_summary = jsonb_build_object('fatal', \
                     'compile abandoned (worker died or exceeded timeout); reaped by maintenance') \
               WHERE status = 'compiling' \
                 AND started_at < now() - ($1::bigint * interval '1 second')";
    match db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [(timeout_secs as i64).into()],
        ))
        .await
    {
        Ok(r) if r.rows_affected() > 0 => tracing::warn!(
            reaped = r.rows_affected(),
            "compile_maintenance: reaped stuck 'compiling' revisions"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(?e, "compile_maintenance: reaper sweep failed"),
    }
}

/// Delete a bounded batch of non-current revisions finished older than the
/// window. Never touches any workspace's `current_revision_id`; the FK cascade
/// removes the `*_definitions` child rows. Safe against an in-flight reader:
/// readers only ever resolve and read the *current* revision, which is excluded
/// here, and no request outlives the (days-long) retention window.
///
/// **A revision any `app_builds` row pins is kept too** — the staging pin
/// (`app_builds.semantic_revision_id`, see `custom_apps_staging_pin`). The
/// rule is "referenced by any build", not "by a draft pointer": it is the
/// simpler one, and still bounded, because builds are themselves GC'd
/// keep-last-N per app — the pin is released when its build is.
pub async fn prune_old_revisions(db: &DatabaseConnection, retention_days: u64) {
    let secs = retention_days as i64 * 86_400;
    let sql = "DELETE FROM revisions \
               WHERE revision_id IN ( \
                   SELECT r.revision_id FROM revisions r \
                   WHERE r.finished_at IS NOT NULL \
                     AND r.finished_at < now() - ($1::bigint * interval '1 second') \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM workspaces w \
                         WHERE w.current_revision_id = r.revision_id \
                     ) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM app_builds b \
                         WHERE b.semantic_revision_id = r.revision_id \
                     ) \
                   LIMIT $2 \
               )";
    match db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [secs.into(), DELETE_BATCH.into()],
        ))
        .await
    {
        Ok(r) if r.rows_affected() > 0 => tracing::info!(
            pruned = r.rows_affected(),
            "compile_maintenance: pruned old non-current revisions"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(?e, "compile_maintenance: retention sweep failed"),
    }
}
