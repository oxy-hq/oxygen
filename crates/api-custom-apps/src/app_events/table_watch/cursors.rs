//! The cursor of each watched table (app events design §7.2): one row per
//! (workspace, environment, table), `app_event_table_cursors`.
//!
//! A cursor exists while some subscription of the org in that environment
//! watches the table ([`reconcile`], run by every subscription reconcile and
//! retire), and goes with the last one. It starts unplaced (`snapshot_id`
//! null); the tracker places it at the snapshot that was newest when the
//! cursor was created, so a change made after the watch was declared is not
//! missed and nothing before it is replayed.
//!
//! **The cursor moves only with the event.** [`advance_in`] runs in the
//! platform transaction that stores the event and its deliveries
//! (`relay::fan_out_with`), compare-and-set on the snapshot it read: a second
//! tracker that raced the first finds the cursor moved, fails, and its
//! transaction (the duplicate event with it) is rolled back.

use chrono::{DateTime, Utc};
use entity::app_event_table_cursors as cursors;
use oxy_custom_apps::events::table::TableRef;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Statement,
};
use serde_json::Value;
use uuid::Uuid;

pub const ACTIVE: &str = "active";
pub const BEHIND_HORIZON: &str = "behind_horizon";
pub const BROKEN: &str = "broken";

/// The table `cursor` watches, as written.
pub fn table_of(cursor: &cursors::Model) -> Option<TableRef> {
    TableRef::parse(&cursor.table_ref).ok()
}

/// Make the org's cursors in `environment` those its table subscriptions
/// call for: one for each `(workspace, table)` some subscription names, none
/// for a table nobody watches any more.
pub async fn reconcile(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    let values = || [org_id.into(), environment.into()];
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_event_table_cursors (id, org_id, workspace_id, environment, table_ref) \
         SELECT gen_random_uuid(), $1, s.workspace_id, $2, substr(s.event_type, 7) \
           FROM app_event_subscriptions s \
          WHERE s.org_id = $1 AND s.environment = $2 AND s.event_type LIKE 'table:%' \
          GROUP BY s.workspace_id, s.event_type \
         ON CONFLICT (workspace_id, environment, table_ref) DO NOTHING",
        values(),
    ))
    .await?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_event_table_cursors c \
          WHERE c.org_id = $1 AND c.environment = $2 \
            AND NOT EXISTS (SELECT 1 FROM app_event_subscriptions s \
                             WHERE s.org_id = c.org_id AND s.environment = c.environment \
                               AND s.workspace_id = c.workspace_id \
                               AND s.event_type = 'table:' || c.table_ref)",
        values(),
    ))
    .await?;
    Ok(())
}

/// Every cursor of the org in `environment`, oldest first.
pub async fn of_environment(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<Vec<cursors::Model>, DbErr> {
    cursors::Entity::find()
        .filter(cursors::Column::OrgId.eq(org_id))
        .filter(cursors::Column::Environment.eq(environment))
        .order_by_asc(cursors::Column::CreatedAt)
        .all(db)
        .await
}

/// The `(org, environment)` pairs with a cursor, for orgs the caller then
/// filters by flag: what the sweep queues a pass for.
pub async fn tracked(db: &DatabaseConnection) -> Result<Vec<(Uuid, String)>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT org_id, environment FROM app_event_table_cursors",
        ))
        .await?;
    rows.into_iter()
        .map(|r| Ok((r.try_get("", "org_id")?, r.try_get("", "environment")?)))
        .collect()
}

/// Environments of `workspace_id` that have a cursor: what a write's nudge
/// queues a pass for.
pub async fn environments_of(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<(Uuid, String)>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT org_id, environment FROM app_event_table_cursors \
              WHERE workspace_id = $1",
            [workspace_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|r| Ok((r.try_get("", "org_id")?, r.try_get("", "environment")?)))
        .collect()
}

/// Move the cursor from `from` to `to` inside `txn`, compare-and-set: `false`
/// when it was no longer at `from` (another pass moved it first).
pub async fn advance_in(
    txn: &impl ConnectionTrait,
    id: Uuid,
    from: Option<i64>,
    to: i64,
) -> Result<bool, DbErr> {
    let done = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_event_table_cursors \
                SET snapshot_id = $3, advanced_at = now(), updated_at = now() \
              WHERE id = $1 AND snapshot_id IS NOT DISTINCT FROM $2",
            [id.into(), from.into(), to.into()],
        ))
        .await?;
    Ok(done.rows_affected() == 1)
}

/// What a pass read of a cursor's table, recorded whether or not it
/// published: the newest snapshot, where the table was read, its columns and
/// the state.
pub struct Checked<'a> {
    pub head: i64,
    pub schema: &'a str,
    pub columns: &'a Value,
    pub table_id: Option<i64>,
    pub state: &'a str,
    pub detail: Option<&'a str>,
}

pub async fn record_check(
    db: &DatabaseConnection,
    id: Uuid,
    checked: &Checked<'_>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_table_cursors \
            SET head_snapshot = $2, watched_schema = $3, table_columns = $4, state = $5, \
                table_id = COALESCE(table_id, $7), state_detail = $6, checked_at = now(), updated_at = now() \
          WHERE id = $1",
        [
            id.into(),
            checked.head.into(),
            checked.schema.into(),
            checked.columns.clone().into(),
            checked.state.into(),
            checked.detail.map(str::to_string).into(),
            checked.table_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Set a state with no read behind it (a table that is gone, a store that
/// has no branch yet): the columns the last read recorded stay.
pub async fn set_state(
    db: &DatabaseConnection,
    id: Uuid,
    state: &str,
    detail: Option<&str>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_table_cursors \
            SET state = $2, state_detail = $3, checked_at = now(), updated_at = now() \
          WHERE id = $1",
        [id.into(), state.into(), detail.map(str::to_string).into()],
    ))
    .await?;
    Ok(())
}

/// Place the cursor at `to` outside an event: its first placement, a
/// resync, or catching up past snapshots that changed nothing it watches.
/// Clears a `behind_horizon` or `broken` state only when `clear` says so.
pub async fn place(
    db: &DatabaseConnection,
    id: Uuid,
    from: Option<i64>,
    to: i64,
    clear: bool,
) -> Result<bool, DbErr> {
    let done = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_event_table_cursors \
                SET snapshot_id = $3, advanced_at = now(), updated_at = now(), \
                    state = CASE WHEN $4 THEN 'active' ELSE state END, \
                    state_detail = CASE WHEN $4 THEN NULL ELSE state_detail END \
              WHERE id = $1 AND snapshot_id IS NOT DISTINCT FROM $2",
            [id.into(), from.into(), to.into(), clear.into()],
        ))
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Whether some listener of the cursor's table in its environment would
/// receive a change: production always; elsewhere only while the
/// environment's events switch is on (`app_environments.events_enabled`).
pub async fn delivers(db: &DatabaseConnection, cursor: &cursors::Model) -> Result<bool, DbErr> {
    if cursor.environment == "production" {
        return Ok(true);
    }
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS on_ FROM app_event_subscriptions s \
               JOIN app_environments e ON e.app_id = s.app_id AND e.name = s.environment \
              WHERE s.org_id = $1 AND s.workspace_id = $2 AND s.environment = $3 \
                AND s.event_type = 'table:' || $4 AND e.events_enabled AND e.deleting_at IS NULL \
              LIMIT 1",
            [
                cursor.org_id.into(),
                cursor.workspace_id.into(),
                cursor.environment.clone().into(),
                cursor.table_ref.clone().into(),
            ],
        ))
        .await?;
    Ok(row.is_some())
}

/// Forget the cursor's place, so it is placed at the newest snapshot when its
/// environment next delivers.
pub async fn unplace(db: &DatabaseConnection, id: Uuid) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_table_cursors SET snapshot_id = NULL, updated_at = now() \
          WHERE id = $1 AND snapshot_id IS NOT NULL",
        [id.into()],
    ))
    .await?;
    Ok(())
}

/// Move the cursor to `to` whatever it was, clear its state and forget the
/// columns it holds as the contract (the next pass reads them afresh, so a
/// column an engineer removed on purpose stops being a break): `oxyc events
/// resync`.
pub async fn force_place(db: &DatabaseConnection, id: Uuid, to: i64) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_table_cursors \
            SET snapshot_id = $2, advanced_at = now(), updated_at = now(), \
                state = 'active', state_detail = NULL, table_columns = NULL, table_id = NULL \
          WHERE id = $1",
        [id.into(), to.into()],
    ))
    .await?;
    Ok(())
}

/// How far behind a cursor is, in snapshots, as of its last check.
pub fn lag(cursor: &cursors::Model) -> Option<i64> {
    Some((cursor.head_snapshot? - cursor.snapshot_id?).max(0))
}

/// When the cursor was created, on the platform's clock: the instant the
/// first placement looks for the newest snapshot at or before.
pub fn created(cursor: &cursors::Model) -> DateTime<Utc> {
    cursor.created_at.with_timezone(&Utc)
}
