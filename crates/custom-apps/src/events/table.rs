//! Table changes as events, the pure side (app events design §7): what
//! `on: { "table": "<database>:<schema>.<table>" }` names, the event type and
//! source it produces, and the `data` of one such event.
//!
//! Nothing here reads a database. The tracker that reads DuckLake snapshots,
//! and the host helper `ctx.tables.changes`, are in `oxy-api-custom-apps`.

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{MAX_PAYLOAD_BYTES, jsonb_text_len};

/// The prefix of a table event's type: `table:pokehouse:bookkeeping.lines`.
pub const TABLE_TYPE_PREFIX: &str = "table:";
/// The most rows an event carries inline, inserted and deleted together.
pub const INLINE_MAX_ROWS: usize = 1_000;
/// The longest any one name (database, schema or table) may be.
const MAX_NAME_LEN: usize = 63;

/// A watched table, as a function's `on` wrote it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableRef {
    /// The workspace database the table is in (`pokehouse`); `None` for the
    /// app's own `app_<writer>` schema, which the workspace's Airhouse holds.
    pub database: Option<String>,
    pub schema: String,
    pub table: String,
}

fn valid_database(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    s.len() <= MAX_NAME_LEN
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl TableRef {
    /// Read `[<database>:]<schema>.<table>`. Case is kept as written: the
    /// catalog the names are looked up in decides what matches.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let bad = |why: &str| {
            format!(
                "`on`: table `{spec}` {why}; write `<database>:<schema>.<table>` (the database \
                 may be left out for the app's own `app_<writer>` schema)"
            )
        };
        let (database, rest) = match spec.split_once(':') {
            Some((db, rest)) if valid_database(db) => (Some(db.to_string()), rest),
            Some(_) => return Err(bad("has no valid database name")),
            None => (None, spec),
        };
        let (schema, table) = rest
            .split_once('.')
            .ok_or_else(|| bad("names no schema and table"))?;
        if !valid_identifier(schema) || !valid_identifier(table) {
            return Err(bad(
                "has a schema or table that is not a plain name (letters, digits, underscores; \
                 not starting with a digit)",
            ));
        }
        Ok(Self {
            database,
            schema: schema.to_string(),
            table: table.to_string(),
        })
    }

    /// The table as written: `pokehouse:bookkeeping.lines` or `app_x.lines`.
    pub fn spec(&self) -> String {
        match &self.database {
            Some(db) => format!("{db}:{}.{}", self.schema, self.table),
            None => format!("{}.{}", self.schema, self.table),
        }
    }

    /// The type its events carry: `table:<spec>`.
    pub fn event_type(&self) -> String {
        format!("{TABLE_TYPE_PREFIX}{}", self.spec())
    }

    /// The envelope's `source`: `/tables/<spec>`.
    pub fn source(&self) -> String {
        format!("/tables/{}", self.spec())
    }

    /// The table a `table:` event type names; `None` for any other type.
    pub fn from_event_type(event_type: &str) -> Option<Self> {
        Self::parse(event_type.strip_prefix(TABLE_TYPE_PREFIX)?).ok()
    }
}

/// Whether `event_type` is a table event's.
pub fn is_table_event_type(event_type: &str) -> bool {
    event_type.starts_with(TABLE_TYPE_PREFIX)
}

/// The tables a function watches: the `{ "table": "..." }` entries of its
/// `on` key, in order and without repeats. Strings in `on` are event types and
/// are [`super::catalog::function_subscribes`]'s.
pub fn function_table_watches(function_manifest: &Value) -> Result<Vec<TableRef>, String> {
    let Some(on) = function_manifest.get("on") else {
        return Ok(Vec::new());
    };
    let entries: Vec<&Value> = match on {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut watches: Vec<TableRef> = Vec::new();
    for entry in entries {
        let Value::Object(object) = entry else {
            continue;
        };
        if let Some(key) = object.keys().find(|k| k.as_str() != "table") {
            return Err(format!(
                "`on`: `{key}` is not a key of a table entry; write {{ \"table\": \"<database>:<schema>.<table>\" }}"
            ));
        }
        let spec = object
            .get("table")
            .and_then(Value::as_str)
            .ok_or("`on`: a table entry needs a `table` string")?;
        let table = TableRef::parse(spec)?;
        if !watches.contains(&table) {
            watches.push(table);
        }
    }
    Ok(watches)
}

/// One row, as the event carries it: column name to JSON value.
pub type Row = Map<String, Value>;

/// The rows a snapshot changed: both sides, with the totals the snapshot
/// changed whether or not the rows are carried.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changed {
    pub inserted: u64,
    pub deleted: u64,
    /// Updates: each is an `update_preimage` and an `update_postimage` of one
    /// row, counted once here and once in each of `inserted` and `deleted`.
    pub updated: u64,
    /// `None` when the rows were not read (too many to carry).
    pub rows: Option<(Vec<Row>, Vec<Row>)>,
}

/// The `data` of a table event: `{ snapshotId, inserted, deleted, rows? }`.
/// `inserted` and `deleted` are counts. `rows` (`{ inserted: [...], deleted:
/// [...] }`) is there when the change is at most [`INLINE_MAX_ROWS`] rows and
/// the whole `data` fits [`MAX_PAYLOAD_BYTES`]; past either the handler reads
/// the rows with `ctx.tables.changes(event)`. An update arrives as a delete
/// plus an insert.
pub fn change_data(snapshot_id: i64, changed: &Changed) -> Value {
    let mut data = json!({
        "snapshotId": snapshot_id,
        "inserted": changed.inserted,
        "deleted": changed.deleted,
        "updated": changed.updated,
    });
    let small = changed.inserted.saturating_add(changed.deleted) <= INLINE_MAX_ROWS as u64;
    if let (true, Some((inserted, deleted))) = (small, &changed.rows) {
        let mut with_rows = data.clone();
        with_rows["rows"] = json!({ "inserted": inserted, "deleted": deleted });
        if jsonb_text_len(&with_rows) <= MAX_PAYLOAD_BYTES {
            data = with_rows;
        }
    }
    data
}

/// The `data` of the one synthetic event `oxyc events resync --backfill`
/// queues: no rows, `backfill: true`.
pub fn backfill_data(snapshot_id: i64) -> Value {
    json!({ "snapshotId": snapshot_id, "inserted": 0, "deleted": 0, "updated": 0, "backfill": true })
}

/// What `ctx.tables.changes` answers: one page of each side.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePage {
    pub inserted: Vec<Row>,
    pub deleted: Vec<Row>,
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_is_database_schema_and_table() {
        let t = TableRef::parse("pokehouse:bookkeeping.platform_report_lines").unwrap();
        assert_eq!(t.database.as_deref(), Some("pokehouse"));
        assert_eq!(t.schema, "bookkeeping");
        assert_eq!(t.table, "platform_report_lines");
        assert_eq!(
            t.event_type(),
            "table:pokehouse:bookkeeping.platform_report_lines"
        );
        assert_eq!(
            t.source(),
            "/tables/pokehouse:bookkeeping.platform_report_lines"
        );
        assert_eq!(TableRef::from_event_type(&t.event_type()), Some(t));
    }

    #[test]
    fn the_database_may_be_left_out() {
        let t = TableRef::parse("app_bookkeeping.lines").unwrap();
        assert_eq!(t.database, None);
        assert_eq!(t.spec(), "app_bookkeeping.lines");
    }

    #[test]
    fn a_malformed_table_is_refused_with_the_shape() {
        for bad in [
            "lines", "a.b.c", ":s.t", "db:s", "db:1s.t", "db:s.t t", "db:s.", "s-x.t",
        ] {
            let err = TableRef::parse(bad).unwrap_err();
            assert!(err.contains("<database>:<schema>.<table>"), "{bad}: {err}");
        }
        assert_eq!(
            TableRef::from_event_type("locations/statement.uploaded"),
            None
        );
    }

    #[test]
    fn on_collects_table_entries_beside_event_types() {
        let m = json!({ "on": ["loc/x.y", { "table": "db:s.t" }, { "table": "db:s.t" }] });
        let watches = function_table_watches(&m).unwrap();
        assert_eq!(watches.len(), 1, "a repeat is one watch");
        assert_eq!(watches[0].spec(), "db:s.t");
        let one = json!({ "on": { "table": "app_x.t" } });
        assert_eq!(function_table_watches(&one).unwrap().len(), 1);
        assert!(
            function_table_watches(&json!({ "on": "loc/x" }))
                .unwrap()
                .is_empty()
        );
        assert!(function_table_watches(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn a_table_entry_refuses_other_keys_and_bad_names() {
        assert!(function_table_watches(&json!({ "on": { "table": "x" } })).is_err());
        let err = function_table_watches(&json!({ "on": { "table": "a.b", "where": "1=1" } }))
            .unwrap_err();
        assert!(err.contains("`where`"), "{err}");
        assert!(function_table_watches(&json!({ "on": {} })).is_err());
    }

    fn row(i: i64) -> Row {
        let mut r = Row::new();
        r.insert("id".into(), json!(i));
        r
    }

    #[test]
    fn a_small_change_carries_its_rows() {
        let changed = Changed {
            inserted: 2,
            deleted: 1,
            updated: 0,
            rows: Some((vec![row(1), row(2)], vec![row(3)])),
        };
        let data = change_data(41, &changed);
        assert_eq!(data["snapshotId"], 41);
        assert_eq!(data["inserted"], 2);
        assert_eq!(data["rows"]["deleted"][0]["id"], 3);
    }

    #[test]
    fn a_change_over_a_thousand_rows_carries_none() {
        let changed = Changed {
            inserted: 600,
            deleted: 401,
            updated: 0,
            rows: Some((vec![row(1)], vec![row(2)])),
        };
        assert!(change_data(1, &changed).get("rows").is_none());
        let at_cap = Changed {
            inserted: 600,
            deleted: 400,
            updated: 0,
            rows: Some((vec![row(1)], vec![row(2)])),
        };
        assert!(change_data(1, &at_cap).get("rows").is_some());
    }

    #[test]
    fn a_change_over_the_payload_limit_carries_none() {
        let mut fat = Row::new();
        fat.insert("blob".into(), json!("x".repeat(MAX_PAYLOAD_BYTES)));
        let changed = Changed {
            inserted: 1,
            deleted: 0,
            updated: 0,
            rows: Some((vec![fat], vec![])),
        };
        let data = change_data(1, &changed);
        assert!(data.get("rows").is_none());
        assert!(jsonb_text_len(&data) <= MAX_PAYLOAD_BYTES);
    }

    #[test]
    fn a_backfill_carries_the_flag_and_no_rows() {
        let data = backfill_data(9);
        assert_eq!(data["backfill"], true);
        assert_eq!(data["inserted"], 0);
        assert!(data.get("rows").is_none());
    }
}
