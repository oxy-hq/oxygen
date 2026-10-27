//! DuckDB's own answer to "does this script only read?".
//!
//! [`super::read_only`] is a hand-written lexer, and a lexer can disagree with
//! DuckDB's grammar — a dollar quote, an identifier with a `$` in it — in a way
//! that hides a write; each fix of it can open another gap. So a script it
//! calls read-only stays on the shared catalog only when DuckDB, given the
//! exact text, also reports every statement in it as a `SELECT`. DuckDB
//! types `DESCRIBE`, `SUMMARIZE`, `SHOW` and `PRAGMA table_info` as queries;
//! `EXPLAIN`, a `PIVOT` (which can create an enum type first) and everything
//! else that is not a query go to the connector's private copy.
//!
//! The statements are extracted and **prepared — parsed and bound, never
//! executed** — on a [`Shadow`]: a separate in-memory database holding empty
//! tables with the shared catalog's names and columns, and the same aliases.
//! Binding there reads no file (a view over `read_csv_auto` would re-sniff the
//! file on every check), and nothing a statement could do reaches the shared
//! database, since it never runs anywhere.
//!
//! Through the C API: duckdb-rs exposes neither a raw connection nor a
//! prepared statement's type, and its `prepare` *executes* every statement
//! before the last. Every failure — the shadow could not be built, the text
//! does not parse, a statement does not bind — answers "not only reads",
//! which costs a private copy and never a leak.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::ptr;

use duckdb::ffi;

use super::TableInfo;
use super::TableSource;
use super::loading::{Aliases, aliases};
use crate::string_literal::StringLiteral;

/// An in-memory database with the shape of a shared directory database and
/// none of its data.
pub(super) struct Shadow {
    db: ffi::duckdb_database,
    /// `SET file_search_path`, so a query naming a file by its relative path
    /// binds as it does on the shared clone.
    search_path: String,
    /// The threads checks ran on, for a test to see they never take a
    /// runtime worker.
    #[cfg(test)]
    checked_on: std::sync::Mutex<Vec<std::thread::ThreadId>>,
}

// SAFETY: a `duckdb_database` may be shared between threads; DuckDB
// synchronises it internally. Every use here opens its own connection, and a
// connection is never shared.
unsafe impl Send for Shadow {}
unsafe impl Sync for Shadow {}

impl Shadow {
    /// The shadow of a database whose files registered as `tables`, under
    /// `dir`. `None` when DuckDB cannot open one; a table whose definition
    /// does not take is left out, so a query naming it does not bind and goes
    /// to the private copy.
    pub(super) fn build(tables: &[TableInfo], dir: &Path) -> Option<Self> {
        let db = open_single_threaded()?;
        let shadow = Shadow {
            db,
            search_path: format!(
                "SET file_search_path = {}",
                StringLiteral::Standard.quote(&dir.display().to_string())
            ),
            #[cfg(test)]
            checked_on: std::sync::Mutex::default(),
        };
        let conn = shadow.connect()?;
        conn.run("LOAD icu");
        for table in tables {
            for stmt in ddl(table) {
                if !conn.run(&stmt) {
                    tracing::debug!(table = %table.name, %stmt, "shadow: definition not taken");
                }
            }
        }
        drop(conn);
        Some(shadow)
    }

    /// Whether every statement in `sql` is a `SELECT`, by DuckDB's parser. A
    /// script with no statements runs nothing, so it only reads.
    ///
    /// Blocking: binding a query that names a file reads the file (the
    /// search path is the real directory), so callers on a runtime run this
    /// on the blocking pool.
    pub(super) fn only_selects(&self, sql: &str) -> bool {
        #[cfg(test)]
        self.checked_on
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        let Ok(text) = CString::new(sql) else {
            return false;
        };
        let Some(conn) = self.connect() else {
            return false;
        };
        conn.run(&self.search_path);
        let Some(extracted) = Extracted::of(&conn, &text) else {
            return false;
        };
        (0..extracted.count).all(|i| extracted.is_select(&conn, i))
    }

    #[cfg(test)]
    pub(super) fn checked_on(&self) -> Vec<std::thread::ThreadId> {
        self.checked_on.lock().unwrap().clone()
    }

    fn connect(&self) -> Option<RawConnection> {
        let mut con: ffi::duckdb_connection = ptr::null_mut();
        // SAFETY: `self.db` is open for as long as `self` lives.
        if unsafe { ffi::duckdb_connect(self.db, &mut con) } != ffi::DuckDBSuccess {
            return None;
        }
        Some(RawConnection(con))
    }
}

impl Drop for Shadow {
    fn drop(&mut self) {
        // SAFETY: opened in `build`, closed once; connections are dropped
        // before their calls return.
        unsafe { ffi::duckdb_close(&mut self.db) };
    }
}

/// An in-memory database with one thread: it only ever parses and binds, so
/// the worker threads DuckDB starts per database by default would idle for
/// as long as the cache entry lives.
fn open_single_threaded() -> Option<ffi::duckdb_database> {
    // SAFETY: the config is created, set and destroyed here, and DuckDB
    // copies it into the database it opens; a null path is in-memory, and the
    // error string (set only on failure) is freed with `duckdb_free`.
    unsafe {
        let mut config: ffi::duckdb_config = ptr::null_mut();
        if ffi::duckdb_create_config(&mut config) != ffi::DuckDBSuccess {
            return None;
        }
        let set = ffi::duckdb_set_config(config, c"threads".as_ptr(), c"1".as_ptr());
        let mut db: ffi::duckdb_database = ptr::null_mut();
        let mut error: *mut c_char = ptr::null_mut();
        let opened = set == ffi::DuckDBSuccess
            && ffi::duckdb_open_ext(ptr::null(), &mut db, config, &mut error) == ffi::DuckDBSuccess;
        if !error.is_null() {
            ffi::duckdb_free(error.cast());
        }
        ffi::duckdb_destroy_config(&mut config);
        opened.then_some(db)
    }
}

/// The empty table and the aliases standing in for one registered file.
fn ddl(table: &TableInfo) -> Vec<String> {
    let q = |s: &str| s.replace('"', "\"\"");
    let name = q(&table.name);
    let columns = table
        .columns
        .iter()
        .map(|(col, ty)| format!(r#""{}" {ty}"#, q(col)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = vec![format!(r#"CREATE TABLE "{name}" ({columns})"#)];
    if let TableSource::File { path, .. } = &table.source {
        let Aliases { full, stem, ext } = aliases(path);
        if let Some(full) = full.filter(|f| *f != name) {
            out.push(format!(r#"CREATE VIEW "{full}" AS SELECT * FROM "{name}""#));
        }
        out.push(format!(r#"CREATE SCHEMA IF NOT EXISTS "{stem}""#));
        out.push(format!(
            r#"CREATE OR REPLACE VIEW "{stem}"."{ext}" AS SELECT * FROM "{name}""#
        ));
    }
    out
}

/// One connection to a [`Shadow`], disconnected on drop.
struct RawConnection(ffi::duckdb_connection);

impl RawConnection {
    /// Run `sql`; whether it succeeded.
    fn run(&self, sql: &str) -> bool {
        let Ok(text) = CString::new(sql) else {
            return false;
        };
        // SAFETY: a zeroed `duckdb_result` is what `duckdb_query` fills, and
        // it is destroyed whether or not the query succeeded.
        unsafe {
            let mut result: ffi::duckdb_result = std::mem::zeroed();
            let ok = ffi::duckdb_query(self.0, text.as_ptr(), &mut result) == ffi::DuckDBSuccess;
            ffi::duckdb_destroy_result(&mut result);
            ok
        }
    }
}

impl Drop for RawConnection {
    fn drop(&mut self) {
        // SAFETY: connected in `Shadow::connect`, disconnected once.
        unsafe { ffi::duckdb_disconnect(&mut self.0) };
    }
}

/// A script split into statements by DuckDB's parser, destroyed on drop.
struct Extracted {
    handle: ffi::duckdb_extracted_statements,
    count: ffi::idx_t,
}

impl Extracted {
    /// `None` when the text does not parse.
    fn of(conn: &RawConnection, text: &CStr) -> Option<Self> {
        let mut handle: ffi::duckdb_extracted_statements = ptr::null_mut();
        // SAFETY: `text` is NUL-terminated and outlives the call; `handle` is
        // destroyed by `Drop` whatever the count.
        let count = unsafe {
            ffi::duckdb_extract_statements(conn.0, text.as_ptr() as *const c_char, &mut handle)
        };
        let extracted = Extracted { handle, count };
        // SAFETY: as above; the error is null when parsing succeeded.
        let parse_error = count == 0
            && !unsafe { ffi::duckdb_extract_statements_error(extracted.handle) }.is_null();
        (!parse_error).then_some(extracted)
    }

    /// Whether statement `index` binds, and is a `SELECT`.
    fn is_select(&self, conn: &RawConnection, index: ffi::idx_t) -> bool {
        let mut prepared: ffi::duckdb_prepared_statement = ptr::null_mut();
        // SAFETY: `index < self.count`; the prepared statement is destroyed
        // whether or not it prepared, as the C API requires.
        unsafe {
            let bound =
                ffi::duckdb_prepare_extracted_statement(conn.0, self.handle, index, &mut prepared)
                    == ffi::DuckDBSuccess;
            let select = bound
                && ffi::duckdb_prepared_statement_type(prepared)
                    == ffi::duckdb_statement_type_DUCKDB_STATEMENT_TYPE_SELECT;
            ffi::duckdb_destroy_prepare(&mut prepared);
            select
        }
    }
}

impl Drop for Extracted {
    fn drop(&mut self) {
        // SAFETY: filled by `duckdb_extract_statements`, destroyed once.
        unsafe { ffi::duckdb_destroy_extracted(&mut self.handle) };
    }
}

#[cfg(test)]
#[path = "parse_check_tests.rs"]
mod tests;
