//! Stored react results: what a `query` result `rN` keeps after its run, so a
//! table can page through it later (`internal-docs/agent-engine-runtime.md`,
//! "Results, tables and charts").
//!
//! The contract is here; the host implements the storage
//! (`oxy_workspace_runtime::react_results`). A result is kept for
//! [`RESULT_TTL`], enforced on read: an older one answers
//! [`ArchiveError::Expired`] with its SQL, to be re-run.
//!
//! Paging ([`page`]) is pure: it sorts the stored rows, breaking ties on row
//! order so pages never overlap or skip, and slices them.

use std::cmp::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// How long a stored result is served.
pub const RESULT_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The most rows one page carries.
pub const MAX_PAGE_ROWS: usize = 500;

/// Where one result is stored: a run's `rN` in a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveKey {
    workspace_id: Uuid,
    run_id: String,
    result_id: String,
}

impl ArchiveKey {
    /// Refuses anything but a UUID-shaped run id and an `r<digits>` result
    /// id, so a key built from a request names one file and nothing else.
    pub fn new(workspace_id: Uuid, run_id: &str, result_id: &str) -> Result<Self, ArchiveError> {
        let digits = result_id.strip_prefix('r').unwrap_or("");
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ArchiveError::BadRequest(format!(
                "`{result_id}` is not a result id"
            )));
        }
        if Uuid::parse_str(run_id).is_err() {
            return Err(ArchiveError::BadRequest(format!(
                "`{run_id}` is not a run id"
            )));
        }
        Ok(Self {
            workspace_id,
            run_id: run_id.to_string(),
            result_id: result_id.to_string(),
        })
    }

    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    /// `<workspace>/<run>/<rN>`: the storage key below the store's prefix.
    pub fn path(&self) -> String {
        format!("{}/{}/{}", self.workspace_id, self.run_id, self.result_id)
    }
}

/// A column's kind, from its values: `number` when every non-null value is a
/// number, else `text`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    Number,
    Text,
}

/// The kind of each column of `rows`.
pub fn column_kinds(width: usize, rows: &[Vec<Value>]) -> Vec<ColumnKind> {
    (0..width)
        .map(|i| {
            let mut cells = rows
                .iter()
                .filter_map(|r| r.get(i))
                .filter(|v| !v.is_null());
            match cells.all(Value::is_number) {
                true => ColumnKind::Number,
                false => ColumnKind::Text,
            }
        })
        .collect()
}

/// One stored result, as written once at query time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchivedResult {
    pub sql: String,
    pub columns: Vec<String>,
    /// Shared with the run's memory while it uploads: serialized through the
    /// `Arc`, never copied.
    #[serde(with = "shared_rows")]
    pub rows: std::sync::Arc<Vec<Vec<Value>>>,
    /// The connector stopped at its row cap: the query has more rows.
    pub truncated: bool,
    pub created_at: DateTime<Utc>,
}

impl ArchivedResult {
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        let ttl = chrono::Duration::from_std(RESULT_TTL).unwrap_or(chrono::TimeDelta::MAX);
        now - self.created_at > ttl
    }
}

/// Which rows to send: from `offset`, at most `limit`, optionally sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageRequest {
    pub offset: usize,
    pub limit: usize,
    /// The column to sort by, by its place: two columns can share a name.
    pub sort: Option<usize>,
    pub desc: bool,
}

/// One page of a stored result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResultPage {
    pub columns: Vec<String>,
    pub column_kinds: Vec<ColumnKind>,
    pub rows: Vec<Vec<Value>>,
    pub offset: usize,
    pub row_count: usize,
    pub truncated: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ArchiveError {
    #[error("{0}")]
    BadRequest(String),
    #[error("result not found")]
    NotFound,
    /// Older than [`RESULT_TTL`]; `sql` re-runs it.
    #[error("result expired")]
    Expired { sql: String },
    #[error("result storage: {0}")]
    Storage(String),
}

/// Where results are kept. The default host keeps none ([`NoArchive`]).
#[async_trait]
pub trait ResultArchive: Send + Sync {
    /// Stores `result` at `key`. An error means it was not stored.
    async fn put(&self, key: &ArchiveKey, result: &ArchivedResult) -> Result<(), ArchiveError>;
    /// The result at `key`, `None` when nothing is stored there.
    async fn get(&self, key: &ArchiveKey) -> Result<Option<ArchivedResult>, ArchiveError>;
}

/// Keeps nothing: a run without storage (`oxy test`, tests).
pub struct NoArchive;

#[async_trait]
impl ResultArchive for NoArchive {
    async fn put(&self, _: &ArchiveKey, _: &ArchivedResult) -> Result<(), ArchiveError> {
        Err(ArchiveError::Storage("this host keeps no results".into()))
    }
    async fn get(&self, _: &ArchiveKey) -> Result<Option<ArchivedResult>, ArchiveError> {
        Ok(None)
    }
}

/// The page `req` asks of `result`, read at `now`.
pub fn page(
    result: &ArchivedResult,
    req: &PageRequest,
    now: DateTime<Utc>,
) -> Result<ResultPage, ArchiveError> {
    if result.expired(now) {
        return Err(ArchiveError::Expired {
            sql: result.sql.clone(),
        });
    }
    let mut order: Vec<usize> = (0..result.rows.len()).collect();
    if let Some(col) = req.sort {
        if col >= result.columns.len() {
            return Err(ArchiveError::BadRequest(format!(
                "no column {col} to sort by; the result has {}",
                result.columns.len()
            )));
        }
        let cell = |i: usize| result.rows[i].get(col).unwrap_or(&Value::Null);
        // A stable sort keeps row order among equal cells: pages never overlap.
        order.sort_by(|&a, &b| {
            let o = compare(cell(a), cell(b));
            if req.desc { o.reverse() } else { o }
        });
    }
    let limit = req.limit.clamp(1, MAX_PAGE_ROWS);
    let rows = order
        .iter()
        .skip(req.offset)
        .take(limit)
        .map(|&i| result.rows[i].clone())
        .collect();
    Ok(ResultPage {
        column_kinds: column_kinds(result.columns.len(), &result.rows),
        columns: result.columns.clone(),
        rows,
        offset: req.offset,
        row_count: result.rows.len(),
        truncated: result.truncated,
        created_at: result.created_at,
    })
}

/// Nulls first, then numbers by value, then everything else by its text.
fn compare(a: &Value, b: &Value) -> Ordering {
    let rank = |v: &Value| match v {
        Value::Null => 0,
        Value::Number(_) => 1,
        _ => 2,
    };
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            x.total_cmp(&y)
        }
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ if rank(a) != rank(b) => rank(a).cmp(&rank(b)),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

/// `Arc<Vec<…>>` as a plain JSON array, without serde's `rc` feature.
mod shared_rows {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    pub fn serialize<S: Serializer>(rows: &Arc<Vec<Vec<Value>>>, s: S) -> Result<S::Ok, S::Error> {
        rows.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<Vec<Value>>>, D::Error> {
        Vec::<Vec<Value>>::deserialize(d).map(Arc::new)
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
