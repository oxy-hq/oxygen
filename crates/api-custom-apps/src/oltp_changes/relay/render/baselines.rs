//! The baseline markers and the current-state view (OLTP change stream design
//! §5.4).
//!
//! - **`_baselines`** holds one row per `b` (begin) or `c` (complete) marker of
//!   a table's baseline. The markers never go to the change table.
//! - **The view** `oltp_<writer>.<table>` is the table's current state: the
//!   latest change per key after the last completed baseline's begin, or after
//!   the last truncate since, whichever is later. It lists the latest shape's
//!   columns only, and its key is the one of the shape the last completed
//!   baseline copied: the window it reads is keyed by that key until the next
//!   baseline completes.
//! - **A baseline still copying shows through:** its snapshots are changes
//!   after the previous baseline's begin, so the view reads each as the key's
//!   latest image, true as of its `seq`, until the baseline completes and the
//!   view moves to it.

use chrono::{DateTime, SecondsFormat, Utc};
use oxy_cameras::airhouse::escape::sql_str;

use super::{BASELINES_TABLE, cast, changes_table_name, qualified, quote};
use crate::oltp_changes::relay::table::{DuckType, ShapeVersion};

/// Which end of a baseline a marker is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Begin,
    Complete,
}

impl MarkerKind {
    fn name(self) -> &'static str {
        match self {
            MarkerKind::Begin => "begin",
            MarkerKind::Complete => "complete",
        }
    }
}

/// One `b` or `c` log row, as `_baselines` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub table: String,
    pub seq: i64,
    pub kind: MarkerKind,
    /// The begin marker's `seq`: a begin's own, and the one a complete names.
    pub begin_seq: i64,
    pub schema_version: i32,
    pub marked_at: DateTime<Utc>,
}

const COLUMNS: &str = "table_name, generation, seq, marker, begin_seq, schema_version, marked_at";

/// The `_baselines` table's `CREATE`, made only when missing.
pub(super) fn baselines_ddl(writer: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {} (table_name VARCHAR NOT NULL, generation BIGINT NOT NULL, \
         seq BIGINT NOT NULL, marker VARCHAR NOT NULL, begin_seq BIGINT NOT NULL, \
         schema_version INTEGER NOT NULL, marked_at TIMESTAMPTZ NOT NULL)",
        qualified(writer, BASELINES_TABLE)
    )
}

/// `markers` of `generation` into `_baselines`, each only if its `(generation,
/// seq)` is not already there. Empty when there are none.
pub fn baselines_insert_sql(writer: &str, generation: i64, markers: &[Marker]) -> String {
    if markers.is_empty() {
        return String::new();
    }
    let target = qualified(writer, BASELINES_TABLE);
    let values = markers
        .iter()
        .map(|m| marker_values(generation, m))
        .collect::<Vec<_>>()
        .join(",\n  ");
    format!(
        "INSERT INTO {target} ({COLUMNS})\nSELECT * FROM (VALUES\n  {values}\n) v({COLUMNS})\n\
         WHERE NOT EXISTS (SELECT 1 FROM {target} b \
         WHERE b.generation = v.generation AND b.seq = v.seq)"
    )
}

fn marker_values(generation: i64, marker: &Marker) -> String {
    let marked_at = marker
        .marked_at
        .to_rfc3339_opts(SecondsFormat::Micros, false);
    let values = [
        cast(&sql_str(&marker.table), &DuckType::Varchar),
        cast(&generation.to_string(), &DuckType::BigInt),
        cast(&marker.seq.to_string(), &DuckType::BigInt),
        cast(&sql_str(marker.kind.name()), &DuckType::Varchar),
        cast(&marker.begin_seq.to_string(), &DuckType::BigInt),
        cast(&marker.schema_version.to_string(), &DuckType::Integer),
        cast(&sql_str(&marked_at), &DuckType::TimestampTz),
    ];
    format!("({})", values.join(", "))
}

/// The view's columns: the latest shape's captured names, in attribute order.
pub fn view_columns(latest: &ShapeVersion) -> Vec<String> {
    let mut captured: Vec<_> = latest.captured.iter().collect();
    captured.sort_by_key(|c| c.attnum);
    captured.into_iter().map(|c| c.name.clone()).collect()
}

/// `CREATE OR REPLACE VIEW` of `table`'s current state over its change table
/// and `_baselines`, with the latest shape's columns, one row per `key`: the
/// primary key of the shape the last completed baseline copied, which capture
/// requires to be non-empty. The view has no rows until a baseline completes.
pub fn view_ddl(writer: &str, table: &str, latest: &ShapeVersion, key: &[String]) -> String {
    debug_assert!(!key.is_empty(), "a keyless table is refused at capture");
    let view = qualified(writer, table);
    let baselines = qualified(writer, BASELINES_TABLE);
    let changes = qualified(writer, &changes_table_name(table));
    let name = sql_str(table);
    let listed = |prefix: &str, names: &[String]| {
        names
            .iter()
            .map(|n| format!("{prefix}{}", quote(n)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let columns = listed("c.", &view_columns(latest));
    let key = listed("c.", key);
    format!(
        "CREATE OR REPLACE VIEW {view} AS\n\
         WITH baseline AS (\n  \
         SELECT generation, begin_seq FROM {baselines}\n   \
         WHERE table_name = {name} AND marker = 'complete'\n   \
         ORDER BY generation DESC, begin_seq DESC LIMIT 1),\n\
         cut AS (\n  \
         SELECT b.generation, GREATEST(b.begin_seq, COALESCE(max(c._seq), b.begin_seq)) AS after_seq\n    \
         FROM baseline b LEFT JOIN {changes} c\n      \
         ON c._generation = b.generation AND c._seq > b.begin_seq AND c._op = 'truncate'\n    \
         GROUP BY b.generation, b.begin_seq)\n\
         SELECT {columns}\n  \
         FROM {changes} c JOIN cut\n    \
         ON c._generation = cut.generation AND c._seq > cut.after_seq\n \
         WHERE c._op <> 'truncate'\n\
         QUALIFY row_number() OVER (PARTITION BY {key} ORDER BY c._seq DESC) = 1 \
         AND c._op <> 'delete'"
    )
}

/// The shape version of `table`'s latest completed baseline in `_baselines`,
/// in the order the view picks it: one row, or none before its first.
pub fn latest_complete_sql(writer: &str, table: &str) -> String {
    format!(
        "SELECT schema_version FROM {} WHERE table_name = {} AND marker = 'complete' \
         ORDER BY generation DESC, begin_seq DESC LIMIT 1",
        qualified(writer, BASELINES_TABLE),
        sql_str(table)
    )
}
