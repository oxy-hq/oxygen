//! When a sandbox was last used, and so when it expires.
//!
//! **Idle is computed, not stored**: activity is a pointer move (a publish —
//! `app_environments.updated_at`) or a function invocation (the newest
//! `app_function_invocations` row of the sandbox's environment), so a function
//! call pays no extra write. Opening the page in a browser is not activity.
//!
//! The rule is stated twice — [`last_activity`] in Rust for what the API
//! shows, and [`IDLE_SANDBOXES_SQL`] for what the sweep selects — and
//! `tests/custom_apps/sandbox_sweep.rs` holds the two together.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// A sandbox's last activity: the later of its last pointer move and its
/// newest function invocation.
pub fn last_activity(
    updated_at: DateTime<Utc>,
    last_invocation: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    last_invocation.map_or(updated_at, |invoked| invoked.max(updated_at))
}

/// When a sandbox last active at `last_activity` is deleted for being idle.
pub fn expires_at(last_activity: DateTime<Utc>, ttl: chrono::Duration) -> DateTime<Utc> {
    last_activity + ttl
}

/// Has a sandbox last active at `last_activity` been idle past `ttl` at `now`?
/// Strictly past: one that expires exactly now is spared this pass.
pub fn is_idle(last_activity: DateTime<Utc>, now: DateTime<Utc>, ttl: chrono::Duration) -> bool {
    expires_at(last_activity, ttl) < now
}

/// The newest function invocation of each **sandbox** of `app_id`, by
/// environment name. A sandbox never invoked has no entry.
///
/// One query: a probe of `idx_app_function_invocations_app_env_created` per
/// sandbox row, rather than a grouped scan of every invocation the app ever
/// made — production's are the bulk of them and are never read here.
pub async fn last_invocations<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<HashMap<String, DateTime<Utc>>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT e.name, \
                    (SELECT max(i.created_at) FROM app_function_invocations i \
                      WHERE i.app_id = e.app_id AND i.environment = e.name) AS last_invocation \
               FROM app_environments e \
              WHERE e.app_id = $1 AND e.kind = 'dev'",
            [app_id.into()],
        ))
        .await?;
    let mut newest = HashMap::new();
    for row in rows {
        let name: String = row.try_get("", "name")?;
        let at: Option<DateTime<Utc>> = row.try_get("", "last_invocation")?;
        if let Some(at) = at {
            newest.insert(name, at);
        }
    }
    Ok(newest)
}

/// The newest function invocation of one sandbox, if it was ever invoked —
/// what the sweep reads again, under the sandbox's row lock, before it
/// expires it.
pub async fn last_invocation<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &str,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT max(created_at) AS last_invocation FROM app_function_invocations \
              WHERE app_id = $1 AND environment = $2",
            [app_id.into(), environment.into()],
        ))
        .await?;
    match row {
        Some(row) => row.try_get("", "last_invocation"),
        None => Ok(None),
    }
}

/// Active sandboxes idle since before `$1`, oldest pointer move first, at most
/// `$2` — [`is_idle`] in SQL, for the sweep.
pub const IDLE_SANDBOXES_SQL: &str = "\
    SELECT e.app_id, e.name \
      FROM app_environments e \
     WHERE e.kind = 'dev' AND e.deleting_at IS NULL \
       AND GREATEST(e.updated_at, COALESCE( \
             (SELECT max(i.created_at) FROM app_function_invocations i \
               WHERE i.app_id = e.app_id AND i.environment = e.name), \
             e.updated_at)) < $1 \
     ORDER BY e.updated_at \
     LIMIT $2";

/// `(app_id, name)` of up to `limit` active sandboxes whose last activity is
/// more than `ttl` before `now`.
pub async fn idle_sandboxes<C: ConnectionTrait>(
    db: &C,
    now: DateTime<Utc>,
    ttl: chrono::Duration,
    limit: i64,
) -> Result<Vec<(Uuid, String)>, DbErr> {
    let cutoff = now - ttl;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            IDLE_SANDBOXES_SQL,
            [cutoff.into(), limit.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("", "app_id")?, row.try_get("", "name")?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
    }

    /// The later of the two counts; a sandbox never invoked falls back to its
    /// last pointer move, and an invocation older than a publish does not
    /// pull activity back.
    #[test]
    fn last_activity_is_the_later_of_a_publish_and_an_invocation() {
        assert_eq!(last_activity(at(1, 9), None), at(1, 9));
        assert_eq!(last_activity(at(1, 9), Some(at(3, 12))), at(3, 12));
        assert_eq!(last_activity(at(5, 9), Some(at(3, 12))), at(5, 9));
    }

    #[test]
    fn a_sandbox_expires_a_ttl_after_its_last_activity_and_not_a_moment_before() {
        let week = chrono::Duration::days(7);
        let active = at(1, 9);
        assert_eq!(expires_at(active, week), at(8, 9));
        assert!(!is_idle(active, at(8, 8), week), "an hour short of a week");
        assert!(!is_idle(active, at(8, 9), week), "exactly a week: spared");
        assert!(is_idle(active, at(8, 10), week), "an hour past");
        assert!(is_idle(active, at(2, 10), chrono::Duration::days(1)));
    }
}
