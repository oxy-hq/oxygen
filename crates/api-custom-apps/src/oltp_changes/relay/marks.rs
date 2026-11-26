//! The relay's marks and lease, in the platform database (OLTP change stream
//! design §5.3). Every instant is the platform database's `now()`, so a nudge
//! and a drain compare on one clock.
//!
//! One row per (org, environment) in `oltp_change_relays`:
//!
//! - `nudged_at`: when the org's store last logged a change ([`nudge`], or the
//!   weekly [`mark_all`]);
//! - `drain_started_at`: when the newest drain pass that left nothing behind
//!   began ([`drained`]);
//! - `pending_since`: since when work has been waiting; the stale alarm reads it;
//! - `failures`, `failed_at` and `retry_after`: the backoff after failed passes
//!   ([`failed`], cleared by [`succeeded`]);
//! - `stale_alarmed_at`: when the sweep last logged the mark stale
//!   ([`alarm_stale`]);
//! - `wake_at`: when a pass that left a store baseline backing off should run
//!   again ([`wake`]); cleared by a pass with no failure ([`succeeded`]), and
//!   kept past a failed pass's backoff ([`failed`]).
//!
//! A mark is **due** while a nudge is not covered by a drain that began a
//! margin after it, and not before the redrain interval has passed since that
//! drain began ([`due`]); nor, after a failed pass, before its `retry_after`,
//! unless nudged since the failure. It is due as well once its `wake_at` has
//! passed, with no change waiting ([`Due::waiting`]). The sweep reads only due
//! marks, so it never connects to a store with nothing to do.
//!
//! The lease is one row's `lease_holder`, `lease_epoch` and `lease_expires_at`:
//! [`acquire`] is one `INSERT … ON CONFLICT DO UPDATE … WHERE … RETURNING`, so
//! two relays never both hold it, and every acquisition bumps the epoch.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// A due mark: an (org, environment) whose store may have changes waiting,
/// or a baseline to retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub org_id: Uuid,
    pub environment: String,
    /// Since when the work has waited.
    pub pending_since: DateTime<Utc>,
    /// Whether changes may wait: due by a nudge, not only by its `wake_at`.
    /// Only such a mark can be stale.
    pub waiting: bool,
}

/// A held lease on (org, environment). The `epoch` fences a holder whose lease
/// expired and was taken by another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub org_id: Uuid,
    pub environment: String,
    pub holder: String,
    pub epoch: i64,
}

/// Record a change in `org_id`'s `environment` store. `nudged_at` never moves
/// back; a new or drained mark waits from now, a due one keeps the time its
/// wait began.
pub async fn nudge(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO oltp_change_relays AS r (org_id, environment, nudged_at, pending_since) \
         VALUES ($1, $2, now(), now()) \
         ON CONFLICT (org_id, environment) DO UPDATE \
            SET nudged_at = GREATEST(r.nudged_at, EXCLUDED.nudged_at), \
                pending_since = COALESCE(r.pending_since, EXCLUDED.pending_since)",
        [org_id.into(), environment.into()],
    ))
    .await
    .map(|_| ())
}

/// The marks the sweep should queue a relay pass for, oldest wait first. A
/// mark backed off after a failed pass ([`failed`]) is not due before its
/// `retry_after`, unless it was nudged since that failure: a fresh write
/// means the store's compute is awake anyway. A mark whose `wake_at` has
/// passed ([`wake`]) is due whatever else it says, but has no change waiting
/// for that alone ([`Due::waiting`]).
pub async fn due(
    db: &impl ConnectionTrait,
    margin: Duration,
    every: Duration,
) -> Result<Vec<Due>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT org_id, environment, pending_since, waiting FROM ( \
               SELECT org_id, environment, \
                      COALESCE(pending_since, nudged_at, wake_at) AS pending_since, \
                      COALESCE(nudged_at IS NOT NULL \
                        AND (drain_started_at IS NULL \
                             OR (drain_started_at < nudged_at + make_interval(secs => $1) \
                                 AND drain_started_at < now() - make_interval(secs => $2))) \
                        AND (retry_after IS NULL OR retry_after <= now() \
                             OR nudged_at > failed_at), false) AS waiting, \
                      COALESCE(wake_at <= now(), false) AS woken \
                 FROM oltp_change_relays) m \
              WHERE waiting OR woken \
              ORDER BY pending_since",
            [margin.as_secs_f64().into(), every.as_secs_f64().into()],
        ))
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Due {
                org_id: row.try_get("", "org_id")?,
                environment: row.try_get("", "environment")?,
                pending_since: row.try_get("", "pending_since")?,
                waiting: row.try_get("", "waiting")?,
            })
        })
        .collect()
}

/// A drain pass that began at `started` (the platform's clock) left nothing
/// waiting. `drain_started_at` never moves back. The mark stays due, waiting
/// from `started`, when a nudge came within `margin` after the drain began;
/// otherwise the wait is cleared.
pub async fn drained(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    started: DateTime<Utc>,
    margin: Duration,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays \
            SET drain_started_at = GREATEST(drain_started_at, $3), \
                pending_since = CASE \
                    WHEN GREATEST(drain_started_at, $3) < nudged_at + make_interval(secs => $4) \
                    THEN $3 ELSE NULL END \
          WHERE org_id = $1 AND environment = $2",
        [
            org_id.into(),
            environment.into(),
            started.into(),
            margin.as_secs_f64().into(),
        ],
    ))
    .await
    .map(|_| ())
}

/// A pass left a store baseline backing off for `after`: wake the mark then
/// (`wake_at`, by the platform's clock), whatever its drain or backoff say,
/// so the sweep retries the baseline though no write nudges the store. It
/// counts no failure and leaves the drain alone. A pass with no failure
/// clears it ([`succeeded`]) before setting its own; a failed one keeps it,
/// no sooner than its backoff ends ([`failed`]).
pub async fn wake(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    after: Duration,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays SET wake_at = now() + make_interval(secs => $3) \
          WHERE org_id = $1 AND environment = $2",
        [
            org_id.into(),
            environment.into(),
            after.as_secs_f64().into(),
        ],
    ))
    .await
    .map(|_| ())
}

/// A pass that took the lease failed: back the mark off. `failures` counts
/// up, and the mark is not due again ([`due`]) for `base` doubled per earlier
/// failure (from 9 on, no further), at most `cap`, unless nudged since. A
/// pending wake is kept, moved no sooner than the backoff's end, so a
/// baseline it woke for is still retried after the failure.
pub async fn failed(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    base: Duration,
    cap: Duration,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays \
            SET failures = failures + 1, failed_at = now(), \
                retry_after = now() + LEAST( \
                    make_interval(secs => $3::float8 * power(2::float8, LEAST(failures, 9))), \
                    make_interval(secs => $4::float8)), \
                wake_at = CASE WHEN wake_at IS NULL THEN NULL ELSE GREATEST(wake_at, \
                    now() + LEAST( \
                        make_interval(secs => $3::float8 * power(2::float8, LEAST(failures, 9))), \
                        make_interval(secs => $4::float8))) END \
          WHERE org_id = $1 AND environment = $2",
        [
            org_id.into(),
            environment.into(),
            base.as_secs_f64().into(),
            cap.as_secs_f64().into(),
        ],
    ))
    .await
    .map(|_| ())
}

/// A pass that took the lease had no writer fail, whether it drained or left
/// rows: the mark's backoff is over, and so is any wake it ran for.
pub async fn succeeded(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays \
            SET failures = 0, failed_at = NULL, retry_after = NULL, wake_at = NULL \
          WHERE org_id = $1 AND environment = $2 \
            AND (failures <> 0 OR failed_at IS NOT NULL OR retry_after IS NOT NULL \
                 OR wake_at IS NOT NULL)",
        [org_id.into(), environment.into()],
    ))
    .await
    .map(|_| ())
}

/// Whether the sweep should log `oltp_changes_stale` now for a mark it found
/// due and stale, recording that it did. Outside a backoff window, every
/// time; inside one (`retry_after` still ahead), once: a mark a nudge makes
/// due while backed off would otherwise alarm at every sweep.
pub async fn alarm_stale(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<bool, DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays SET stale_alarmed_at = now() \
          WHERE org_id = $1 AND environment = $2 \
            AND (retry_after IS NULL OR retry_after <= now() \
                 OR stale_alarmed_at IS NULL OR stale_alarmed_at < failed_at)",
        [org_id.into(), environment.into()],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
}

/// The weekly sweep: every mark is nudged now, so each store is drained once
/// whatever its marks say. Returns how many marks were nudged.
pub async fn mark_all(db: &impl ConnectionTrait) -> Result<u64, DbErr> {
    db.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays \
            SET nudged_at = now(), pending_since = COALESCE(pending_since, now())",
    ))
    .await
    .map(|r| r.rows_affected())
}

/// Take the lease on (org, environment) for `ttl`, bumping its epoch, if it is
/// free (never held, released or expired). `None` when another holder has it.
pub async fn acquire(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    holder: &str,
    ttl: Duration,
) -> Result<Option<Lease>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO oltp_change_relays AS r \
                    (org_id, environment, lease_holder, lease_epoch, lease_expires_at) \
             VALUES ($1, $2, $3, 1, now() + make_interval(secs => $4)) \
             ON CONFLICT (org_id, environment) DO UPDATE \
                SET lease_holder = EXCLUDED.lease_holder, \
                    lease_epoch = r.lease_epoch + 1, \
                    lease_expires_at = EXCLUDED.lease_expires_at \
              WHERE r.lease_expires_at IS NULL OR r.lease_expires_at < now() \
             RETURNING lease_epoch",
            [
                org_id.into(),
                environment.into(),
                holder.into(),
                ttl.as_secs_f64().into(),
            ],
        ))
        .await?;
    row.map(|row| {
        Ok(Lease {
            org_id,
            environment: environment.to_string(),
            holder: holder.to_string(),
            epoch: row.try_get("", "lease_epoch")?,
        })
    })
    .transpose()
}

/// Extend `lease` to `ttl` from now. `false` when the holder, the epoch or an
/// unexpired lease no longer match: the caller has lost the lease and stops.
pub async fn renew(db: &impl ConnectionTrait, lease: &Lease, ttl: Duration) -> Result<bool, DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays \
            SET lease_expires_at = now() + make_interval(secs => $5) \
          WHERE org_id = $1 AND environment = $2 AND lease_holder = $3 AND lease_epoch = $4 \
            AND lease_expires_at > now()",
        [
            lease.org_id.into(),
            lease.environment.clone().into(),
            lease.holder.clone().into(),
            lease.epoch.into(),
            ttl.as_secs_f64().into(),
        ],
    ))
    .await
    .map(|r| r.rows_affected() == 1)
}

/// Give the lease up, if it is still this holder's at this epoch. The epoch
/// stays, so the next acquisition still counts up.
pub async fn release(db: &impl ConnectionTrait, lease: &Lease) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_relays SET lease_holder = NULL, lease_expires_at = NULL \
          WHERE org_id = $1 AND environment = $2 AND lease_holder = $3 AND lease_epoch = $4",
        [
            lease.org_id.into(),
            lease.environment.clone().into(),
            lease.holder.clone().into(),
            lease.epoch.into(),
        ],
    ))
    .await
    .map(|_| ())
}
