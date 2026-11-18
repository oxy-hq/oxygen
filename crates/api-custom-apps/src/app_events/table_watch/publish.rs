//! Publishing one table change (app events design §7.2, steps 3 and 4).
//!
//! One event per changed snapshot, type `table:<table as written>`, `data`
//! `{ snapshotId, inserted, deleted, rows? }`
//! ([`oxy_custom_apps::events::table::change_data`]).
//!
//! **Where the pieces live.** The rows are org data, so the body goes to the
//! org's OLTP outbox log as a body the platform wrote, already relayed
//! ([`oxy_oltp::events::write_platform_body`]); Oxy's Postgres holds the
//! event's routing metadata and the cursor.
//!
//! **Atomic where it matters.** The body is written first, keyed by the id the
//! event will have: a pass that dies after it leaves a body no event points at
//! (retention's orphan sweep removes it) and the retry rewrites the same one.
//! Then one platform transaction stores the event, inserts a delivery per
//! listener (queuing each its run) and moves the cursor
//! ([`relay::fan_out_with_cursor`]). A snapshot is delivered exactly when the
//! cursor has moved past it, and the move is compare-and-set, so two passes
//! that race publish it once. The cursor is not in the org's OLTP store with
//! the outbox: a cross-database transaction does not exist, the body must be
//! readable before any delivery can be, and the cursor must commit with the
//! deliveries, which are platform rows.

use chrono::Utc;
use entity::app_event_table_cursors as cursors;
use oxy_custom_apps::events::table::{Changed, TableRef, backfill_data, change_data};
use oxy_custom_apps::events::{OxyEvent, event_id, jsonb_text_len};
use oxy_oltp::events::{
    EventsError, OutboxTarget, PLATFORM_WRITER, PlatformBody, write_platform_body,
};
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::Value;
use uuid::Uuid;

use super::super::envelope::TABLE_DATASCHEMA;
use super::super::relay::{
    CURSOR_MOVED, CursorMove, RelayStats, RelayedEvent, fan_out_with_cursor,
};
use super::super::stores::shared_store_of;

/// Why a change was not published.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The environment's store has no branch yet: nothing can be published
    /// there, and nothing is wrong.
    #[error("the environment has no OLTP branch yet")]
    NoStore,
    /// Another pass moved the cursor first; its event is the one.
    #[error("another pass published this snapshot")]
    Raced,
    #[error("{0}")]
    Failed(String),
}

/// The event's key: unique per workspace and snapshot, so two workspaces'
/// identical table names and snapshot numbers never share an id.
pub fn key(workspace_id: Uuid, snapshot_id: i64) -> String {
    format!("{workspace_id}:{snapshot_id}")
}

/// What a change publishes: the event, its data and its place in the org's store.
struct Prepared {
    event: RelayedEvent,
    data: Value,
    target: OutboxTarget,
    name: String,
}

fn prepare(
    cursor: &cursors::Model,
    table: &TableRef,
    key: String,
    data: Value,
    only_listener: Option<(Uuid, String)>,
) -> Result<Prepared, PublishError> {
    // The platform's own event: its body goes where every app's shares.
    let target = shared_store_of(&cursor.environment).ok_or_else(|| {
        PublishError::Failed(format!("`{}` has no relayed store", cursor.environment))
    })?;
    let event_type = table.event_type();
    let id = event_id(cursor.org_id, &cursor.environment, &event_type, &key);
    let now = Utc::now();
    let data_bytes = i32::try_from(jsonb_text_len(&data)).unwrap_or(i32::MAX);
    let event = RelayedEvent {
        envelope: OxyEvent {
            specversion: "1.0".to_string(),
            id,
            event_type: event_type.clone(),
            source: table.source(),
            time: now,
            dataschema: TABLE_DATASCHEMA.to_string(),
            data: Value::Null,
            key,
            environment: cursor.environment.clone(),
            invocation: None,
            actor: None,
            causationid: None,
            rootid: id,
            depth: 0,
        },
        org_id: cursor.org_id,
        workspace_id: cursor.workspace_id,
        source_app_id: None,
        writer_role: PLATFORM_WRITER.to_string(),
        data_bytes,
        schema_revision_id: None,
        delay_seconds: 0,
        relay_error: None,
        directed: None,
        only_listener,
    };
    Ok(Prepared {
        event,
        data,
        target,
        name: event_type,
    })
}

async fn publish(
    db: &DatabaseConnection,
    prepared: Prepared,
    cursor_move: &CursorMove,
) -> Result<RelayStats, PublishError> {
    let Prepared {
        event,
        data,
        target,
        name,
    } = prepared;
    let body = PlatformBody {
        id: event.envelope.id,
        environment: &event.envelope.environment,
        name: &name,
        key: &event.envelope.key,
        data: &data,
        created_at: event.envelope.time,
    };
    write_platform_body(db, event.org_id, &target, &body)
        .await
        .map_err(|e| match e {
            EventsError::NoBranch { .. } => PublishError::NoStore,
            other => PublishError::Failed(other.to_string()),
        })?;
    fan_out_with_cursor(db, &event, cursor_move)
        .await
        .map_err(|e| match e {
            DbErr::Custom(m) if m == CURSOR_MOVED => PublishError::Raced,
            other => PublishError::Failed(other.to_string()),
        })
}

/// Publish what `snapshot` changed and move the cursor from `from` to it.
pub async fn publish_change(
    db: &DatabaseConnection,
    cursor: &cursors::Model,
    table: &TableRef,
    from: Option<i64>,
    snapshot: i64,
    changed: &Changed,
) -> Result<RelayStats, PublishError> {
    let prepared = prepare(
        cursor,
        table,
        key(cursor.workspace_id, snapshot),
        change_data(snapshot, changed),
        None,
    )?;
    publish(
        db,
        prepared,
        &CursorMove {
            id: cursor.id,
            from,
            to: snapshot,
        },
    )
    .await
}

/// Publish the one backfill event of a resync, to `listener` alone, with the
/// cursor already at `snapshot` (it moves to the same place: a no-op move
/// that still guards against a concurrent pass).
pub async fn publish_backfill(
    db: &DatabaseConnection,
    cursor: &cursors::Model,
    table: &TableRef,
    snapshot: i64,
    listener: (Uuid, String),
) -> Result<RelayStats, PublishError> {
    let key = format!("backfill:{}:{}", cursor.workspace_id, Uuid::new_v4());
    let prepared = prepare(cursor, table, key, backfill_data(snapshot), Some(listener))?;
    publish(
        db,
        prepared,
        &CursorMove {
            id: cursor.id,
            from: Some(snapshot),
            to: snapshot,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_workspaces_never_share_an_event_key() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_ne!(key(a, 7), key(b, 7));
        assert_ne!(key(a, 7), key(a, 8));
        assert!(key(a, 7).len() <= oxy_custom_apps::events::MAX_KEY_LEN);
    }
}
