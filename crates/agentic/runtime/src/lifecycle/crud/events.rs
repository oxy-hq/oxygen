//! CRUD on the `agentic_run_events` table.

use std::collections::HashMap;

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::*, ColumnTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, Statement, TransactionTrait,
};
use serde_json::Value;

use crate::lifecycle::entity::run_event;

use super::now;

pub struct EventRow {
    pub seq: i64,
    pub event_type: String,
    pub payload: Value,
    pub attempt: i32,
}

pub async fn insert_event(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
    seq: i64,
    event_type: &str,
    payload: &Value,
    attempt: i32,
) -> Result<(), DbErr> {
    let event = run_event::ActiveModel {
        id: NotSet,
        run_id: Set(run_id.to_string()),
        seq: Set(seq),
        event_type: Set(event_type.to_string()),
        payload: Set(payload.clone()),
        attempt: Set(attempt),
        created_at: Set(now()),
    };
    match run_event::Entity::insert(event)
        .on_conflict(
            OnConflict::columns([run_event::Column::RunId, run_event::Column::Seq])
                .do_nothing()
                .to_owned(),
        )
        .exec(db)
        .await
    {
        Ok(_) | Err(DbErr::RecordNotInserted) => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

pub async fn batch_insert_events(
    db: &DatabaseConnection,
    run_id: &str,
    events: &[(i64, String, String, i32)],
) -> Result<(), DbErr> {
    if events.is_empty() {
        return Ok(());
    }
    let ts = now();
    let txn = db.begin().await?;
    for (seq, event_type, payload_str, attempt) in events {
        let payload: Value = serde_json::from_str(payload_str).unwrap_or(Value::Null);
        let event = run_event::ActiveModel {
            id: NotSet,
            run_id: Set(run_id.to_string()),
            seq: Set(*seq),
            event_type: Set(event_type.clone()),
            payload: Set(payload),
            attempt: Set(*attempt),
            created_at: Set(ts),
        };
        let res = run_event::Entity::insert(event)
            .on_conflict(
                OnConflict::columns([run_event::Column::RunId, run_event::Column::Seq])
                    .do_nothing()
                    .to_owned(),
            )
            .exec(&txn)
            .await;
        match res {
            Ok(_) | Err(DbErr::RecordNotInserted) => {}
            Err(e) => {
                txn.rollback().await.ok();
                return Err(e);
            }
        }
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
