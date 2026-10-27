//! Registering a directory's CSV/Parquet files with a DuckDB database.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use duckdb::Connection;

use super::{
    LoadStrategy, TableInfo, TableSource, ensure_icu, normalize_table_name, schema::describe_table,
};
use crate::{connector::ConnectorError, string_literal::StringLiteral};

/// Where the objects registered for a connector's files live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Catalog {
    /// The connection's temp catalog, which no other connection sees.
    Temp,
    /// The database's catalog, which every `try_clone()` of the connection
    /// sees — what a cached directory database needs.
    Database,
}

pub(super) fn open_in_memory() -> Result<Connection, ConnectorError> {
    Connection::open_in_memory().map_err(|e| ConnectorError::ConnectionError(e.to_string()))
}

/// Shrinks `conn`'s database to at most `max` worker threads, fewer on a
/// smaller machine. A database left at DuckDB's default still answers, so a
/// failure is only logged.
pub(super) fn bound_threads(conn: &Connection, max: usize, dir: &Path) {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cores.min(max);
    if let Err(e) = conn.execute_batch(&format!("SET threads = {threads}")) {
        tracing::warn!(
            dir = %dir.display(),
            error = %e,
            "DuckDB: could not bound a directory database's threads"
        );
    }
}

/// An in-memory database with `dir`'s files registered in `catalog`.
pub(super) fn open_directory(
    dir: &Path,
    strategy: LoadStrategy,
    catalog: Catalog,
) -> Result<(Connection, Vec<TableInfo>), ConnectorError> {
    let conn = open_in_memory()?;
    // Set file_search_path so queries referencing CSV/Parquet filenames
    // (e.g. `FROM 'cardio_4_4.csv'`) resolve to this directory.
    if let Ok(abs_dir) = dir.canonicalize() {
        set_file_search_path(&conn, &abs_dir);
    }
    let files = directory_candidates(dir)?;
    let file_refs: Vec<(&Path, LoadStrategy)> =
        files.iter().map(|p| (p.as_path(), strategy)).collect();
    let loaded_tables = register_files(&conn, &file_refs, catalog)?;
    Ok((conn, loaded_tables))
}

/// The `*.csv` and `*.parquet` files in `dir`, sorted, keeping only the
/// Parquet file when two share a stem.
pub(super) fn directory_candidates(dir: &Path) -> Result<Vec<PathBuf>, ConnectorError> {
    // stem → (path, is_parquet)
    let mut candidates: HashMap<String, (PathBuf, bool)> = HashMap::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ConnectorError::ConnectionError(format!("cannot read directory: {e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if ext != "csv" && ext != "parquet" {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let is_parquet = ext == "parquet";
        candidates
            .entry(stem)
            .and_modify(|e| {
                if is_parquet {
                    *e = (path.clone(), true);
                }
            })
            .or_insert((path, is_parquet));
    }

    let mut files: Vec<PathBuf> = candidates.into_values().map(|(p, _)| p).collect();
    files.sort();
    Ok(files)
}

/// Best-effort, like the directory registration it serves.
fn set_file_search_path(conn: &Connection, abs_dir: &Path) {
    let sql = format!(
        "SET file_search_path = {}",
        StringLiteral::Standard.quote(&abs_dir.display().to_string())
    );
    let _ = conn.execute_batch(&sql);
}

/// The per-connection state a clone of a directory database does not
/// inherit from the connection it was cloned from.
pub(super) fn apply_session_setup(conn: &Connection, abs_dir: &Path) {
    set_file_search_path(conn, abs_dir);
    ensure_icu(conn);
}

/// Register each file as a view or table in `catalog` — under its normalized
/// stem, its full file name and `"<stem>"."<ext>"` — and describe it.
pub(super) fn register_files(
    conn: &Connection,
    files: &[(&Path, LoadStrategy)],
    catalog: Catalog,
) -> Result<Vec<TableInfo>, ConnectorError> {
    ensure_icu(conn);
    let mut loaded_tables: Vec<TableInfo> = Vec::with_capacity(files.len());
    for (path, strategy) in files {
        loaded_tables.push(register_file(conn, path, *strategy, catalog)?);
    }
    Ok(loaded_tables)
}

fn register_file(
    conn: &Connection,
    path: &Path,
    strategy: LoadStrategy,
    catalog: Catalog,
) -> Result<TableInfo, ConnectorError> {
    let temp = match catalog {
        Catalog::Temp => "TEMP ",
        Catalog::Database => "",
    };
    let abs = path.canonicalize().map_err(|e| {
        ConnectorError::ConnectionError(format!("cannot resolve {}: {e}", path.display()))
    })?;
    let raw_stem = abs
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed");
    let name = normalize_table_name(raw_stem);
    let Aliases {
        full: full_name,
        stem: stem_q,
        ext: ext_q,
    } = aliases(&abs);
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let path_str = abs.display().to_string().replace('\'', "''");
    let src_expr = match ext.as_str() {
        "parquet" => format!("read_parquet('{path_str}')"),
        "csv" => format!("read_csv_auto('{path_str}')"),
        _ => format!("'{path_str}'"),
    };

    let create_sql = match strategy {
        LoadStrategy::View => {
            format!(r#"CREATE OR REPLACE {temp}VIEW "{name}" AS SELECT * FROM {src_expr}"#)
        }
        LoadStrategy::Materialized => {
            format!(r#"CREATE OR REPLACE {temp}TABLE "{name}" AS SELECT * FROM {src_expr}"#)
        }
    };
    conn.execute_batch(&create_sql)
        .map_err(|e| ConnectorError::query_failed(create_sql.clone(), e.to_string()))?;

    // Also expose the file under its full name (e.g. `oxymart.csv`) so
    // semantic-model views that declare `table: "oxymart.csv"` resolve
    // without falling through to DuckDB's file-replacement scan, which
    // does not honor `file_search_path` for quoted identifiers.
    if let Some(full) = full_name.as_deref()
        && full != name
    {
        let alias_sql =
            format!(r#"CREATE OR REPLACE {temp}VIEW "{full}" AS SELECT * FROM "{name}""#);
        conn.execute_batch(&alias_sql)
            .map_err(|e| ConnectorError::query_failed(alias_sql.clone(), e.to_string()))?;
    }

    // Also register a schema-qualified alias `"<stem>"."<ext>"`. The
    // semantic-model compiler renders `table: stores.parquet` as an
    // UNQUOTED `FROM stores.parquet`, which DuckDB parses as
    // schema.table; the single-identifier alias above only matches the
    // quoted form, so the unquoted reference otherwise depends on the
    // file-replacement scan (environment-sensitive; silently empty for
    // Parquet in some setups). The qualified view makes it resolve to a
    // real catalog object. Mirrors the oxy-core local pool registration.
    // It was always in the database catalog, whatever `catalog` says.
    let _ = conn.execute_batch(&format!(r#"CREATE SCHEMA IF NOT EXISTS "{stem_q}""#));
    let qualified_sql =
        format!(r#"CREATE OR REPLACE VIEW "{stem_q}"."{ext_q}" AS SELECT * FROM "{name}""#);
    let _ = conn.execute_batch(&qualified_sql);

    let columns = describe_table(conn, &name)
        .map_err(|e| ConnectorError::query_failed(format!("DESCRIBE \"{name}\""), e.to_string()))?;

    Ok(TableInfo {
        name,
        columns,
        source: TableSource::File {
            path: abs,
            strategy,
        },
    })
}

/// The names besides its normalized one that a file is registered under —
/// its full file name, and the `"<stem>"."<ext>"` pair — each escaped for a
/// quoted identifier. One definition for the registration and for
/// `parse_check`'s shadow of it.
pub(super) struct Aliases {
    pub full: Option<String>,
    pub stem: String,
    pub ext: String,
}

pub(super) fn aliases(abs: &Path) -> Aliases {
    let quoted = |s: &str| s.replace('"', "\"\"");
    Aliases {
        full: abs.file_name().and_then(|s| s.to_str()).map(quoted),
        stem: quoted(
            abs.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed"),
        ),
        ext: quoted(
            &abs.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default(),
        ),
    }
}
