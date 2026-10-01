//! Comparing one table a transform build wrote with its live table (P2),
//! counts only. Every statement is a read ([`super::reads`]), sent through the
//! preview Airhouse connector on a Reader, with the overlay off so a live name
//! reads live.
//!
//! 1. **Shape**: both sides' columns from `information_schema.columns`.
//! 2. **Size and fingerprint**: `count(*)` and an order-independent
//!    `sum(hash(<common columns>))`, columns in name order, minus ignored
//!    ones (`_airway…` and the run's `ignore_columns`).
//! 3. **Difference**: only when the fingerprints differ and neither side is
//!    over `OXY_PREVIEW_DIFF_MAX_ROWS`, `EXCEPT ALL` both ways — no key join,
//!    since DuckLake has no keys, and `EXCEPT ALL` keeps duplicates honest.
//!    A partial copy (over the copy-on-write cap, it started empty) holds
//!    only what the preview wrote, so only `only_in_preview` means anything.
//!
//! What comes back is counts and column names — never a value from a row. A
//! failed read is reported in fixed words ([`ReadFailed`]); the engine's own
//! message, which can quote a row value, is never stored.

use agentic_connector::DatabaseConnector;
use serde::{Deserialize, Serialize};

use super::reads::{self, Columns};

pub const DIFF_MAX_ROWS_VAR: &str = "OXY_PREVIEW_DIFF_MAX_ROWS";
pub const DEFAULT_DIFF_MAX_ROWS: u64 = 2_000_000;

/// `OXY_PREVIEW_DIFF_MAX_ROWS`, or [`DEFAULT_DIFF_MAX_ROWS`] when unset. A
/// value that is not a row count is an error, not a silent default.
pub fn diff_max_rows() -> Result<u64, String> {
    match std::env::var(DIFF_MAX_ROWS_VAR)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        None | Some("") => Ok(DEFAULT_DIFF_MAX_ROWS),
        Some(v) => v
            .parse()
            .map_err(|_| format!("{DIFF_MAX_ROWS_VAR}={v:?} is not a row count")),
    }
}

/// How a compare reads.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Above this many rows on either side, no `EXCEPT ALL`.
    pub diff_max_rows: u64,
    /// Column names left out of the fingerprint and the difference, besides
    /// `_airway…` (lowercase).
    pub ignore_columns: Vec<String>,
    /// The workspace's DuckLake catalog, when named. Unnamed, nothing is
    /// filtered by catalog: over Airhouse's wire `current_database()` need not
    /// be the catalog the tables are in.
    pub catalog: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retyped {
    pub column: String,
    pub live: String,
    pub preview: String,
}

/// One table's outcome: counts and column names only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableCompare {
    /// The live name, `schema.table`.
    pub table: String,
    /// `None` when there is no live table (the build created it).
    pub live_rows: Option<u64>,
    pub preview_rows: Option<u64>,
    pub equal: bool,
    pub only_in_preview: Option<u64>,
    pub only_in_live: Option<u64>,
    pub columns_added: Vec<String>,
    pub columns_removed: Vec<String>,
    pub columns_retyped: Vec<Retyped>,
    pub partial: bool,
    /// The build dropped the table: the preview has none, live keeps its rows.
    #[serde(default)]
    pub dropped: bool,
    /// The preview already held this table before the build started (an
    /// earlier run or build wrote it), so the build may have read that copy
    /// rather than live.
    #[serde(default)]
    pub preexisting: bool,
    pub skipped_reason: Option<String>,
}

impl TableCompare {
    fn named(sides: &Sides) -> Self {
        Self {
            table: format!("{}.{}", sides.live.0, sides.live.1),
            live_rows: None,
            preview_rows: None,
            equal: false,
            only_in_preview: None,
            only_in_live: None,
            columns_added: vec![],
            columns_removed: vec![],
            columns_retyped: vec![],
            partial: sides.partial,
            dropped: sides.dropped,
            preexisting: sides.preexisting,
            skipped_reason: None,
        }
    }

    /// A table that could not be compared, and why (fixed words).
    pub fn skipped(sides: &Sides, why: ReadFailed) -> Self {
        Self {
            skipped_reason: Some(why.reason().into()),
            ..Self::named(sides)
        }
    }

    fn columns_changed(&self) -> bool {
        !self.columns_added.is_empty()
            || !self.columns_removed.is_empty()
            || !self.columns_retyped.is_empty()
    }
}

/// Which read failed. The only words a failure leaves in the outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFailed {
    Columns,
    Size,
    Difference,
}

impl ReadFailed {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Columns => "the table's columns could not be read",
            Self::Size => "the table could not be counted",
            Self::Difference => "the rows that differ could not be counted",
        }
    }
}

/// One table to compare: its live name, the preview schema standing in for
/// that schema, and what the preview's record says of it.
pub struct Sides {
    /// `(schema, table)` of the live table, lowercase.
    pub live: (String, String),
    pub preview_schema: String,
    pub partial: bool,
    pub dropped: bool,
    pub preexisting: bool,
}

const NO_PREVIEW_TABLE: &str = "the preview has no such table any more";
const DROPPED: &str = "the build dropped this table";
const NO_COMMON: &str = "the two sides have no column in common";
const PARTIAL_OVER: &str = "partial copy: diff skipped (the live table is over the copy-on-write \
                            cap, and over OXY_PREVIEW_DIFF_MAX_ROWS)";

/// Compare one table (module doc).
pub async fn compare_table(
    conn: &dyn DatabaseConnector,
    sides: &Sides,
    limits: &Limits,
) -> Result<TableCompare, ReadFailed> {
    let (schema, table) = &sides.live;
    let mut out = TableCompare::named(sides);
    let live = reads::columns(conn, schema, table, limits).await?;
    let live_name = reads::relation(schema, table, limits);
    if sides.dropped {
        if !live.is_empty() {
            out.live_rows = Some(reads::size(conn, &live_name, &[]).await?.0);
        }
        out.skipped_reason = Some(DROPPED.into());
        return Ok(out);
    }
    let preview = reads::columns(conn, &sides.preview_schema, table, limits).await?;
    if preview.is_empty() {
        out.skipped_reason = Some(NO_PREVIEW_TABLE.into());
        return Ok(out);
    }
    shape(&mut out, &live, &preview);
    let common = common_columns(&live, &preview, limits);
    let preview_name = reads::relation(&sides.preview_schema, table, limits);
    let (preview_rows, preview_fp) = reads::size(conn, &preview_name, &common).await?;
    out.preview_rows = Some(preview_rows);
    if live.is_empty() {
        (out.only_in_preview, out.only_in_live) = (Some(preview_rows), Some(0));
        return Ok(out);
    }
    let (live_rows, live_fp) = reads::size(conn, &live_name, &common).await?;
    out.live_rows = Some(live_rows);
    if common.is_empty() {
        out.skipped_reason = Some(NO_COMMON.into());
        return Ok(out);
    }
    let same = live_rows == preview_rows && live_fp == preview_fp;
    let pair = Pair {
        preview: &preview_name,
        live: &live_name,
        common: &common,
    };
    difference(conn, out, same, pair, limits).await
}

/// The two relations and the columns they are compared on.
struct Pair<'a> {
    preview: &'a str,
    live: &'a str,
    common: &'a [String],
}

/// Step 3: equal, over the limit, or the `EXCEPT ALL` counts.
async fn difference(
    conn: &dyn DatabaseConnector,
    mut out: TableCompare,
    same: bool,
    pair: Pair<'_>,
    limits: &Limits,
) -> Result<TableCompare, ReadFailed> {
    out.equal = same && !out.partial && !out.columns_changed();
    if same && !out.partial {
        (out.only_in_preview, out.only_in_live) = (Some(0), Some(0));
        return Ok(out);
    }
    let largest = out
        .live_rows
        .unwrap_or(0)
        .max(out.preview_rows.unwrap_or(0));
    if largest > limits.diff_max_rows {
        out.skipped_reason = Some(if out.partial {
            PARTIAL_OVER.to_string()
        } else {
            format!(
                "{largest} rows is over {DIFF_MAX_ROWS_VAR} ({}): sizes and fingerprints only",
                limits.diff_max_rows
            )
        });
        return Ok(out);
    }
    out.only_in_preview =
        Some(reads::except_all(conn, pair.preview, pair.live, pair.common).await?);
    if !out.partial {
        out.only_in_live =
            Some(reads::except_all(conn, pair.live, pair.preview, pair.common).await?);
    }
    Ok(out)
}

/// Added, removed and retyped columns, by name.
fn shape(out: &mut TableCompare, live: &Columns, preview: &Columns) {
    let find = |cols: &Columns, name: &str| cols.iter().find(|(n, _)| n == name).cloned();
    for (name, ty) in preview {
        match find(live, name) {
            None => out.columns_added.push(name.clone()),
            Some((_, live_ty)) if !live_ty.eq_ignore_ascii_case(ty) => {
                out.columns_retyped.push(Retyped {
                    column: name.clone(),
                    live: live_ty,
                    preview: ty.clone(),
                })
            }
            Some(_) => {}
        }
    }
    for (name, _) in live {
        if find(preview, name).is_none() {
            out.columns_removed.push(name.clone());
        }
    }
}

/// The columns both sides have, in name order, minus the ignored ones.
fn common_columns(live: &Columns, preview: &Columns, limits: &Limits) -> Vec<String> {
    let mut common: Vec<String> = preview
        .iter()
        .map(|(n, _)| n.clone())
        .filter(|n| live.iter().any(|(l, _)| l == n))
        .filter(|n| {
            let lower = n.to_ascii_lowercase();
            !lower.starts_with("_airway") && !limits.ignore_columns.contains(&lower)
        })
        .collect();
    common.sort();
    common
}
