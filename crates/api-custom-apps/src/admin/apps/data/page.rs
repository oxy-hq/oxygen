//! One page of one table: what a request may ask for, the SQL that reads it,
//! and how each value comes back. All pure, so the caps are tested without a
//! database.
//!
//! **Caps, each enforced in SQL so a large value never leaves the database
//! whole:**
//!
//! - at most [`MAX_ROWS`] rows a page, fewer for a wide table: a page's
//!   worst case (every cell at its cap) stays under [`PAGE_BUDGET_BYTES`]
//!   ([`rows_per_page`]);
//! - a text value is cut at [`TEXT_CHARS`] characters (`left(…)`), and so is
//!   any other value's text form, JSON included;
//! - a `bytea` sends its first [`BYTES_SHOWN`] bytes (`substring(…)`) and its
//!   size, never its contents.
//!
//! **Names are the catalog's.** [`rows_sql`] takes a [`Table`] and
//! [`OrderTerm`]s built from it, so every identifier it writes came from
//! `pg_catalog`; it quotes each one anyway. The limit and offset are bound
//! parameters.

use serde::Serialize;

use super::super::environment_scope::ScopeError;
use super::catalog::{Column, ColumnKind, Table};
use super::error;

/// The most rows one page holds.
pub const MAX_ROWS: u32 = 200;
/// A page's size when the request names none.
pub const DEFAULT_ROWS: u32 = 50;
/// Where a value's text is cut, in characters.
pub const TEXT_CHARS: usize = 256;
/// How much of a `bytea` is sent, in bytes.
pub const BYTES_SHOWN: usize = 32;
/// A page's worst-case size: every cell at its cap.
pub const PAGE_BUDGET_BYTES: usize = 4 * 1024 * 1024;
/// One cell's worst case: `TEXT_CHARS + 1` characters (the extra one tells a
/// cut value from one that fits) of up to four UTF-8 bytes, and its size.
const CELL_MAX_BYTES: usize = (TEXT_CHARS + 1) * 4 + 16;

/// A sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Asc,
    Desc,
}

impl Dir {
    fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// What a request asked for, checked for shape only. The column it orders
/// by is checked against the table's catalog by [`order_terms`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// The page size asked for, clamped to `1..=MAX_ROWS`.
    pub limit: u32,
    pub offset: u32,
    pub order: Option<(String, Dir)>,
}

impl PageRequest {
    /// `limit` defaults to [`DEFAULT_ROWS`] and is clamped to [`MAX_ROWS`];
    /// `0` is refused. `dir` is `asc` or `desc` and needs `order`.
    pub fn parse(
        limit: Option<u32>,
        offset: Option<u32>,
        order: Option<&str>,
        dir: Option<&str>,
    ) -> Result<Self, ScopeError> {
        let limit = match limit {
            None => DEFAULT_ROWS,
            Some(0) => return Err(error::invalid_page("limit must be at least 1")),
            Some(n) => n.min(MAX_ROWS),
        };
        let dir = match dir.map(str::trim).filter(|d| !d.is_empty()) {
            None | Some("asc") => Dir::Asc,
            Some("desc") => Dir::Desc,
            Some(other) => {
                return Err(error::invalid_page(format!(
                    "dir must be \"asc\" or \"desc\", not {other:?}"
                )));
            }
        };
        let order = order
            .filter(|name| !name.is_empty())
            .map(|name| (name.to_string(), dir));
        Ok(Self {
            limit,
            offset: offset.unwrap_or(0),
            order,
        })
    }
}

/// How many rows a page of a `columns`-wide table holds: the request's
/// size, cut so that every cell at its cap still fits [`PAGE_BUDGET_BYTES`].
/// Never below one row.
pub fn rows_per_page(requested: u32, columns: usize) -> u32 {
    let per_row = columns.max(1) * CELL_MAX_BYTES;
    let fits = u32::try_from(PAGE_BUDGET_BYTES / per_row).unwrap_or(u32::MAX);
    requested.min(MAX_ROWS).min(fits.max(1))
}

/// One `ORDER BY` term: a column of the table, and its direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderTerm {
    pub column: String,
    pub dir: Dir,
}

/// The page's order: the column asked for, then the primary key as a tie
/// break so a page boundary is stable; the primary key alone when nothing is
/// asked; nothing for a table without one. A column the table does not have
/// is refused, whatever it names, and so is one Postgres cannot order
/// ([`Column::sortable`]) — a `json` column would fail the read with `42883`.
/// A key column Postgres cannot order is left out of the tie break.
pub fn order_terms(
    table: &Table,
    asked: Option<&(String, Dir)>,
) -> Result<Vec<OrderTerm>, ScopeError> {
    let mut terms = Vec::new();
    if let Some((name, dir)) = asked {
        let column = table
            .column(name)
            .ok_or_else(|| error::unknown_column(&table.name, name))?;
        if !column.sortable {
            return Err(error::unsortable_column(&table.name, column));
        }
        terms.push(OrderTerm {
            column: column.name.clone(),
            dir: *dir,
        });
    }
    for key in table.primary_key().filter(|key| key.sortable) {
        if !terms.iter().any(|t| t.column == key.name) {
            terms.push(OrderTerm {
                column: key.name.clone(),
                dir: Dir::Asc,
            });
        }
    }
    Ok(terms)
}

/// A Postgres identifier, double-quoted, with any `"` doubled.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A column's two select expressions: its value's capped text (`NULL` for a
/// `NULL`) and its full size in bytes where that is cheap to know.
fn select_exprs(column: &Column) -> [String; 2] {
    let c = quote_ident(&column.name);
    let cut = TEXT_CHARS + 1;
    match column.kind {
        ColumnKind::Bytes => [
            format!("pg_catalog.encode(pg_catalog.substring({c} FROM 1 FOR {BYTES_SHOWN}), 'hex')"),
            format!("pg_catalog.octet_length({c})::pg_catalog.int8"),
        ],
        ColumnKind::Text => [
            format!("pg_catalog.left({c}::pg_catalog.text, {cut})"),
            format!("pg_catalog.octet_length({c}::pg_catalog.text)::pg_catalog.int8"),
        ],
        ColumnKind::Json | ColumnKind::Other => [
            format!("pg_catalog.left({c}::pg_catalog.text, {cut})"),
            "NULL::pg_catalog.int8".to_string(),
        ],
    }
}

/// The page's SQL: two expressions per column (see [`select_exprs`]), from
/// `"schema"."table"`, in `order`, `LIMIT $1 OFFSET $2` (both `int8`).
pub fn rows_sql(schema: &str, table: &Table, order: &[OrderTerm]) -> String {
    let select = if table.columns.is_empty() {
        // A table with no columns still has rows; say so with a constant.
        "NULL::pg_catalog.text".to_string()
    } else {
        table
            .columns
            .iter()
            .flat_map(select_exprs)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let from = format!("{}.{}", quote_ident(schema), quote_ident(&table.name));
    let order_by = if order.is_empty() {
        String::new()
    } else {
        let terms = order
            .iter()
            .map(|t| format!("{} {}", quote_ident(&t.column), t.dir.sql()))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" ORDER BY {terms}")
    };
    format!("SELECT {select} FROM {from}{order_by} LIMIT $1 OFFSET $2")
}

/// One value on the wire.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Cell {
    /// SQL `NULL`, as JSON `null` — never the string `"NULL"`.
    Null,
    /// JSON that fit under the cap, as JSON.
    Json { json: serde_json::Value },
    /// A `bytea`: its first bytes in hex, and its size in bytes.
    Bytes {
        hex: String,
        size: i64,
        truncated: bool,
    },
    /// Anything else's text, cut at [`TEXT_CHARS`].
    Text {
        text: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
        /// The full value's size in bytes, when the value was cut and it is known.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<i64>,
    },
}

/// A value as the page SQL returned it — `preview` is `NULL` exactly when
/// the value is — turned into a [`Cell`].
pub fn cell(kind: ColumnKind, preview: Option<String>, size: Option<i64>) -> Cell {
    let Some(preview) = preview else {
        return Cell::Null;
    };
    if kind == ColumnKind::Bytes {
        let size = size.unwrap_or_default();
        return Cell::Bytes {
            truncated: usize::try_from(size).unwrap_or(usize::MAX) > BYTES_SHOWN,
            hex: preview,
            size,
        };
    }
    let cut = preview.chars().count() > TEXT_CHARS;
    if kind == ColumnKind::Json
        && !cut
        && let Ok(json) = serde_json::from_str(&preview)
    {
        return Cell::Json { json };
    }
    if !cut {
        return Cell::Text {
            text: preview,
            truncated: false,
            size: None,
        };
    }
    Cell::Text {
        text: preview.chars().take(TEXT_CHARS).collect(),
        truncated: true,
        size,
    }
}

/// The caps a response was read under, so the console can say what it cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Caps {
    pub max_rows: u32,
    pub text_chars: usize,
    pub bytes_shown: usize,
    pub statement_timeout_ms: u32,
}

impl Caps {
    pub const fn current() -> Self {
        Self {
            max_rows: MAX_ROWS,
            text_chars: TEXT_CHARS,
            bytes_shown: BYTES_SHOWN,
            statement_timeout_ms: super::read::STATEMENT_TIMEOUT_MS,
        }
    }
}
