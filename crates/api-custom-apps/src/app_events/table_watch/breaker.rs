//! The per-subscription circuit breaker (app events design §7.3).
//!
//! The depth limit cannot see through a table write: handler A writes table
//! T1, B watches T1 and writes T2, A watches T2. [`super::loop_guard`] refuses
//! a function's write to the table that woke it; this breaker catches every
//! loop the guard cannot name, visibly.
//!
//! **The rule.** A subscription that received [`MAX_DELIVERIES`] or more table
//! deliveries within the last [`WINDOW`] is paused: `paused_at` and
//! `paused_reason` are set on its `app_event_subscriptions` row, and from then
//! on the relay's fan-out holds its deliveries (a held delivery is recorded,
//! visible in `oxyc events ls`, and delivered by an explicit redeliver). A
//! pause pages in production (`app_events_subscription_paused`).
//!
//! **The threshold is 150 in 5 minutes** (0.5 a second sustained). Every
//! changed snapshot is one handler run, so this is a handler running every two
//! seconds for five minutes without a pause: an honest pipeline lands a table
//! a few times an hour, and an ingest that appends per webhook batches below
//! it. It is a count of deliveries rather than of snapshots, so a table with
//! three listeners trips each listener at its own rate. A catch-up after the
//! tracker was down counts too, on purpose: a flood of handler runs is what
//! the breaker is for, and `oxyc events resync` is the way out of either.
//!
//! **Only `resync` resumes.** It moves the cursor to now, clears the pause and
//! the states, so what changed while the loop ran is skipped, not replayed.

use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

/// The window deliveries are counted in.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);
/// Deliveries within [`WINDOW`] that pause a subscription.
pub const MAX_DELIVERIES: i64 = 150;

/// A subscription the breaker paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tripped {
    pub app_id: Uuid,
    pub function_name: String,
    pub deliveries: i64,
}

/// Pause every subscription to `event_type` in the workspace's `environment`
/// that is over the rate. Returns the ones it paused.
pub async fn trip(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    environment: &str,
    event_type: &str,
) -> Result<Vec<Tripped>, DbErr> {
    let reason = format!(
        "{MAX_DELIVERIES} or more deliveries within {} minutes: a loop, or a table written \
         faster than a handler should run",
        WINDOW.as_secs() / 60
    );
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "WITH recent AS ( \
               SELECT d.app_id, d.function_name, count(*) AS n \
                 FROM app_event_deliveries d JOIN app_events e ON e.id = d.event_id \
                WHERE e.org_id = $1 AND e.environment = $3 AND e.type = $4 \
                  AND e.workspace_id = $2 AND e.published_at > now() - make_interval(secs => $5) \
                  AND e.published_at > COALESCE((SELECT max(s0.resumed_at) FROM app_event_subscriptions s0 \
                        WHERE s0.app_id = d.app_id AND s0.function_name = d.function_name \
                          AND s0.environment = e.environment AND s0.event_type = e.type), 'epoch') \
                GROUP BY d.app_id, d.function_name HAVING count(*) >= $6) \
             UPDATE app_event_subscriptions s \
                SET paused_at = now(), paused_reason = $7, updated_at = now() \
               FROM recent r \
              WHERE s.org_id = $1 AND s.workspace_id = $2 AND s.environment = $3 \
                AND s.event_type = $4 AND s.paused_at IS NULL \
                AND s.app_id = r.app_id AND s.function_name = r.function_name \
          RETURNING s.app_id, s.function_name, r.n",
            [
                org_id.into(),
                workspace_id.into(),
                environment.into(),
                event_type.into(),
                (WINDOW.as_secs() as f64).into(),
                MAX_DELIVERIES.into(),
                reason.into(),
            ],
        ))
        .await?;
    rows.into_iter()
        .map(|r| {
            Ok(Tripped {
                app_id: r.try_get("", "app_id")?,
                function_name: r.try_get("", "function_name")?,
                deliveries: r.try_get("", "n")?,
            })
        })
        .collect()
}

/// Resume `function_name` of `app_id` in `environment`: clear its pause.
pub async fn resume(
    db: &DatabaseConnection,
    app_id: Uuid,
    function_name: &str,
    environment: &str,
) -> Result<u64, DbErr> {
    let done = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_event_subscriptions SET paused_at = NULL, paused_reason = NULL, \
                    resumed_at = now(), updated_at = now() \
              WHERE app_id = $1 AND function_name = $2 AND environment = $3 \
                AND event_type LIKE 'table:%'",
            [app_id.into(), function_name.into(), environment.into()],
        ))
        .await?;
    Ok(done.rows_affected())
}
