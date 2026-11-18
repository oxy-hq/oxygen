//! One tracker pass over an (org, environment)'s cursors (app events design
//! §7.2): read each watched table's changes from its cursor to the newest
//! snapshot, publish one event per changed snapshot, move the cursor.
//!
//! Per cursor, in this order, each step stopping the pass for that cursor:
//!
//! 1. **The workspace's snapshots** ([`TableReader::snapshots`]). A workspace
//!    with no Airhouse is passed by.
//! 2. **Where the table is.** Outside production, the environment's own copy
//!    (`app_<writer>__<env>`) when it has the table, else production's,
//!    read-only (§7.3 "Environments"). Both share one snapshot sequence, so a
//!    cursor stays valid when the copy appears.
//! 3. **The columns are a contract.** What the last pass read is the contract;
//!    a column removed, renamed or retyped marks the cursor `broken` and pages
//!    in production, publishing nothing. A column added is fine. A `broken`
//!    cursor is read again every pass and goes `active` the moment the
//!    contract holds.
//! 4. **The retention horizon.** A cursor whose next snapshot has been expired
//!    is `behind_horizon` and pages; `oxyc events resync` is the way out.
//! 5. **The first placement.** An unplaced cursor goes to the snapshot that
//!    was newest when it was created: no replay of what came before the watch.
//! 6. **Publish** up to [`SNAPSHOTS_PER_PASS`] changed snapshots, oldest first
//!    ([`super::publish`]); the rest wait for the next pass, which the pass
//!    queues itself. A snapshot that changed nothing the table holds
//!    (compaction, other tables) is skipped by the cursor moving past it.

use entity::app_event_table_cursors as cursors;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::events::table::TableRef;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::cursors::{ACTIVE, BEHIND_HORIZON, BROKEN, Checked, place, record_check, set_state};
use super::publish::{PublishError, publish_change};
use super::reader::{Columns, ReadError, TableReader, broken_columns, reader};
use super::{Attention, breaker, page};
use airhouse::app_schema::environment_schema;

/// The most changed snapshots one pass publishes for one cursor.
pub const SNAPSHOTS_PER_PASS: usize = 50;

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackStats {
    pub published: usize,
    pub deliveries: usize,
    /// More changed snapshots wait: the next pass is queued by the caller.
    pub more: bool,
}

/// What one cursor's pass came to.
enum Outcome {
    Done,
    More,
}

/// Where `table` is read in `environment`: the environment's own copy of an
/// app's schema when it has the table, else the schema as written.
pub(crate) async fn watched_schema(
    reader: &dyn TableReader,
    workspace_id: Uuid,
    environment: &str,
    table: &TableRef,
) -> Result<(String, Option<Columns>), ReadError> {
    if let Some(label) = AppEnvironment::parse(environment).and_then(|e| e.schema_label())
        && let Some(sibling) = environment_schema(&table.schema, &label)
        && let Some(columns) = reader.columns(workspace_id, &sibling, &table.table).await?
    {
        return Ok((sibling, Some(columns)));
    }
    let columns = reader
        .columns(workspace_id, &table.schema, &table.table)
        .await?;
    Ok((table.schema.clone(), columns))
}

fn stored_contract(cursor: &cursors::Model) -> Option<Columns> {
    serde_json::from_value(cursor.table_columns.clone()?).ok()
}

/// The contract after this read: what was relied on plus any column added.
fn merged(stored: Option<Columns>, now: &Columns) -> Columns {
    let mut merged = stored.unwrap_or_default();
    for (name, kind) in now {
        merged.entry(name.clone()).or_insert_with(|| kind.clone());
    }
    merged
}

/// Run one pass over every cursor of `org_id` in `environment`.
pub async fn track_once(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<TrackStats, String> {
    if !super::super::enabled_for(org_id)
        || super::super::relay::relayed_store(environment).is_none()
    {
        return Ok(TrackStats::default());
    }
    let reader = reader();
    let mut stats = TrackStats::default();
    for cursor in super::cursors::of_environment(db, org_id, environment)
        .await
        .map_err(|e| e.to_string())?
    {
        match track_cursor(db, reader.as_ref(), &cursor, &mut stats).await {
            Ok(Outcome::Done) => {}
            Ok(Outcome::More) => stats.more = true,
            Err(e) => tracing::warn!(
                %org_id, environment, table = %cursor.table_ref, error = %e,
                "app events: a table pass failed; the cursor stays where it was"
            ),
        }
    }
    Ok(stats)
}

async fn track_cursor(
    db: &DatabaseConnection,
    reader: &dyn TableReader,
    cursor: &cursors::Model,
    stats: &mut TrackStats,
) -> Result<Outcome, String> {
    let db_err = |e: sea_orm::DbErr| e.to_string();
    let Some(table) = super::cursors::table_of(cursor) else {
        let detail = format!("`{}` is not a table name", cursor.table_ref);
        set_state(db, cursor.id, BROKEN, Some(&detail))
            .await
            .map_err(db_err)?;
        return Ok(Outcome::Done);
    };
    // Outside production a watch records nothing until a listener's events
    // switch is on, then starts from the snapshot current at that moment, so
    // production's churn never fills a sandbox with held rows.
    if !super::cursors::delivers(db, cursor).await.map_err(db_err)? {
        super::cursors::unplace(db, cursor.id)
            .await
            .map_err(db_err)?;
        return Ok(Outcome::Done);
    }
    // What the app's own `ctx.airhouse.query` could not read is never read by
    // the platform on its behalf, whatever put the watch there.
    if let Err(why) = super::declare::check_readable(&table) {
        set_state(db, cursor.id, BROKEN, Some(&why))
            .await
            .map_err(db_err)?;
        attention(cursor, Attention::Broken, &why, cursor.state == BROKEN);
        return Ok(Outcome::Done);
    }
    let ws = cursor.workspace_id;
    let range = match reader.snapshots(ws).await {
        Ok(range) => range,
        Err(ReadError::NotConfigured) => return Ok(Outcome::Done),
        Err(e) => return Err(e.to_string()),
    };
    let (schema, columns) = watched_schema(reader, ws, &cursor.environment, &table)
        .await
        .map_err(|e| e.to_string())?;
    let Some(columns) = columns else {
        return missing(db, cursor, &table).await;
    };
    let stored = stored_contract(cursor);
    let mut broken = stored
        .as_ref()
        .map(|c| broken_columns(c, &columns))
        .unwrap_or_default();
    // A `CREATE OR REPLACE` (or a drop and create) of the watched name is a new
    // table with a new id: `table_changes` would read only the new one, and the
    // old rows would never arrive as deletes. A contract break; resync moves
    // the watch onto the new id.
    let table_id = reader
        .table_id(ws, &schema, &table.table)
        .await
        .map_err(|e| e.to_string())?;
    // Ids are compared only for the table they were read from: outside
    // production a watch reads production's table until the environment's own
    // copy appears, and that copy is another table with another id, not a
    // replacement (`record_check` takes its id as the new baseline).
    if let (Some(was), Some(now)) = (cursor.table_id, table_id)
        && cursor.watched_schema.as_deref() == Some(schema.as_str())
        && was != now
    {
        broken.push(format!(
            "the table was replaced (id {was} is now {now}): what changed across the \
             replacement cannot be delivered; run `oxyc events resync`"
        ));
    }
    let contract = serde_json::to_value(merged(stored, &columns)).unwrap_or(Value::Null);
    if !broken.is_empty() {
        let detail = broken.join("; ");
        record_check(
            db,
            cursor.id,
            &Checked {
                head: range.newest,
                schema: &schema,
                columns: &contract,
                table_id,
                state: BROKEN,
                detail: Some(&detail),
            },
        )
        .await
        .map_err(db_err)?;
        attention(cursor, Attention::Broken, &detail, cursor.state == BROKEN);
        return Ok(Outcome::Done);
    }
    // A cursor that was broken and holds the contract again resumes.
    let resumed = cursor.state == BROKEN;
    let state = if cursor.state == BEHIND_HORIZON {
        BEHIND_HORIZON
    } else {
        ACTIVE
    };
    let detail = (state == BEHIND_HORIZON)
        .then_some(cursor.state_detail.as_deref())
        .flatten();
    record_check(
        db,
        cursor.id,
        &Checked {
            head: range.newest,
            schema: &schema,
            columns: &contract,
            table_id,
            state,
            detail,
        },
    )
    .await
    .map_err(db_err)?;
    if resumed {
        tracing::info!(table = %cursor.table_ref, "app events: a table's columns hold the contract again");
    }
    if state == BEHIND_HORIZON {
        return Ok(Outcome::Done);
    }
    let Some(from) = placed(db, reader, cursor).await? else {
        return Ok(Outcome::Done);
    };
    if from + 1 < range.oldest {
        return horizon(db, cursor, from, range.oldest).await;
    }
    publish_range(
        db,
        reader,
        cursor,
        &table,
        &schema,
        (from, range.newest),
        stats,
    )
    .await
}

/// The cursor's snapshot, placing it first when it has none. `None` when it
/// could not be placed this pass.
async fn placed(
    db: &DatabaseConnection,
    reader: &dyn TableReader,
    cursor: &cursors::Model,
) -> Result<Option<i64>, String> {
    if let Some(at) = cursor.snapshot_id {
        return Ok(Some(at));
    }
    let ws = cursor.workspace_id;
    // Production: from when the watch was declared. Elsewhere: from now (the
    // switch was off until a moment ago, and nothing before it was recorded).
    let declared = if cursor.environment == "production" {
        reader
            .snapshot_at(ws, super::cursors::created(cursor))
            .await
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    let at = match declared {
        Some(at) => at,
        None => {
            reader
                .snapshots(ws)
                .await
                .map_err(|e| e.to_string())?
                .newest
        }
    };
    let moved = place(db, cursor.id, None, at, false)
        .await
        .map_err(|e| e.to_string())?;
    Ok(moved.then_some(at))
}

async fn publish_range(
    db: &DatabaseConnection,
    reader: &dyn TableReader,
    cursor: &cursors::Model,
    table: &TableRef,
    schema: &str,
    (from, head): (i64, i64),
    stats: &mut TrackStats,
) -> Result<Outcome, String> {
    if from >= head {
        return Ok(Outcome::Done);
    }
    let ws = cursor.workspace_id;
    let changed = match reader
        .changed_snapshots(ws, schema, &table.table, from, head)
        .await
    {
        Ok(changed) => changed,
        Err(ReadError::SnapshotGone(_)) => return horizon(db, cursor, from, head).await,
        Err(ReadError::TableMissing(_)) => return missing(db, cursor, table).await,
        Err(e) => return Err(e.to_string()),
    };
    let more = changed.len() > SNAPSHOTS_PER_PASS;
    let mut at = from;
    for snapshot in changed.into_iter().take(SNAPSHOTS_PER_PASS) {
        pause_runaways(db, cursor, table).await;
        let rows = match reader.changes_at(ws, schema, &table.table, snapshot).await {
            Ok(rows) => rows,
            Err(ReadError::SnapshotGone(_)) => return horizon(db, cursor, at, snapshot).await,
            Err(e) => return Err(e.to_string()),
        };
        match publish_change(db, cursor, table, Some(at), snapshot, &rows).await {
            Ok(done) => {
                stats.published += 1;
                stats.deliveries += done.deliveries;
                at = snapshot;
            }
            Err(PublishError::Raced) => return Ok(Outcome::Done),
            Err(PublishError::NoStore) => {
                let why = "the environment has no OLTP branch to hold the change yet";
                set_state(db, cursor.id, ACTIVE, Some(why))
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(Outcome::Done);
            }
            Err(PublishError::Failed(e)) => return Err(e),
        }
    }
    if more {
        return Ok(Outcome::More);
    }
    // Past the last change, to what was the newest: nothing between changed
    // the table (compaction, other tables, DDL).
    place(db, cursor.id, Some(at), head, false)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Outcome::Done)
}

/// Pause any subscription that is over the breaker's rate, and page for each.
async fn pause_runaways(db: &DatabaseConnection, cursor: &cursors::Model, table: &TableRef) {
    let tripped = breaker::trip(
        db,
        cursor.org_id,
        cursor.workspace_id,
        &cursor.environment,
        &table.event_type(),
    )
    .await;
    match tripped {
        Ok(paused) => {
            for t in paused {
                let detail = format!(
                    "{}'s function `{}` received {} deliveries within {} minutes",
                    t.app_id,
                    t.function_name,
                    t.deliveries,
                    breaker::WINDOW.as_secs() / 60
                );
                attention(cursor, Attention::Paused, &detail, false);
            }
        }
        Err(e) => tracing::warn!(error = %e, "app events: the table breaker could not be read"),
    }
}

async fn horizon(
    db: &DatabaseConnection,
    cursor: &cursors::Model,
    from: i64,
    oldest: i64,
) -> Result<Outcome, String> {
    let detail = format!(
        "the cursor is at snapshot {from} and snapshot {oldest} is the oldest Airhouse still \
         holds: what changed between is gone; run `oxyc events resync`"
    );
    set_state(db, cursor.id, BEHIND_HORIZON, Some(&detail))
        .await
        .map_err(|e| e.to_string())?;
    attention(
        cursor,
        Attention::BehindHorizon,
        &detail,
        cursor.state == BEHIND_HORIZON,
    );
    Ok(Outcome::Done)
}

/// A table that is not there. One that never was is waited for; one the
/// contract knew is broken.
async fn missing(
    db: &DatabaseConnection,
    cursor: &cursors::Model,
    table: &TableRef,
) -> Result<Outcome, String> {
    if cursor.table_columns.is_some() {
        let detail = format!("table `{}.{}` is gone", table.schema, table.table);
        set_state(db, cursor.id, BROKEN, Some(&detail))
            .await
            .map_err(|e| e.to_string())?;
        attention(cursor, Attention::Broken, &detail, cursor.state == BROKEN);
    } else {
        let waiting = "the table does not exist yet";
        set_state(db, cursor.id, ACTIVE, Some(waiting))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(Outcome::Done)
}

fn attention(cursor: &cursors::Model, what: Attention, detail: &str, repeat: bool) {
    page(
        &cursor.environment,
        cursor.org_id,
        &cursor.table_ref,
        what,
        detail,
        repeat,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[(&str, &str)]) -> Columns {
        names
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect()
    }

    /// What was relied on is kept as it was; a column added joins it, so a
    /// later removal of that column is a break too.
    #[test]
    fn the_contract_keeps_what_was_read_and_gains_what_was_added() {
        let stored = columns(&[("id", "INTEGER"), ("amt", "INTEGER")]);
        let now = columns(&[("id", "INTEGER"), ("amt", "INTEGER"), ("extra", "TEXT")]);
        let merged = merged(Some(stored), &now);
        assert_eq!(merged, now);
        assert_eq!(
            super::merged(None, &now),
            now,
            "the first read is the contract"
        );
    }

    #[test]
    fn a_pass_is_bounded() {
        assert_eq!(SNAPSHOTS_PER_PASS, 50);
    }
}
