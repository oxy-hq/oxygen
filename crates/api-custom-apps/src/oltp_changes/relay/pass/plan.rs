//! A batch's rows, planned table by table (step 5 of a batch, in the module
//! docs of `relay`). Pure: the caller reads the rows and the shapes, and
//! records the holds.
//!
//! - **Grouped by table**, each table planned from **every** shape recorded
//!   for it, not only the ones the batch's rows use: a rename between two
//!   shapes must not slip through as a new column.
//! - **A row with op `t` needs no shape.** A table without shapes (dropped,
//!   or no longer captured) plans from none when its rows are all truncates;
//!   any other row of it holds the table.
//! - **A table that cannot ship is refused whole**, with the reason it is
//!   held for, and its rows stay in the log: an [`Unsupported`] plan, a row
//!   without a shape for its `schema_version`, an op outside `i u d t`, an
//!   image that is not one, or a value [`insert_statements`] cannot render.
//!   The writer's other tables ship.
//! - **Each row's MAC is verified** (`oltp_changes::provenance`). A verified
//!   row ships its actor and invocation; any other ships as `unverified`,
//!   with no invocation.
//!
//! [`Unsupported`]: crate::oltp_changes::relay::table::Unsupported

use std::collections::{BTreeMap, HashMap};

use oxy_oltp::changes::LogRow;
use uuid::Uuid;

use crate::oltp_changes::provenance::ChangeSigner;
use crate::oltp_changes::relay::PRODUCTION;
use crate::oltp_changes::relay::render::{ShipRow, insert_statements, parse_image};
use crate::oltp_changes::relay::table::{ChangeTable, ShapeVersion};

/// The actor a change ships as when its MAC does not verify.
const UNVERIFIED: &str = "unverified";

/// Where a batch comes from: the bare writer name (`oltp_<writer>` in
/// Airhouse, the log's `writer`), its schema in the store (`app_<writer>`),
/// which the host signs, and the stream's generation.
#[derive(Debug, Clone, Copy)]
pub(super) struct Source<'a> {
    pub writer: &'a str,
    pub schema: &'a str,
    pub generation: i64,
}

/// A table ready to ship: its plan, its `INSERT`s (at least one, each of at
/// most [`INSERT_ROWS`](crate::oltp_changes::relay::INSERT_ROWS) rows) and
/// the `seq`s they write, every one of them.
#[derive(Debug)]
pub(super) struct Ready {
    pub table: ChangeTable,
    pub inserts: Vec<String>,
    pub seqs: Vec<i64>,
}

/// A table that cannot ship: held for `reason`, its `rows` left in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Refused {
    pub table: String,
    pub reason: String,
    pub rows: usize,
}

/// A batch, planned: what ships and what is held, each by table name.
#[derive(Debug, Default)]
pub(super) struct Planned {
    pub ready: Vec<Ready>,
    pub refused: Vec<Refused>,
}

/// Plan `rows`, one batch from `source`, against every shape recorded for
/// its tables. `verify` checks a row's MAC.
pub(super) fn plan(
    source: Source<'_>,
    rows: Vec<LogRow>,
    shapes: &HashMap<String, Vec<ShapeVersion>>,
    verify: &impl Fn(&ChangeSigner<'_>, Option<&[u8]>) -> bool,
) -> Planned {
    let mut by_table: BTreeMap<String, Vec<LogRow>> = BTreeMap::new();
    for row in rows {
        by_table
            .entry(row.table_name.clone())
            .or_default()
            .push(row);
    }
    let mut planned = Planned::default();
    for (table, rows) in by_table {
        let versions = shapes.get(&table).map(Vec::as_slice).unwrap_or_default();
        match plan_table(source, &table, &rows, versions, verify) {
            Ok(ready) => planned.ready.push(ready),
            Err(reason) => planned.refused.push(Refused {
                table,
                reason,
                rows: rows.len(),
            }),
        }
    }
    planned
}

/// One table's rows as its `INSERT`, or why the table is held.
fn plan_table(
    source: Source<'_>,
    table: &str,
    rows: &[LogRow],
    versions: &[ShapeVersion],
    verify: &impl Fn(&ChangeSigner<'_>, Option<&[u8]>) -> bool,
) -> Result<Ready, String> {
    for row in rows {
        op_name(row.op).ok_or_else(|| unknown_op(row))?;
        let shaped = versions
            .iter()
            .any(|v| v.schema_version == row.schema_version);
        if row.op != 't' && !shaped {
            return Err(format!(
                "seq {}: no recorded shape for schema_version {}",
                row.seq, row.schema_version
            ));
        }
    }
    let plan = ChangeTable::plan(table, versions).map_err(|e| e.to_string())?;
    let ship = rows
        .iter()
        .map(|row| ship_row(row, source, verify))
        .collect::<Result<Vec<_>, _>>()?;
    let inserts = insert_statements(source.writer, &plan, source.generation, &ship)
        .map_err(|e| e.to_string())?;
    // A table is ready only with `INSERT`s that write its `seq`s: they are
    // confirmed and deleted once the batch commits.
    if inserts.is_empty() {
        return Err("no rows to write".to_owned());
    }
    Ok(Ready {
        table: plan,
        inserts,
        seqs: rows.iter().map(|row| row.seq).collect(),
    })
}

/// The `_op` a log row's op ships as; `None` for one outside `i u d t`.
fn op_name(op: char) -> Option<&'static str> {
    match op {
        'i' => Some("insert"),
        'u' => Some("update"),
        'd' => Some("delete"),
        't' => Some("truncate"),
        _ => None,
    }
}

fn unknown_op(row: &LogRow) -> String {
    format!("seq {}: unknown op {:?}", row.seq, row.op)
}

/// `row` ready to render. An insert or an update carries the new row, a
/// delete the old one, each as its image; a truncate carries none.
fn ship_row(
    row: &LogRow,
    source: Source<'_>,
    verify: &impl Fn(&ChangeSigner<'_>, Option<&[u8]>) -> bool,
) -> Result<ShipRow, String> {
    let op = op_name(row.op).ok_or_else(|| unknown_op(row))?;
    let image = if row.op == 't' {
        None
    } else {
        let text = row
            .row_image
            .as_deref()
            .ok_or_else(|| format!("seq {}: {op} without a row image", row.seq))?;
        Some(parse_image(text).map_err(|e| format!("seq {}: {e}", row.seq))?)
    };
    let (actor, invocation) = attribution(row, source, verify);
    Ok(ShipRow {
        generation: source.generation,
        seq: row.seq,
        txid: row.txid,
        op,
        changed_at: row.changed_at,
        actor,
        invocation,
        schema_version: row.schema_version,
        image,
    })
}

/// Who made `row`: its actor and invocation when its MAC verifies for the
/// writer's schema in production, else [`UNVERIFIED`] and none.
fn attribution(
    row: &LogRow,
    source: Source<'_>,
    verify: &impl Fn(&ChangeSigner<'_>, Option<&[u8]>) -> bool,
) -> (String, Option<Uuid>) {
    if let Some(actor) = row.actor.as_deref() {
        let signer = ChangeSigner {
            txid: row.txid,
            environment: PRODUCTION,
            writer: source.schema,
            actor,
            invocation: row.invocation,
        };
        if verify(&signer, row.provenance.as_deref()) {
            return (actor.to_owned(), row.invocation);
        }
    }
    (UNVERIFIED.to_owned(), None)
}

#[cfg(test)]
mod tests;
