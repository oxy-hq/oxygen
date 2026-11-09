//! CRUD on the `agentic_run_events` table.

use std::collections::HashMap;

use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, Statement, TransactionTrait,
};
use serde_json::Value;

use crate::lifecycle::entity::run_event;
use crate::lifecycle::remote_wake::RUN_EVENTS_CHANNEL;

use super::now;

pub struct EventRow {
    pub seq: i64,
    pub event_type: String,
    pub payload: Value,
    pub attempt: i32,
}

/// Columns of one event row, in the order [`insert_events_sql`] binds them.
const EVENT_COLUMNS: usize = 6;

/// Rows per statement. Postgres binds at most 65 535 parameters; this keeps a
/// statement under 10 % of that, and a bridge flush or a workflow decision is
/// far smaller anyway.
const ROWS_PER_STATEMENT: usize = 1000;

/// The append, with its NOTIFY in the same statement, for `rows` event rows.
///
/// `INSERT … RETURNING` feeds a `pg_notify` guarded by `EXISTS`, so the
/// notification rides the insert's own commit (deferred to the caller's,
/// inside a transaction), costs no second pool acquire, fires once however
/// many rows the batch holds, and is skipped when every row was a conflict —
/// `DO NOTHING` returns no row — because there is nothing new for a stream to
/// read. The payload is the run id and nothing else; the listener looks the
/// stream up by it (`crate::lifecycle::remote_wake`).
///
/// Parameters: six per row (`run_id, seq, event_type, payload, attempt,
/// created_at`), then the channel, then the run id.
pub(crate) fn insert_events_sql(rows: usize) -> String {
    let groups: Vec<String> = (0..rows)
        .map(|row| {
            let first = row * EVENT_COLUMNS + 1;
            let params: Vec<String> = (first..first + EVENT_COLUMNS)
                .map(|n| format!("${n}"))
                .collect();
            format!("({})", params.join(", "))
        })
        .collect();
    let channel = rows * EVENT_COLUMNS + 1;
    let run_id = channel + 1;
    format!(
        "WITH inserted AS ( \
             INSERT INTO agentic_run_events \
                 (run_id, seq, event_type, payload, attempt, created_at) \
             VALUES {} \
             ON CONFLICT (run_id, seq) DO NOTHING \
             RETURNING run_id \
         ) SELECT pg_notify(${channel}, ${run_id}) WHERE EXISTS (SELECT 1 FROM inserted)",
        groups.join(", ")
    )
}

/// One event row to append: `(seq, event_type, payload, attempt)`.
type EventInput<'a> = (i64, &'a str, Value, i32);

/// Append `rows` for `run_id` in statements of [`ROWS_PER_STATEMENT`], each
/// carrying its NOTIFY. Every write to `agentic_run_events` ends here.
async fn insert_rows(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
    rows: Vec<EventInput<'_>>,
) -> Result<(), DbErr> {
    let created_at = now();
    for chunk in rows.chunks(ROWS_PER_STATEMENT) {
        let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * EVENT_COLUMNS + 2);
        for (seq, event_type, payload, attempt) in chunk {
            values.extend([
                run_id.into(),
                (*seq).into(),
                (*event_type).into(),
                payload.clone().into(),
                (*attempt).into(),
                created_at.into(),
            ]);
        }
        values.push(RUN_EVENTS_CHANNEL.into());
        values.push(run_id.into());
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            insert_events_sql(chunk.len()),
            values,
        ))
        .await?;
    }
    Ok(())
}

/// Append one event row for `run_id`, and wake its streams on every pod.
///
/// Every append to `agentic_run_events` goes through this module —
/// [`insert_event`], [`batch_insert_events`] (the event bridge) or
/// [`append_events`] (a caller's own transaction; `agentic-automation`'s
/// decision commit) — and every one rings [`RUN_EVENTS_CHANNEL`] in the
/// write's transaction. A stream on a pod that is not driving the run parks
/// on that notification instead of polling. A new writer that bypasses this
/// module leaves such a stream on its safety-net timer.
pub async fn insert_event(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
    seq: i64,
    event_type: &str,
    payload: &Value,
    attempt: i32,
) -> Result<(), DbErr> {
    insert_rows(
        db,
        run_id,
        vec![(seq, event_type, payload.clone(), attempt)],
    )
    .await
}

/// Append `events` for `run_id` as consecutive rows from `first_seq`, inside
/// the caller's connection or transaction, with the NOTIFY in the same
/// statement. For a writer that owns its transaction and needs the rows to
/// land, or not, with its other writes.
pub async fn append_events(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
    first_seq: i64,
    events: &[(String, Value)],
    attempt: i32,
) -> Result<(), DbErr> {
    if events.is_empty() {
        return Ok(());
    }
    let rows = events
        .iter()
        .enumerate()
        .map(|(i, (event_type, payload))| {
            (
                first_seq + i as i64,
                event_type.as_str(),
                payload.clone(),
                attempt,
            )
        })
        .collect();
    insert_rows(db, run_id, rows).await
}

/// `NOTIFY oxy_run_events, '<run_id>'` on `db`, for a write that changes what
/// a stream should do without appending an event: a run-row-only terminal
/// transition (`transition_run`, the recovery and dead-letter paths). Inside a
/// transaction Postgres delivers it at commit, after the row is visible, and
/// collapses it with any identical notification the same transaction sent.
pub async fn notify_run_events(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_notify($1, $2)",
        [RUN_EVENTS_CHANNEL.into(), run_id.into()],
    ))
    .await?;
    Ok(())
}

/// [`notify_run_events`] for a terminal row write that has **already
/// committed** on a pooled connection. The notify is a second statement and a
/// second acquire, which can fail in exactly the pool-exhaustion scenario the
/// stream's timer is sized against — and the run did end, so the caller's
/// post-write work (the failure observer, a lease release) must still run and
/// its result must still read as the close it was. A missed notify costs the
/// stream one safety-net interval, which is what the timer is for.
pub async fn notify_run_events_best_effort(db: &impl sea_orm::ConnectionTrait, run_id: &str) {
    if let Err(e) = notify_run_events(db, run_id).await {
        tracing::debug!(run_id, error = %e, "terminal run row written but not notified");
    }
}

/// The event bridge's flush: pre-numbered rows, each with its own attempt,
/// as one transaction so the batch lands whole or not at all.
pub async fn batch_insert_events(
    db: &DatabaseConnection,
    run_id: &str,
    events: &[(i64, String, String, i32)],
) -> Result<(), DbErr> {
    if events.is_empty() {
        return Ok(());
    }
    let rows = events
        .iter()
        .map(|(seq, event_type, payload_str, attempt)| {
            let payload: Value = serde_json::from_str(payload_str).unwrap_or(Value::Null);
            (*seq, event_type.as_str(), payload, *attempt)
        })
        .collect();
    let txn = db.begin().await?;
    if let Err(e) = insert_rows(&txn, run_id, rows).await {
        txn.rollback().await.ok();
        return Err(e);
    }
    txn.commit().await?;
    Ok(())
}

pub async fn get_events_after(
    db: &DatabaseConnection,
    run_id: &str,
    after_seq: i64,
) -> Result<Vec<EventRow>, DbErr> {
    let models = run_event::Entity::find()
        .filter(run_event::Column::RunId.eq(run_id))
        .filter(run_event::Column::Seq.gt(after_seq))
        .order_by_asc(run_event::Column::Seq)
        .all(db)
        .await?;
    Ok(models
        .into_iter()
        .map(|m| EventRow {
            seq: m.seq,
            event_type: m.event_type,
            payload: m.payload,
            attempt: m.attempt,
        })
        .collect())
}

pub async fn get_all_events(db: &DatabaseConnection, run_id: &str) -> Result<Vec<EventRow>, DbErr> {
    let models = run_event::Entity::find()
        .filter(run_event::Column::RunId.eq(run_id))
        .order_by_asc(run_event::Column::Seq)
        .all(db)
        .await?;
    Ok(models
        .into_iter()
        .map(|m| EventRow {
            seq: m.seq,
            event_type: m.event_type,
            payload: m.payload,
            attempt: m.attempt,
        })
        .collect())
}

/// Batched variant of [`get_all_events`] — one SQL round-trip for an entire
/// set of run IDs. Returns a map from `run_id` to its ordered event rows.
/// Run IDs absent from the result had no events.
pub async fn get_all_events_for_runs(
    db: &DatabaseConnection,
    run_ids: &[String],
) -> Result<HashMap<String, Vec<EventRow>>, DbErr> {
    if run_ids.is_empty() {
        return Ok(HashMap::new());
    }

    #[derive(FromQueryResult)]
    struct RawRow {
        run_id: String,
        seq: i64,
        event_type: String,
        payload: Value,
        attempt: i32,
    }

    let rows = RawRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT run_id, seq, event_type, payload, attempt \
         FROM agentic_run_events \
         WHERE run_id = ANY($1) \
         ORDER BY run_id, seq",
        [run_ids.to_vec().into()],
    ))
    .all(db)
    .await?;

    let mut map: HashMap<String, Vec<EventRow>> = HashMap::new();
    for row in rows {
        map.entry(row.run_id).or_default().push(EventRow {
            seq: row.seq,
            event_type: row.event_type,
            payload: row.payload,
            attempt: row.attempt,
        });
    }
    Ok(map)
}

pub async fn delete_events_from_seq(
    db: &DatabaseConnection,
    run_id: &str,
    from_seq: i64,
) -> Result<u64, DbErr> {
    let result = run_event::Entity::delete_many()
        .filter(run_event::Column::RunId.eq(run_id))
        .filter(run_event::Column::Seq.gte(from_seq))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

pub async fn get_max_seq(db: &impl sea_orm::ConnectionTrait, run_id: &str) -> Result<i64, DbErr> {
    let last = run_event::Entity::find()
        .filter(run_event::Column::RunId.eq(run_id))
        .order_by_desc(run_event::Column::Seq)
        .one(db)
        .await?;
    Ok(last.map(|m| m.seq).unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every append statement carries its NOTIFY: a writer routed through this
    /// module cannot leave a remote-driven stream on its safety-net timer.
    #[test]
    fn every_append_statement_notifies_the_run_once() {
        let one = insert_events_sql(1);
        assert!(one.contains("VALUES ($1, $2, $3, $4, $5, $6)"), "{one}");
        assert!(one.contains("pg_notify($7, $8)"), "{one}");
        assert!(
            one.contains("ON CONFLICT (run_id, seq) DO NOTHING"),
            "{one}"
        );
        assert!(
            one.contains("WHERE EXISTS (SELECT 1 FROM inserted)"),
            "{one}"
        );

        let three = insert_events_sql(3);
        assert!(three.contains("($7, $8, $9, $10, $11, $12), ($13, $14, $15, $16, $17, $18)"));
        assert!(three.contains("pg_notify($19, $20)"), "{three}");
        assert_eq!(three.matches("pg_notify").count(), 1);
    }

    /// The batch helper hands the builder exactly the rows it was given.
    #[test]
    fn a_batch_of_events_numbers_its_rows_from_the_first_seq() {
        let events = vec![
            ("a".to_string(), Value::Null),
            ("b".to_string(), Value::Bool(true)),
        ];
        let rows: Vec<EventInput<'_>> = events
            .iter()
            .enumerate()
            .map(|(i, (t, p))| (7 + i as i64, t.as_str(), p.clone(), 2))
            .collect();
        assert_eq!(rows[0].0, 7);
        assert_eq!(rows[1].0, 8);
        assert_eq!(rows[1].1, "b");
        assert!(rows.len() <= ROWS_PER_STATEMENT);
    }
}
