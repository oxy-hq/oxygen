//! A batch's Airhouse side, through the process's [`ChangeSink`]: the last
//! batch the writer's `_progress` holds (step 3), the DDL the batch needs
//! (step 6), and its one DuckLake transaction (step 7).

use std::collections::HashMap;

use super::plan::Ready;
use super::target::Target;
use crate::oltp_changes::relay::check::Shipped;
use crate::oltp_changes::relay::render::{
    changes_table_name, columns_sql, ensure_schema_ddl, progress_read_sql, progress_write_sql,
    table_ddl,
};
use crate::oltp_changes::relay::sink::{ChangeSink, SinkError};

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
/// table made or given its new columns, as Admin.
pub(super) async fn ddl(
    sink: &dyn ChangeSink,
    target: &Target,
    ready: &[Ready],
) -> Result<(), String> {
    let listed = sink
        .query(target.workspace, &columns_sql(&target.name))
        .await
        .map_err(|e| format!("read the change tables' columns: {e}"))?;
    let statements = ddl_statements(&target.name, ready, &columns_by_table(listed));
    sink.admin(target.workspace, &statements)
        .await
        .map_err(|e| format!("bring the change tables up to the batch: {e}"))
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
