//! Per-token usage: counted in memory, written once a minute (design §3.7,
//! layer 3 — "usage, not one row per request").
//!
//! Every request a key or token authenticated adds one [`UsageSample`] to an
//! in-process accumulator keyed by `(token_id, UTC day)`. [`flush`] drains it
//! into `api_token_usage_daily` with an additive upsert, so several pods
//! flushing the same token on the same day sum rather than overwrite. The
//! whole batch goes in one statement, not one per row.
//!
//! **Best-effort by design.** A lost minute of counts is acceptable; a failed
//! flush is logged and its rows are put back for the next tick. Nothing here
//! can fail a request.
//!
//! **Bounded.** Entries live for at most one flush interval, so the map holds
//! one row per token active in the last minute (two around midnight UTC).
//! If flushes keep failing it stops growing at [`MAX_PENDING_ROWS`] and drops
//! new tokens' samples instead. Every stored string is truncated.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, NaiveDate, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement, Value};
use uuid::Uuid;

/// How long a usage row is kept.
pub const USAGE_RETENTION_DAYS: i64 = 400;
/// How often the flusher runs.
pub const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
/// Rows held while the database is unreachable, before new keys are dropped.
const MAX_PENDING_ROWS: usize = 20_000;
const MAX_IP_CHARS: usize = 64;
const MAX_USER_AGENT_CHARS: usize = 256;
const MAX_ROUTE_CHARS: usize = 256;

/// One finished request made with a token.
#[derive(Clone, Debug)]
pub struct UsageSample {
    pub token_id: Uuid,
    pub status: u16,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// The matched route template (`/api/{workspace_id}/sql/query`). `None`
    /// when nothing was routed — the raw path is never a substitute.
    pub route: Option<String>,
    pub at: DateTime<Utc>,
}

/// What one `(token, day)` has accumulated since the last flush.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageRow {
    pub token_id: Uuid,
    pub day: NaiveDate,
    pub requests: i64,
    pub errors_4xx: i64,
    pub errors_5xx: i64,
    pub last_ip: Option<String>,
    pub last_user_agent: Option<String>,
    pub last_route: Option<String>,
    pub last_seen_at: DateTime<Utc>,
}

fn bounded(value: Option<String>, max_chars: usize) -> Option<String> {
    value.map(|v| v.chars().take(max_chars).collect())
}

impl UsageRow {
    fn empty(token_id: Uuid, day: NaiveDate, at: DateTime<Utc>) -> Self {
        Self {
            token_id,
            day,
            requests: 0,
            errors_4xx: 0,
            errors_5xx: 0,
            last_ip: None,
            last_user_agent: None,
            last_route: None,
            last_seen_at: at,
        }
    }

    fn add(&mut self, sample: UsageSample) {
        self.requests += 1;
        match sample.status {
            400..=499 => self.errors_4xx += 1,
            500..=599 => self.errors_5xx += 1,
            _ => {}
        }
        // "Last" follows the newest request, whatever order samples arrive in.
        if sample.at >= self.last_seen_at {
            self.last_seen_at = sample.at;
            self.last_ip = bounded(sample.ip, MAX_IP_CHARS);
            self.last_user_agent = bounded(sample.user_agent, MAX_USER_AGENT_CHARS);
            self.last_route = bounded(sample.route, MAX_ROUTE_CHARS);
        }
    }

    /// Fold a row that failed to flush back in beside what arrived since.
    fn merge(&mut self, other: UsageRow) {
        self.requests += other.requests;
        self.errors_4xx += other.errors_4xx;
        self.errors_5xx += other.errors_5xx;
        if other.last_seen_at > self.last_seen_at {
            self.last_seen_at = other.last_seen_at;
            self.last_ip = other.last_ip;
            self.last_user_agent = other.last_user_agent;
            self.last_route = other.last_route;
        }
    }
}

/// The accumulator proper; the process holds one behind a mutex.
#[derive(Default)]
pub(crate) struct UsageAccumulator {
    rows: HashMap<(Uuid, NaiveDate), UsageRow>,
    dropped: u64,
}

impl UsageAccumulator {
    pub(crate) fn record(&mut self, sample: UsageSample) {
        let key = (sample.token_id, sample.at.date_naive());
        if !self.rows.contains_key(&key) && self.rows.len() >= MAX_PENDING_ROWS {
            self.dropped += 1;
            return;
        }
        self.rows
            .entry(key)
            .or_insert_with(|| UsageRow::empty(key.0, key.1, sample.at))
            .add(sample);
    }

    /// Take everything accumulated, leaving the accumulator empty. Also
    /// returns how many samples were dropped at the cap since the last drain.
    pub(crate) fn drain(&mut self) -> (Vec<UsageRow>, u64) {
        let rows = std::mem::take(&mut self.rows).into_values().collect();
        (rows, std::mem::take(&mut self.dropped))
    }

    /// Put rows a failed flush could not write back, merged with whatever
    /// arrived for the same `(token, day)` meanwhile.
    pub(crate) fn restore(&mut self, rows: Vec<UsageRow>) {
        for row in rows {
            let key = (row.token_id, row.day);
            let has_room = self.rows.len() < MAX_PENDING_ROWS;
            match self.rows.get_mut(&key) {
                Some(existing) => existing.merge(row),
                None if has_room => {
                    self.rows.insert(key, row);
                }
                None => self.dropped += row.requests.max(0) as u64,
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }
}

static ACCUMULATOR: LazyLock<Mutex<UsageAccumulator>> =
    LazyLock::new(|| Mutex::new(UsageAccumulator::default()));

fn with_accumulator<T>(f: impl FnOnce(&mut UsageAccumulator) -> T) -> T {
    // A poisoned lock holds a still-valid map; usage must never panic a request.
    let mut guard = ACCUMULATOR.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut guard)
}

/// Count one request. Cheap and infallible: a map update under a mutex.
pub fn record(sample: UsageSample) {
    with_accumulator(|a| a.record(sample));
}

/// Whether anything is waiting to be written — lets the flusher skip the
/// database entirely on a quiet minute.
pub fn has_pending() -> bool {
    with_accumulator(|a| !a.rows.is_empty() || a.dropped > 0)
}

/// What one row binds, in order: the cast each of its placeholders carries.
const ROW_CASTS: [&str; 9] = [
    "uuid",
    "date",
    "bigint",
    "bigint",
    "bigint",
    "text",
    "text",
    "text",
    "timestamptz",
];
/// Postgres counts a statement's bind parameters in sixteen bits.
const MAX_BIND_PARAMS: usize = 65_535;
/// Rows one statement carries. A flush is usually a handful of rows and so one
/// statement; a backlog at [`MAX_PENDING_ROWS`] is twenty.
const ROWS_PER_STATEMENT: usize = 1_000;
const _: () = assert!(ROWS_PER_STATEMENT * ROW_CASTS.len() <= MAX_BIND_PARAMS);

const UPSERT_HEAD: &str = r#"
INSERT INTO api_token_usage_daily (
    token_id, day, requests, errors_4xx, errors_5xx,
    last_ip, last_user_agent, last_route, last_seen_at
)
SELECT v.token_id, v.day, v.requests, v.errors_4xx, v.errors_5xx,
       v.last_ip, v.last_user_agent, v.last_route, v.last_seen_at
FROM (VALUES
"#;

/// Additive, so concurrent pods sum. The `WHERE EXISTS` skips a token whose
/// row is gone (or was never mirrored) instead of failing on the foreign key —
/// a failure would put the row back and retry it forever. Ordered, so two pods
/// flushing the same tokens lock their rows in one order instead of deadlocking.
const UPSERT_TAIL: &str = r#"
) AS v (
    token_id, day, requests, errors_4xx, errors_5xx,
    last_ip, last_user_agent, last_route, last_seen_at
)
WHERE EXISTS (SELECT 1 FROM api_tokens WHERE id = v.token_id)
ORDER BY v.token_id, v.day
ON CONFLICT (token_id, day) DO UPDATE SET
    requests = api_token_usage_daily.requests + EXCLUDED.requests,
    errors_4xx = api_token_usage_daily.errors_4xx + EXCLUDED.errors_4xx,
    errors_5xx = api_token_usage_daily.errors_5xx + EXCLUDED.errors_5xx,
    last_ip = CASE WHEN EXCLUDED.last_seen_at >= api_token_usage_daily.last_seen_at
                   THEN EXCLUDED.last_ip ELSE api_token_usage_daily.last_ip END,
    last_user_agent = CASE WHEN EXCLUDED.last_seen_at >= api_token_usage_daily.last_seen_at
                   THEN EXCLUDED.last_user_agent ELSE api_token_usage_daily.last_user_agent END,
    last_route = CASE WHEN EXCLUDED.last_seen_at >= api_token_usage_daily.last_seen_at
                   THEN EXCLUDED.last_route ELSE api_token_usage_daily.last_route END,
    last_seen_at = GREATEST(api_token_usage_daily.last_seen_at, EXCLUDED.last_seen_at)
"#;

/// The upsert for `rows` rows: one `VALUES` tuple each, its parameters
/// numbered on from the previous row's.
fn upsert_sql(rows: usize) -> String {
    let tuples: Vec<String> = (0..rows)
        .map(|row| {
            let first = row * ROW_CASTS.len() + 1;
            let params: Vec<String> = ROW_CASTS
                .iter()
                .enumerate()
                .map(|(i, cast)| format!("${}::{cast}", first + i))
                .collect();
            format!("    ({})", params.join(", "))
        })
        .collect();
    format!("{UPSERT_HEAD}{}{UPSERT_TAIL}", tuples.join(",\n"))
}

/// One statement writing `rows`. A statement cannot update the same row
/// twice, so each `(token, day)` must appear once — which a drained
/// accumulator guarantees, being a map keyed by exactly that.
fn upsert_statement(rows: &[UsageRow]) -> Statement {
    let mut values: Vec<Value> = Vec::with_capacity(rows.len() * ROW_CASTS.len());
    for row in rows {
        let bound: [Value; ROW_CASTS.len()] = [
            row.token_id.into(),
            row.day.into(),
            row.requests.into(),
            row.errors_4xx.into(),
            row.errors_5xx.into(),
            row.last_ip.clone().into(),
            row.last_user_agent.clone().into(),
            row.last_route.clone().into(),
            row.last_seen_at.fixed_offset().into(),
        ];
        values.extend(bound);
    }
    Statement::from_sql_and_values(DatabaseBackend::Postgres, upsert_sql(rows.len()), values)
}

/// Write `rows` through `execute`, one statement per [`ROWS_PER_STATEMENT`].
/// A statement writes all of its rows or none, so on an error the count that
/// comes back is exact: every row before the failed statement was written.
async fn write_rows<F, Fut>(rows: &[UsageRow], mut execute: F) -> Result<(), (usize, DbErr)>
where
    F: FnMut(Statement) -> Fut,
    Fut: Future<Output = Result<(), DbErr>>,
{
    let mut written = 0;
    for chunk in rows.chunks(ROWS_PER_STATEMENT) {
        execute(upsert_statement(chunk))
            .await
            .map_err(|e| (written, e))?;
        written += chunk.len();
    }
    Ok(())
}

/// Write everything accumulated. Returns how many `(token, day)` rows were
/// drained and sent (a token whose row is gone is sent and skipped). On a
/// database error the unwritten rows go back into the
/// accumulator and the error is returned for the caller to log.
pub async fn flush<C: ConnectionTrait>(db: &C) -> Result<usize, DbErr> {
    let (mut rows, dropped) = with_accumulator(UsageAccumulator::drain);
    if dropped > 0 {
        tracing::warn!(
            dropped,
            "token usage: samples dropped at the pending-row cap"
        );
    }
    let total = rows.len();
    let execute = |statement| async move { db.execute_raw(statement).await.map(|_| ()) };
    let outcome = write_rows(&rows, execute).await;
    if let Err((written, e)) = outcome {
        with_accumulator(|a| a.restore(rows.split_off(written)));
        return Err(e);
    }
    Ok(total)
}

/// Delete usage rows older than `retain_days`. Idempotent; indexed by day.
pub async fn prune_older_than<C: ConnectionTrait>(db: &C, retain_days: i64) -> Result<u64, DbErr> {
    let cutoff = (Utc::now() - chrono::Duration::days(retain_days)).date_naive();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM api_token_usage_daily WHERE day < $1::date",
        [cutoff.into()],
    ))
    .await
    .map(|r| r.rows_affected())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
