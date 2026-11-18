//! What the host does for table events (app events design §7): the loop
//! guard on a write, the nudge after one, and `ctx.tables.changes`.
//!
//! **The guard.** A run started by a change to table T may not write T
//! (`table_watch::loop_guard`); [`ProjectFunctionHost::refuse_table_loop`]
//! asks before a `ctx.airhouse` or `ctx.warehouse` write is routed or sent.
//!
//! **The nudge.** After a write that landed in an Airhouse the host queues the
//! tracker for the workspace's watched tables
//! ([`ProjectFunctionHost::nudge_table_watch`]), at most once per invocation:
//! the first write already asks for every table changed so far, and the sweep
//! catches the rest.
//!
//! **`ctx.tables.changes(event)`** (the host op `events.changes`) reads, as
//! the platform, the rows the snapshot of *this run's own event* changed:
//! never another event's, never another table's, whatever the function asks.
//! Pages of at most [`MAX_PAGE`] rows a side; `hasMore` says to ask again.

use oxy_custom_apps::events::table::{TableRef, is_table_event_type};
use sea_orm::EntityTrait;
use serde_json::{Value, json};

use super::*;
use crate::app_events::table_watch::reader::{ReadError, reader};
use crate::app_events::table_watch::{loop_guard, nudge, track};

/// The most rows of each side one call returns.
pub(super) const MAX_PAGE: usize = 10_000;

/// A `ctx.tables.changes` from a run no table event started, or for another
/// event than the one that did.
const NOT_YOURS: &str = "TableChangesNotYours: ctx.tables.changes answers only inside a \
     function started by a table change, and only for that event";

fn page_arg(payload: &Value, field: &str, default: usize, max: usize) -> Result<usize, String> {
    match payload.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| (n as usize).min(max))
            .ok_or_else(|| format!("`{field}` must be a whole number, 0 or more")),
    }
}

impl ProjectFunctionHost {
    /// `Err` with the loop's error when this run was started by a change to a
    /// table that `sql` writes. `database`: the database the write names
    /// (`ctx.warehouse`), or `None` (`ctx.airhouse`).
    pub(super) fn refuse_table_loop(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<(), String> {
        let Some(cause) = &self.event_cause else {
            return Ok(());
        };
        let Some(watched) = TableRef::from_event_type(&cause.event_type) else {
            return Ok(());
        };
        let own = self.caps.airhouse.schema();
        let written = loop_guard::written_tables(sql);
        match loop_guard::refuse(&watched, database, own.as_deref(), &written) {
            Some(loop_error) => Err(loop_error),
            None => Ok(()),
        }
    }

    /// Queue the tracker for this workspace's watched tables, once per
    /// invocation. Best-effort: the sweep is the fallback.
    pub(super) async fn nudge_table_watch(&self) {
        if self
            .table_nudged
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        nudge(&self.db, self.org_id, self.project_id).await;
    }

    /// `events.changes`: one page of what this run's event changed.
    pub(super) async fn table_changes(&self, payload: Value) -> Result<Value, String> {
        if !crate::app_events::enabled_for(self.org_id) {
            return Err("EventsNotEnabled: app events are not enabled for this org".to_string());
        }
        let cause = self.event_cause.as_ref().ok_or(NOT_YOURS)?;
        let asked = payload.get("eventId").and_then(Value::as_str);
        if !is_table_event_type(&cause.event_type) || asked != Some(cause.id.to_string().as_str()) {
            return Err(NOT_YOURS.to_string());
        }
        let limit = page_arg(&payload, "limit", MAX_PAGE, MAX_PAGE)?;
        let offset = page_arg(&payload, "offset", 0, usize::MAX)?;
        let event = entity::app_events::Entity::find_by_id(cause.id)
            .one(&self.db)
            .await
            .map_err(|e| format!("could not read the event: {e}"))?
            .filter(|e| e.org_id == self.org_id)
            .ok_or("TableChangesUnavailable: the event is gone")?;
        let table = TableRef::from_event_type(&event.r#type).ok_or(NOT_YOURS)?;
        // A backfill event carries no snapshot of changes.
        let Some(snapshot) = event.key.rsplit_once(':').and_then(|(head, n)| {
            (!head.starts_with("backfill")).then(|| n.parse::<i64>().ok())?
        }) else {
            return Ok(json!({ "inserted": [], "deleted": [], "hasMore": false }));
        };
        let reader = reader();
        let (schema, columns) = track::watched_schema(
            reader.as_ref(),
            event.workspace_id,
            &event.environment,
            &table,
        )
        .await
        .map_err(|e| format!("TableChangesUnavailable: {e}"))?;
        if columns.is_none() {
            return Err(format!(
                "TableChangesUnavailable: table `{}.{}` is gone",
                table.schema, table.table
            ));
        }
        let page = reader
            .change_page(
                event.workspace_id,
                &schema,
                &table.table,
                snapshot,
                limit,
                offset,
            )
            .await
            .map_err(|e| match e {
                ReadError::SnapshotGone(_) => format!(
                    "TableChangesUnavailable: snapshot {snapshot} has been expired; its rows \
                     are no longer kept"
                ),
                other => format!("TableChangesUnavailable: {other}"),
            })?;
        serde_json::to_value(page).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_argument_defaults_caps_and_refuses_a_non_number() {
        let payload = json!({ "limit": 50_000, "offset": 3, "bad": "x" });
        assert_eq!(
            page_arg(&payload, "limit", MAX_PAGE, MAX_PAGE),
            Ok(MAX_PAGE)
        );
        assert_eq!(page_arg(&payload, "offset", 0, usize::MAX), Ok(3));
        assert_eq!(page_arg(&payload, "missing", 7, 10), Ok(7));
        assert!(page_arg(&payload, "bad", 0, 10).is_err());
        assert_eq!(page_arg(&json!({ "limit": null }), "limit", 9, 10), Ok(9));
    }
}
