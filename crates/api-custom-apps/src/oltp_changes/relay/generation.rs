//! A store's generation, on the control plane (OLTP change stream design §5.5):
//! it counts up on each restore the relay settles, and outlives the store's
//! relay row, so a recreated row never hands out a generation twice.
//!
//! A bump that timed out behind a long transaction backs off on the same row
//! ([`bump_failed`]), so the relay does not queue its lock on the stream on
//! every pass, until [`bump_retry_in`] says it may try again; a bump that
//! succeeds ends the backoff ([`bump_succeeded`]).

use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// The next generation of `org_id`'s `environment`: past both what was handed
/// out and `floor`, the highest the store or Airhouse already holds.
pub async fn next_generation(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    floor: i64,
) -> Result<i64, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO oltp_change_generations AS g (org_id, environment, generation) \
             VALUES ($1, $2, $3 + 1) \
             ON CONFLICT (org_id, environment) DO UPDATE \
               SET generation = GREATEST(g.generation, $3) + 1, bumped_at = now() \
             RETURNING generation",
            [org_id.into(), environment.into(), floor.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("the generation upsert returned no row".into()))?;
    row.try_get("", "generation")
}

/// A bump of `org_id`'s `environment` timed out: back it off for `base`
/// doubled per earlier attempt (from 9 on, no further), at most `cap`.
/// Returns how long until it may be tried again; `None` with no row, which a
/// bump never leaves, as [`next_generation`] made it first.
pub async fn bump_failed(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    base: Duration,
    cap: Duration,
) -> Result<Option<Duration>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE oltp_change_generations \
                SET bump_attempts = bump_attempts + 1, \
                    bump_retry_at = now() + LEAST( \
                      make_interval(secs => $3::float8 * power(2::float8, LEAST(bump_attempts, 9))), \
                      make_interval(secs => $4::float8)) \
              WHERE org_id = $1 AND environment = $2 \
          RETURNING EXTRACT(EPOCH FROM bump_retry_at - now())::float8 AS retry_in",
            [
                org_id.into(),
                environment.into(),
                base.as_secs_f64().into(),
                cap.as_secs_f64().into(),
            ],
        ))
        .await?;
    row.map(|row| row.try_get("", "retry_in").map(seconds))
        .transpose()
}

/// How long until a backed-off bump of `org_id`'s `environment` may be
/// tried again; `None` when it may be tried now.
pub async fn bump_retry_in(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<Option<Duration>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXTRACT(EPOCH FROM bump_retry_at - now())::float8 AS retry_in \
               FROM oltp_change_generations \
              WHERE org_id = $1 AND environment = $2 AND bump_retry_at > now()",
            [org_id.into(), environment.into()],
        ))
        .await?;
    row.map(|row| row.try_get("", "retry_in").map(seconds))
        .transpose()
}

/// A bump of `org_id`'s `environment` succeeded: its backoff is over.
pub async fn bump_succeeded(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE oltp_change_generations SET bump_attempts = 0, bump_retry_at = NULL \
          WHERE org_id = $1 AND environment = $2 \
            AND (bump_attempts <> 0 OR bump_retry_at IS NOT NULL)",
        [org_id.into(), environment.into()],
    ))
    .await
    .map(|_| ())
}

/// Whole and fractional seconds, never negative, as a [`Duration`].
fn seconds(secs: f64) -> Duration {
    Duration::from_secs_f64(secs.max(0.0))
}
