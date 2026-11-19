//! Query results live in task memory; the event log keeps only ids and SQL.
//!
//! A tool `put`s rows and hands the model a `result_id` (plus a [`Preview`]
//! if it wants one). After a resume the rows are gone but the id and its SQL
//! come back through [`ResultStore::restore`]; the Phase 3 `query` tool
//! re-executes the SQL when it needs the rows again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agentic_core::react::ResultHandle;
use serde_json::Value;

/// Default preview row cap. There is no named preview constant in
/// `agentic-analytics` (`QueryExecuted` carries all rows, fetched up to
/// `DEFAULT_SAMPLE_LIMIT` = 1000 in `solver/executing/mod.rs`; its tracing
/// preview is 5), so this is the plan's example cap.
pub const DEFAULT_PREVIEW_ROWS: usize = 100;

/// The part of a `ToolReply.value` that may reach the event log: exactly
/// `{result_id, sql}` when both are strings, else `Null`. Rows and previews
/// never persist (full rows live only in task memory).
pub(super) fn persistable_value(value: &Value) -> Value {
    match (
        value.get("result_id").and_then(Value::as_str),
        value.get("sql").and_then(Value::as_str),
    ) {
        (Some(id), Some(sql)) => serde_json::json!({"result_id": id, "sql": sql}),
        _ => Value::Null,
    }
}

/// One stored query result.
#[derive(Debug)]
pub struct StoredResult {
    pub sql: String,
    pub columns: Vec<String>,
    pub rows: Arc<Vec<Vec<Value>>>,
}

/// The first rows of a result, with the totals needed to say it is cut.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
    pub truncated: bool,
}

#[derive(Default)]
struct Inner {
    /// `None` rows = restored id whose rows are gone; only the SQL is known.
    entries: HashMap<String, (String, Option<Arc<StoredResult>>)>,
    next: u64,
}

/// Cheap to clone; clones share one store.
#[derive(Clone, Default)]
pub struct ResultStore {
    inner: Arc<Mutex<Inner>>,
}

impl ResultStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // No await is held across the lock, so poisoning means a panic in a
        // trivial critical section; the map is still consistent.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Stores a result, returning `r1`, `r2`, ... in insertion order.
    pub fn put(&self, sql: String, columns: Vec<String>, rows: Vec<Vec<Value>>) -> String {
        let mut g = self.lock();
        g.next += 1;
        let id = format!("r{}", g.next);
        let stored = Arc::new(StoredResult {
            sql: sql.clone(),
            columns,
            rows: Arc::new(rows),
        });
        g.entries.insert(id.clone(), (sql, Some(stored)));
        id
    }

    /// The full result, or `None` for an unknown or restored-without-rows id.
    pub fn get(&self, id: &str) -> Option<Arc<StoredResult>> {
        self.lock().entries.get(id)?.1.clone()
    }

    /// The first `max_rows` rows; `None` when the rows are not in memory.
    pub fn preview(&self, id: &str, max_rows: usize) -> Option<Preview> {
        let r = self.get(id)?;
        let row_count = r.rows.len();
        Some(Preview {
            columns: r.columns.clone(),
            rows: r.rows.iter().take(max_rows).cloned().collect(),
            row_count,
            truncated: row_count > max_rows,
        })
    }

    /// [`preview`](Self::preview) at [`DEFAULT_PREVIEW_ROWS`].
    pub fn default_preview(&self, id: &str) -> Option<Preview> {
        self.preview(id, DEFAULT_PREVIEW_ROWS)
    }

    /// Registers an id whose rows are missing. `put` never reuses it: the
    /// counter moves past the highest restored `r<N>`.
    pub fn restore(&self, id: &str, sql: String) {
        let mut g = self.lock();
        if let Some(n) = id.strip_prefix('r').and_then(|n| n.parse::<u64>().ok()) {
            g.next = g.next.max(n);
        }
        g.entries.entry(id.to_string()).or_insert((sql, None));
    }

    pub fn sql_for(&self, id: &str) -> Option<String> {
        self.lock().entries.get(id).map(|e| e.0.clone())
    }
}

impl ResultHandle for ResultStore {
    fn put(&self, sql: String, columns: Vec<String>, rows: Vec<Vec<Value>>) -> String {
        ResultStore::put(self, sql, columns, rows)
    }
    fn rows(&self, id: &str) -> Option<Arc<Vec<Vec<Value>>>> {
        self.get(id).map(|r| r.rows.clone())
    }
    fn sql_for(&self, id: &str) -> Option<String> {
        ResultStore::sql_for(self, id)
    }
    fn restore(&self, id: &str, sql: String) {
        ResultStore::restore(self, id, sql)
    }
}
