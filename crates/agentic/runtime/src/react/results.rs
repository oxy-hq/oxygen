//! Query results live in task memory; the event log keeps only ids and SQL.
//!
//! A tool `put`s rows and hands the model a `result_id` (plus a [`Preview`]
//! if it wants one), then `persist`s it: with an archive attached
//! ([`ResultStore::attach_archive`]) the result is written to the host's
//! storage, so a table can page it later and a resumed run can read it back
//! ([`ResultStore::load`]). After a resume the rows are gone from memory but
//! the id and its SQL come back through [`ResultStore::restore`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agentic_core::react::ResultHandle;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use super::archive::{ArchiveKey, ArchivedResult, ColumnKind, ResultArchive, column_kinds};

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
    /// The rows held in memory: all of them until `persist`, then at most
    /// [`MEMORY_ROWS`]. The stored copy has them all.
    pub rows: Arc<Vec<Vec<Value>>>,
    /// How many rows the query returned, whatever memory holds.
    pub row_count: usize,
    /// Each column's kind, from every row the query returned, not only the
    /// ones memory holds.
    pub column_kinds: Vec<ColumnKind>,
    /// The connector stopped at its row cap (set by `persist`).
    pub truncated: bool,
}

/// The most rows a result keeps in memory once persisted: the answer's table
/// shows 50 and pages the rest from storage, a chart plots at most 500. A run
/// holds every result it made for its whole life, and a process drives many
/// runs, so the 10k a query may return stay in storage only.
pub const MEMORY_ROWS: usize = 1_000;

/// The first rows of a result, with the totals needed to say it is cut.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
    pub truncated: bool,
}

/// Where this run's results are written.
#[derive(Clone)]
struct Attached {
    store: Arc<dyn ResultArchive>,
    workspace_id: Uuid,
    run_id: String,
}

#[derive(Default)]
struct Inner {
    /// `None` rows = restored id whose rows are gone; only the SQL is known.
    entries: HashMap<String, (String, Option<Arc<StoredResult>>)>,
    next: u64,
    archive: Option<Attached>,
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
            column_kinds: column_kinds(columns.len(), &rows),
            columns,
            row_count: rows.len(),
            rows: Arc::new(rows),
            truncated: false,
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
        let row_count = r.row_count;
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

    /// Writes this run's results to `store` from now on, under
    /// `workspace_id` and `run_id`.
    pub fn attach_archive(&self, store: Arc<dyn ResultArchive>, workspace_id: Uuid, run_id: &str) {
        self.lock().archive = Some(Attached {
            store,
            workspace_id,
            run_id: run_id.to_string(),
        });
    }

    fn archive_key(&self, id: &str) -> Option<(Arc<dyn ResultArchive>, ArchiveKey)> {
        let a = self.lock().archive.clone()?;
        match ArchiveKey::new(a.workspace_id, &a.run_id, id) {
            Ok(key) => Some((a.store, key)),
            Err(e) => {
                tracing::warn!(result_id = id, "react result not archived: {e}");
                None
            }
        }
    }

    /// Result `id`, from memory or, for an id restored without rows, from the
    /// archive (kept in memory after). `None` when neither has it.
    pub async fn load(&self, id: &str) -> Option<Arc<StoredResult>> {
        if let Some(r) = self.get(id) {
            return Some(r);
        }
        let (store, key) = self.archive_key(id)?;
        let archived = match store.get(&key).await {
            Ok(found) => found?,
            Err(e) => {
                tracing::warn!(result_id = id, "react result not read back: {e}");
                return None;
            }
        };
        let stored = Arc::new(StoredResult {
            sql: archived.sql.clone(),
            column_kinds: column_kinds(archived.columns.len(), &archived.rows),
            columns: archived.columns,
            row_count: archived.rows.len(),
            rows: Arc::new(archived.rows.iter().take(MEMORY_ROWS).cloned().collect()),
            truncated: archived.truncated,
        });
        self.lock()
            .entries
            .insert(id.to_string(), (archived.sql, Some(stored.clone())));
        Some(stored)
    }

    /// Marks result `id` `truncated`, trims memory to [`MEMORY_ROWS`], and
    /// uploads every row in a task of its own, off the loop's path: the
    /// upload holds the full rows (shared, not copied) until it ends. A run
    /// that reads the result back before the upload lands finds it missing
    /// and shows the table's first page only.
    fn write(&self, id: &str, truncated: bool) {
        let Some(full) = self.get(id) else { return };
        let kept = Arc::new(StoredResult {
            sql: full.sql.clone(),
            columns: full.columns.clone(),
            rows: match full.rows.len() > MEMORY_ROWS {
                true => Arc::new(full.rows.iter().take(MEMORY_ROWS).cloned().collect()),
                false => full.rows.clone(),
            },
            row_count: full.row_count,
            column_kinds: full.column_kinds.clone(),
            truncated,
        });
        if let Some(entry) = self.lock().entries.get_mut(id) {
            entry.1 = Some(kept);
        }
        let Some((store, key)) = self.archive_key(id) else {
            return;
        };
        let archived = ArchivedResult {
            sql: full.sql.clone(),
            columns: full.columns.clone(),
            rows: full.rows.clone(),
            truncated,
            created_at: chrono::Utc::now(),
        };
        let id = id.to_string();
        agentic_core::hub_task::spawn_with_hub(async move {
            if let Err(e) = store.put(&key, &archived).await {
                tracing::warn!(result_id = id, "react result not archived: {e}");
            }
        });
    }
}

#[async_trait]
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
    async fn persist(&self, id: &str, truncated: bool) {
        self.write(id, truncated)
    }
}
