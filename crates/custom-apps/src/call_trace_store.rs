//! Where an invocation's call trace ([`crate::call_trace`]) is kept:
//! `app_function_invocation_calls`, one row per invocation that reached
//! tenant code, Control-placed (`crates/workspace-runtime/tests/integration/data_placement.rs`).
//!
//! **Off the request path.** [`spawn_write`] hands the row to a task of its
//! own and returns at once, so a `/fn` answer never waits on the INSERT (the
//! custom-app perf rule: usage writes are off the request path). A lost write
//! reads back as `not_recorded`, which the console already shows.
//!
//! **Why Postgres and not the product ClickHouse store**, where span-shaped
//! data otherwise lives: product capture is opt-in and off on a dev box, so a
//! waterfall read from it would go dark exactly where people debug. The bound
//! is by design instead: at most `MAX_TRACED_CALLS` calls a row, and a
//! [`retention_days`] prune (default 3) run by the tracking cleanup loop.

use chrono::Utc;
use entity::app_function_invocation_calls as calls;
use sea_orm::{
    ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, Statement,
};
use sentry::SentryFutureExt;
use uuid::Uuid;

use crate::call_trace::CallSnapshot;

/// Days a trace is kept, unless `OXY_APP_CALL_TRACE_RETENTION_DAYS` says
/// otherwise. A debugging aid, not history.
pub const DEFAULT_RETENTION_DAYS: i64 = 3;

/// Rows one prune statement deletes, so a large backlog never holds one long
/// lock.
const PRUNE_BATCH: i64 = 10_000;

/// The retention window in days: the env override when it is a positive
/// integer, the default otherwise.
pub fn retention_days() -> i64 {
    parse_retention(
        std::env::var("OXY_APP_CALL_TRACE_RETENTION_DAYS")
            .ok()
            .as_deref(),
    )
}

fn parse_retention(value: Option<&str>) -> i64 {
    value
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// The row a settled invocation's trace is written as.
pub struct TraceRow {
    pub invocation_id: Uuid,
    pub app_id: Uuid,
    pub init_ms: Option<u32>,
    pub snapshot: CallSnapshot,
}

fn to_i32(n: u32) -> i32 {
    n.min(i32::MAX as u32) as i32
}

/// Write `row` from a task of its own and return without waiting. Carries the
/// caller's Sentry hub, as the held-write log's writes do.
pub fn spawn_write(db: DatabaseConnection, row: TraceRow) {
    tokio::spawn(
        async move {
            let invocation_id = row.invocation_id;
            if let Err(e) = write(&db, row).await {
                tracing::warn!(%invocation_id, "failed to write the invocation's call trace: {e}");
            }
        }
        .bind_hub(sentry::Hub::current()),
    );
}

/// Close the trace a settled run left on `meters` and write it off the
/// request path. A run that made no calls still gets its row, so "made no
/// calls" reads apart from "not recorded"; one whose tenant code never ran (a
/// compile failure, a timeout during setup) gets a row with no `init_ms`,
/// which reads as "did not start". A `shed` run writes nothing: it was
/// refused for load, and a write per refusal would add to the load.
pub fn spawn_settled(
    db: &DatabaseConnection,
    app_id: Uuid,
    invocation_id: Uuid,
    status: &str,
    meters: &crate::telemetry::InvocationMeters,
) {
    if status == "shed" {
        return;
    }
    let snapshot = meters.calls.finish(meters.host_calls());
    spawn_write(
        db.clone(),
        TraceRow {
            invocation_id,
            app_id,
            init_ms: meters.init_ms(),
            snapshot,
        },
    );
}

/// Insert `row`; a second write for the same invocation is a no-op.
pub async fn write(db: &DatabaseConnection, row: TraceRow) -> Result<(), DbErr> {
    let model = calls::ActiveModel {
        invocation_id: Set(row.invocation_id),
        app_id: Set(row.app_id),
        created_at: Set(Utc::now().fixed_offset()),
        init_ms: Set(row.init_ms.map(to_i32)),
        total_calls: Set(to_i32(row.snapshot.total_calls)),
        dropped: Set(to_i32(row.snapshot.dropped)),
        unfinished: Set(to_i32(row.snapshot.unfinished)),
        calls: Set(row.snapshot.document()),
    };
    calls::Entity::insert(model)
        .on_conflict_do_nothing_on([calls::Column::InvocationId])
        .exec(db)
        .await
        .map(|_| ())
}

/// The trace of `invocation_id`, scoped to `app_id` as well as by key.
pub async fn read(
    db: &DatabaseConnection,
    app_id: Uuid,
    invocation_id: Uuid,
) -> Result<Option<calls::Model>, DbErr> {
    use sea_orm::{ColumnTrait, QueryFilter};
    calls::Entity::find_by_id(invocation_id)
        .filter(calls::Column::AppId.eq(app_id))
        .one(db)
        .await
}

/// Delete every trace older than the retention window, in batches. Idempotent
/// across replicas: two pruning at once delete disjoint or no rows.
pub async fn prune_expired(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let cutoff = (Utc::now() - chrono::Duration::days(retention_days())).fixed_offset();
    let mut deleted = 0;
    loop {
        let statement = Statement::from_sql_and_values(
            db.get_database_backend(),
            "DELETE FROM app_function_invocation_calls WHERE ctid IN \
             (SELECT ctid FROM app_function_invocation_calls WHERE created_at < $1 LIMIT $2)",
            [cutoff.into(), PRUNE_BATCH.into()],
        );
        let batch = db.execute_raw(statement).await?.rows_affected();
        deleted += batch;
        if batch < PRUNE_BATCH as u64 {
            return Ok(deleted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_three_days_unless_a_positive_override_says_otherwise() {
        assert_eq!(parse_retention(None), 3);
        assert_eq!(parse_retention(Some("7")), 7);
        assert_eq!(parse_retention(Some(" 1 ")), 1);
        for bad in ["0", "-2", "soon", ""] {
            assert_eq!(parse_retention(Some(bad)), 3, "{bad}");
        }
    }
}
