//! A batch's Airhouse side, through the process's [`ChangeSink`]: the last
//! batch the writer's `_progress` holds (step 3), the DDL the batch needs
//! (step 6), its one DuckLake transaction (step 7), and the current-state
//! views made after it ([`view`]).

use std::collections::{BTreeMap, HashMap};

use super::plan::Ready;
use super::target::Target;
use crate::oltp_changes::relay::check::Shipped;
use crate::oltp_changes::relay::render::{
    changes_table_name, columns_sql, ensure_schema_ddl, latest_complete_sql, progress_read_sql,
    progress_write_sql, table_ddl, view_columns, view_ddl,
};
use crate::oltp_changes::relay::sink::{ChangeSink, SinkError};
use crate::oltp_changes::relay::table::ShapeVersion;

/// A table's current-state view to make or replace, and what decides its
/// key ([`view`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingView {
    pub table: String,
    /// The latest shape: the view's columns.
    pub latest: ShapeVersion,
    /// Each recorded shape version's primary key.
    pub keys: BTreeMap<i32, Vec<String>>,
    /// The shape version of the baseline the batch completes, if it does.
    pub completed: Option<i32>,
}

impl PendingView {
    /// `table`'s view from every shape `versions` records, completed by no
    /// batch: for a pass that renders a pending view again. `None` without a
    /// shape.
    pub(super) fn of(table: &str, versions: &[ShapeVersion]) -> Option<Self> {
        let latest = versions.iter().max_by_key(|v| v.schema_version)?.clone();
        Some(Self {
            table: table.to_owned(),
            latest,
            keys: versions
                .iter()
                .map(|v| (v.schema_version, v.primary_key.clone()))
                .collect(),
            completed: None,
        })
    }
}

/// The last batch Airhouse holds for `target`. `None` when its schema or its
/// `_progress` table does not exist yet: nothing was ever shipped there.
pub(super) async fn shipped(
    sink: &dyn ChangeSink,
    target: &Target,
) -> Result<Option<Shipped>, String> {
    let read = progress_read_sql(&target.name);
    let result = sink.query(target.workspace, &read).await;
    progress_from(result, &|e: &SinkError| sink.is_missing(e))
}

/// [`shipped`] from the progress read's result. `is_missing` is the sink's
/// [`ChangeSink::is_missing`]: only a statement's own error that its schema
/// or table does not exist is no progress, never a failure to connect.
fn progress_from(
    read: Result<Vec<Vec<Option<String>>>, SinkError>,
    is_missing: &dyn Fn(&SinkError) -> bool,
) -> Result<Option<Shipped>, String> {
    let doing = "read the writer's Airhouse progress";
    let rows = match read {
        Ok(rows) => rows,
        Err(e) if is_missing(&e) => return Ok(None),
        Err(e) => return Err(format!("{doing}: {e}")),
    };
    rows.first()
        .map(|row| Shipped::from_row(row.as_slice()))
        .transpose()
        .map_err(|e| format!("{doing}: {e}"))
}

/// Step 6: the writer's schema and `_progress`, then each ready table's change
/// table made or given its new columns, as Admin. Returns the current-state
/// views the batch needs made or replaced, for the caller to make one at a
/// time ([`view`]) once the batch's write has committed, so a view Airhouse
/// refuses never stops the batch, and none is replaced ahead of its rows.
pub(super) async fn ddl(
    sink: &dyn ChangeSink,
    target: &Target,
    ready: &[Ready],
) -> Result<Vec<PendingView>, String> {
    let listed = sink
        .query(target.workspace, &columns_sql(&target.name))
        .await
        .map_err(|e| format!("read the change tables' columns: {e}"))?;
    let existing = columns_by_table(listed);
    let statements = ddl_statements(&target.name, ready, &existing);
    sink.admin(target.workspace, &statements)
        .await
        .map_err(|e| format!("bring the change tables up to the batch: {e}"))?;
    Ok(view_statements(ready, &existing))
}

/// Make or replace `view`, as Admin, on its own. Its columns are the latest
/// shape's; its key is the key of the shape the latest complete baseline
/// copied, as Airhouse holds it after the batch: the batch's own, when it
/// completes one; else the latest in `_baselines` (one read, only here);
/// else, before any completes, the latest shape's.
pub(super) async fn view(
    sink: &dyn ChangeSink,
    target: &Target,
    view: &PendingView,
) -> Result<(), String> {
    let completed = match view.completed {
        Some(version) => Some(version),
        None => latest_complete(sink, target, &view.table).await?,
    };
    let key = completed
        .and_then(|version| view.keys.get(&version))
        .unwrap_or(&view.latest.primary_key);
    let statement = view_ddl(&target.name, &view.table, &view.latest, key);
    sink.admin(target.workspace, &[statement])
        .await
        .map_err(|e| format!("make the current-state view: {e}"))
}

/// The shape version of `table`'s latest complete baseline in Airhouse;
/// `None` before its first, or before `_baselines` exists.
async fn latest_complete(
    sink: &dyn ChangeSink,
    target: &Target,
    table: &str,
) -> Result<Option<i32>, String> {
    let doing = "read the table's latest complete baseline";
    let read = sink
        .query(target.workspace, &latest_complete_sql(&target.name, table))
        .await;
    let rows = match read {
        Ok(rows) => rows,
        Err(e) if sink.is_missing(&e) => return Ok(None),
        Err(e) => return Err(format!("{doing}: {e}")),
    };
    let cell = rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next());
    let Some(version) = cell.flatten() else {
        return Ok(None);
    };
    version
        .parse()
        .map(Some)
        .map_err(|e| format!("{doing}: {version:?}: {e}"))
}

/// The rows of `render::columns_sql`, by table.
fn columns_by_table(rows: Vec<Vec<Option<String>>>) -> HashMap<String, Vec<String>> {
    let mut columns: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        if let [Some(table), Some(column)] = row.as_slice() {
            columns
                .entry(table.clone())
                .or_default()
                .push(column.clone());
        }
    }
    columns
}

fn ddl_statements(
    writer: &str,
    ready: &[Ready],
    existing: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut statements = ensure_schema_ddl(writer);
    for table in ready {
        let listed = existing.get(&changes_table_name(&table.table.table));
        statements.extend(table_ddl(writer, &table.table, listed.map(Vec::as_slice)));
    }
    statements
}

/// The views the ready tables need made or replaced.
fn view_statements(ready: &[Ready], existing: &HashMap<String, Vec<String>>) -> Vec<PendingView> {
    ready
        .iter()
        .filter_map(|table| view_statement(table, existing))
        .collect()
}

/// The table's current-state view, when it follows the latest shape and
/// Airhouse's does not: made when missing, replaced when its columns differ
/// (compared without case, in order) or the batch completes a baseline, which
/// may follow a key change the columns do not show. Not at a begin: the view
/// still reads the previous baseline's window then, which only its own key
/// keeps one row per key. No view for a table without a shape.
fn view_statement(table: &Ready, existing: &HashMap<String, Vec<String>>) -> Option<PendingView> {
    let latest = table.latest.as_ref()?;
    let wanted = view_columns(latest);
    let current = existing.get(&table.table.table);
    let unchanged = current.is_some_and(|listed| {
        listed.len() == wanted.len()
            && listed
                .iter()
                .zip(&wanted)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    });
    (!unchanged || table.completes_baseline.is_some()).then(|| PendingView {
        table: table.table.table.clone(),
        latest: latest.clone(),
        keys: table.keys.clone(),
        completed: table.completes_baseline,
    })
}

/// Step 7: every ready table's rows, then `_progress` at `batch`, in one
/// transaction as Writer. Every ready table's `INSERT`s are sent, none
/// skipped: the `seq`s the batch then confirms and deletes are exactly those
/// these statements wrote.
pub(super) async fn write(
    sink: &dyn ChangeSink,
    target: &Target,
    generation: i64,
    batch: i64,
    ready: &[Ready],
) -> Result<(), String> {
    let statements = write_statements(&target.name, generation, batch, ready);
    sink.write(target.workspace, &statements)
        .await
        .map_err(|e| format!("write batch {batch} to Airhouse: {e}"))
}

fn write_statements(writer: &str, generation: i64, batch: i64, ready: &[Ready]) -> Vec<String> {
    let mut statements: Vec<String> = ready
        .iter()
        .flat_map(|table| table.inserts.iter().cloned())
        .collect();
    statements.extend(progress_write_sql(writer, generation, batch));
    statements
}

#[cfg(test)]
mod tests;
