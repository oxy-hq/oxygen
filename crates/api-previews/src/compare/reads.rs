//! The reads a compare sends ([`super::table`]). Each failure is returned as a
//! fixed [`ReadFailed`] and logged by class only: an engine's message can
//! quote a row value (`Could not convert string '…'`), and neither the stored
//! outcome nor the platform's logs may carry one.

use agentic_connector::DatabaseConnector;
use agentic_core::result::CellValue;

use super::table::{Limits, ReadFailed};

/// `(column, type)` in ordinal order.
pub(super) type Columns = Vec<(String, String)>;

/// `schema.table`'s columns, `[]` when there is no such table. Filtered by
/// catalog only when the workspace names one ([`Limits::catalog`]).
pub(crate) async fn columns(
    conn: &dyn DatabaseConnector,
    schema: &str,
    table: &str,
    limits: &Limits,
) -> Result<Columns, ReadFailed> {
    let in_catalog = limits.catalog.as_deref().map_or(String::new(), |c| {
        format!("table_catalog = {} AND ", literal(c))
    });
    let sql = format!(
        "SELECT column_name, data_type FROM information_schema.columns \
         WHERE {in_catalog}lower(table_schema) = {} AND lower(table_name) = {} \
         ORDER BY ordinal_position",
        literal(&schema.to_ascii_lowercase()),
        literal(&table.to_ascii_lowercase())
    );
    let result = conn
        .execute_query(&sql, 10_000)
        .await
        .map_err(|e| failed(ReadFailed::Columns, schema, table, &e))?;
    Ok(result
        .result
        .rows
        .iter()
        .filter_map(|row| match (row.0.first(), row.0.get(1)) {
            (Some(CellValue::Text(name)), Some(CellValue::Text(ty))) => {
                Some((name.clone(), ty.clone()))
            }
            _ => None,
        })
        .collect())
}

/// `count(*)` and the fingerprint of `common` (as text: a `HUGEINT` sum does
/// not survive a float). No common column: no fingerprint.
pub(super) async fn size(
    conn: &dyn DatabaseConnector,
    relation: &str,
    common: &[String],
) -> Result<(u64, Option<String>), ReadFailed> {
    let fp = if common.is_empty() {
        "NULL".to_string()
    } else {
        format!(
            "CAST(sum(hash({})::HUGEINT) AS VARCHAR)",
            column_list(common)
        )
    };
    let sql = format!("SELECT count(*) AS n, {fp} AS fp FROM {relation}");
    let fail = |e: &dyn std::fmt::Display| failed(ReadFailed::Size, relation, "", e);
    let result = conn.execute_query(&sql, 1).await.map_err(|e| fail(&e))?;
    let row = result.result.rows.first().ok_or_else(|| fail(&"no row"))?;
    let n = row
        .0
        .first()
        .map(number)
        .transpose()
        .map_err(|e| fail(&e))?;
    let fp = match row.0.get(1) {
        Some(CellValue::Text(t)) => Some(t.clone()),
        Some(CellValue::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Ok((n.unwrap_or(0), fp))
}

/// Rows of `left` not in `right`, over `common`, duplicates counted.
pub(super) async fn except_all(
    conn: &dyn DatabaseConnector,
    left: &str,
    right: &str,
    common: &[String],
) -> Result<u64, ReadFailed> {
    let cols = column_list(common);
    let sql = format!(
        "SELECT count(*) FROM (SELECT {cols} FROM {left} EXCEPT ALL SELECT {cols} FROM {right})"
    );
    let fail = |e: &dyn std::fmt::Display| failed(ReadFailed::Difference, left, "", e);
    let result = conn.execute_query(&sql, 1).await.map_err(|e| fail(&e))?;
    result
        .result
        .rows
        .first()
        .and_then(|r| r.0.first())
        .map(number)
        .transpose()
        .map_err(|e| fail(&e))?
        .ok_or_else(|| fail(&"no row"))
}

/// Log a failed read by its class, and return the fixed failure.
fn failed(
    what: ReadFailed,
    relation: &str,
    table: &str,
    error: &dyn std::fmt::Display,
) -> ReadFailed {
    tracing::warn!(target: "preview", relation, table, class = %error_class(&error.to_string()),
        "preview compare: {}", what.reason());
    what
}

/// An engine error's class: the text before the first quote (where a value
/// would start), at most 120 characters. `query failed: Conversion Error:
/// Could not convert string` — never the string.
pub(crate) fn error_class(message: &str) -> String {
    message
        .split(['\'', '"', '`'])
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}

fn number(cell: &CellValue) -> Result<u64, &'static str> {
    match cell {
        CellValue::Number(n) if *n >= 0.0 => Ok(*n as u64),
        CellValue::Text(t) => t.trim().parse().map_err(|_| "not a count"),
        CellValue::Null => Ok(0),
        CellValue::Number(_) => Err("not a count"),
    }
}

/// `schema.table`, in the workspace's catalog when it names one.
pub(super) fn relation(schema: &str, table: &str, limits: &Limits) -> String {
    let catalog = limits
        .catalog
        .as_deref()
        .map_or(String::new(), |c| format!("{}.", ident(c)));
    format!("{catalog}{}.{}", ident(schema), ident(table))
}

fn column_list(columns: &[String]) -> String {
    columns
        .iter()
        .map(|c| ident(c))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
