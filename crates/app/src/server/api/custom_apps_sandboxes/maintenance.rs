//! The sandbox maintenance loop: a boot-time loop, shaped like
//! `previews::maintenance`, that expires idle sandboxes and retries teardowns
//! that did not finish. It only writes rows and queues tasks — the teardown
//! itself is a `custom_app_sandbox_teardown` task ([`super::teardown`]) on the
//! worker fleet — so it runs on any node, with or without workers.
//!
//! Each pass ([`sweep`]):
//! 1. **Expire** up to [`SANDBOXES_PER_PASS`] active sandboxes whose last
//!    activity (`super::activity`) is more than [`super::idle_ttl`] ago: each
//!    is deleted exactly as a `DELETE` would (`ops::begin_delete`), with no
//!    actor and the reason `expired`.
//! 2. **End** the sandboxes of sandbox agent tokens that were revoked or
//!    expired more than a day ago ([`super::token_ended`]), reason
//!    `token_ended`.
//! 3. **Retry** up to [`SANDBOXES_PER_PASS`] sandboxes marked deleting more
//!    than [`stale_teardown`] ago: their teardown failed, or never got a
//!    worker. Deleting one again marks it now and queues a new run, so it is
//!    next retried a further [`stale_teardown`] later.
//!
//! **The list is a moment old when each sandbox's turn comes.** A publish
//! may have landed on an idle sandbox, or a stale teardown finished and its
//! name been created again. So each delete says why the sandbox was selected
//! (`ops::Expect`) and looks again under the sandbox's row lock: an expiry
//! only of one still idle, a retry only of the very row still stuck.
//!
//! **Safe across replicas.** A delete queues nothing for a sandbox whose
//! teardown is still queued or running (`ops::delete`), so two nodes sweeping
//! at once queue one teardown between them; and a teardown holds the
//! sandbox's lock for its whole run (`super::lock`).

use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, Statement};
use uuid::Uuid;

use super::ops::Expect;
use super::{SandboxError, TeardownReason, activity, ops};

pub const INTERVAL_ENV: &str = "OXY_APP_SANDBOX_MAINTENANCE_INTERVAL_SECS";
const DEFAULT_INTERVAL_SECS: u64 = 900;
const MIN_INTERVAL_SECS: u64 = 60;
/// Sandboxes expired, and teardowns retried, per pass; the next pass continues.
pub const SANDBOXES_PER_PASS: i64 = 50;

/// How long a sandbox may stay `deleting` before its teardown is queued again.
pub fn stale_teardown() -> chrono::Duration {
    chrono::Duration::hours(6)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaintenanceConfig {
    pub interval: Duration,
}

impl MaintenanceConfig {
    /// `OXY_APP_SANDBOX_MAINTENANCE_INTERVAL_SECS`, default 900, at least 60.
    pub fn from_env() -> Self {
        let secs = std::env::var(INTERVAL_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_SECS)
            .max(MIN_INTERVAL_SECS);
        Self {
            interval: Duration::from_secs(secs),
        }
    }
}

/// Spawn the detached loop. Pure DB work, so it runs on any node regardless
/// of `--no-workers`; the queued teardowns run wherever the worker fleet does.
pub fn spawn(config: MaintenanceConfig) {
    tokio::spawn(async move {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "sandbox_maintenance: DB connect failed; loop not started"
                );
                return;
            }
        };
        tracing::info!(
            interval_secs = config.interval.as_secs(),
            "sandbox_maintenance: started"
        );
        let mut tick = tokio::time::interval(config.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await; // skip the startup storm, as previews::maintenance does
        loop {
            tick.tick().await;
            match sweep(&db, Utc::now()).await {
                Ok(0) => {}
                Ok(queued) => tracing::info!(queued, "sandbox_maintenance: teardowns queued"),
                Err(e) => tracing::warn!(error = %e, "sandbox_maintenance: sweep failed"),
            }
        }
    });
}

/// One pass at `now`: expire idle sandboxes, end those whose token ended,
/// then retry stale teardowns.
/// Answers how many teardowns it queued. A sandbox that cannot be deleted is
/// logged and left for the next pass; only a failed read fails the pass.
pub async fn sweep(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<usize, DbErr> {
    let ttl = super::idle_ttl();
    let idle_since = Expect::IdleSince(now - ttl);
    let idle = activity::idle_sandboxes(db, now, ttl, SANDBOXES_PER_PASS)
        .await?
        .into_iter()
        .map(|(app_id, name)| (app_id, name, idle_since))
        .collect();
    let mut queued = delete_each(db, idle, TeardownReason::Expired).await;
    queued += super::token_ended::sweep(db, now).await?;
    let stale = stale_teardowns(db, now - stale_teardown(), SANDBOXES_PER_PASS).await?;
    queued += delete_each(db, stale, TeardownReason::Retried).await;
    Ok(queued)
}

/// One sandbox the pass selected, and what must still be true of it when the
/// pass reaches it — the list is a moment old by then.
type Selected = (Uuid, String, Expect);

/// Up to `limit` sandboxes marked deleting before `marked_before`,
/// longest-waiting first — each with the row it was, so a sandbox created
/// again under the name is not taken for it.
async fn stale_teardowns(
    db: &DatabaseConnection,
    marked_before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Selected>, DbErr> {
    use sea_orm::ConnectionTrait;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT app_id, name, created_at FROM app_environments \
              WHERE kind = 'dev' AND deleting_at < $1 \
              ORDER BY deleting_at LIMIT $2",
            [marked_before.into(), limit.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            let created_at: DateTime<Utc> = row.try_get("", "created_at")?;
            let expect = Expect::Stale {
                created_at,
                marked_before,
            };
            Ok((row.try_get("", "app_id")?, row.try_get("", "name")?, expect))
        })
        .collect()
}

/// `ops::delete_if` for each selected sandbox; how many teardowns were queued.
async fn delete_each(
    db: &DatabaseConnection,
    selected: Vec<Selected>,
    reason: TeardownReason,
) -> usize {
    let mut queued = 0;
    for (app_id, name, expect) in selected {
        match delete_one(db, app_id, &name, reason, expect).await {
            Ok(Some(deletion)) if deletion.queued => {
                tracing::info!(%app_id, environment = %name, run_id = %deletion.run_id,
                    reason = reason.as_str(), "sandbox_maintenance: teardown queued");
                queued += 1;
            }
            // Its teardown is already on its way — another replica's sweep, a
            // `DELETE`, or a run still waiting for a worker.
            Ok(Some(_)) => {}
            // It moved on since the pass selected it — published to, invoked,
            // or created again under the name: not this pass's to delete.
            Ok(None) => {
                tracing::debug!(%app_id, environment = %name, reason = reason.as_str(),
                    "sandbox_maintenance: no longer what the pass selected; left alone");
            }
            // Deleted, and torn down, between the read and now.
            Err(SandboxError::NotFound(_) | SandboxError::AppNotFound) => {}
            Err(e) => {
                tracing::warn!(%app_id, environment = %name, reason = reason.as_str(), error = %e,
                    "sandbox_maintenance: could not queue the teardown");
            }
        }
    }
    queued
}

async fn delete_one(
    db: &DatabaseConnection,
    app_id: Uuid,
    name: &str,
    reason: TeardownReason,
    expect: Expect,
) -> Result<Option<ops::Deletion>, SandboxError> {
    let environment =
        AppEnvironment::parse(name).ok_or_else(|| SandboxError::InvalidName(name.to_string()))?;
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| SandboxError::db("load the app", e))?
        .ok_or(SandboxError::AppNotFound)?;
    ops::delete_if(db, &app, &environment, None, reason, expect).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fifteen minutes by default; the override is whole seconds, never under
    /// a minute. nextest runs each test in its own process, so the env writes
    /// are its own.
    #[test]
    fn the_interval_is_fifteen_minutes_unless_overridden_and_never_under_one() {
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::remove_var(INTERVAL_ENV) };
        assert_eq!(
            MaintenanceConfig::from_env().interval,
            Duration::from_secs(900)
        );
        for (raw, secs) in [
            ("120", 120),
            (" 3600 ", 3600),
            ("5", 60),
            ("0", 60),
            ("soon", 900),
        ] {
            unsafe { std::env::set_var(INTERVAL_ENV, raw) };
            assert_eq!(
                MaintenanceConfig::from_env().interval,
                Duration::from_secs(secs),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_teardown_is_retried_after_six_hours() {
        assert_eq!(stale_teardown(), chrono::Duration::hours(6));
    }
}
