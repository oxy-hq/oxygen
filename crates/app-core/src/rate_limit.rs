//! Fixed-window rate limiting shared by every replica, counted in Postgres.
//!
//! Until this module every limiter in the HTTP layer was a `HashMap` behind a
//! process-wide `Mutex`: publishes per publisher, SDK events per `(user, app)`,
//! function calls, beacon batches, frontline PIN attempts per org. Each one
//! bounded what *one replica* would take, so behind `N` replicas a caller held
//! `N` budgets, and a limit written as `60/min` meant whatever the replica
//! count happened to be that day. The fleet doc (`multi-instance-fleet.md`,
//! "Per-process state") lists which state may stay per process; a limit that
//! is a security or cost boundary may not.
//!
//! **Model.** One row per `(key, window_start)` in `rate_limit_windows`,
//! windows aligned to the Unix epoch so every replica agrees on which window
//! `now` is in without talking to each other. [`take`] is one statement —
//! `INSERT … ON CONFLICT DO UPDATE SET count = count + 1 RETURNING count` —
//! so concurrent callers on different replicas serialize on the row and the
//! `limit + 1`-th caller in a window is the first refused, whichever replica
//! it hit. A refused call still counts; once over, a window stays over until
//! it ends, exactly as the in-process tables behaved.
//!
//! **Fails open.** A limiter must never be the reason a page is down. When the
//! database refuses the statement the call is *allowed*, the failure is logged
//! once per [`FAIL_OPEN_LOG_EVERY`] with the running total, and
//! [`fail_open_total`] counts it (there is no metrics endpoint in the serve
//! process; the log line is the signal, as for the usage queue). The
//! credential-level lockouts behind these limiters (`verify_pin`, admission
//! control) are not affected by this choice: they stay closed on their own.
//!
//! **Expired rows** are deleted opportunistically, every [`GC_EVERY`] counted
//! calls whichever limiter counted them: a synchronous [`take`] counts one, a
//! [`Deferred`] flush counts every call it wrote. The delete takes rows whose
//! own window ended more than one window ago; each row carries its
//! `expires_at`, so a caller with a one-minute window never deletes a live row
//! of a one-hour family. No periodic task owns this.
//!
//! **Hot paths pre-check in memory.** A function call would otherwise cost a
//! write per request, so a [`Precheck`] keeps an epoch-aligned per-process
//! count for the same key and refuses *before* the database once this replica
//! alone has seen more than the limit. A process's own count is never more
//! than the shared count for the same window, so the pre-check can only refuse
//! what the database would have refused — it saves the write on a flood and
//! changes no verdict.
//!
//! **Telemetry routes never wait, and cost one write per key per second.** The
//! beacon's unload path and SDK event ingest promise to answer without
//! blocking on a write, and the usage queue's rule for them is one write per
//! batch, not per request. [`Deferred`] keeps both: each request is answered
//! from this replica's count plus the last verdict the shared table gave for
//! the key, and the calls are accumulated as per-`(key, window)` deltas that
//! one flusher task per limiter writes every [`DEFERRED_FLUSH_INTERVAL`] with
//! one multi-row upsert — so telemetry never holds more than one pool
//! connection, however many replicas or callers. Enforcement across replicas
//! there is one tick late, by design.
//!
//! **Sliding where it matters.** A fixed window lets a caller spend two
//! ceilings across a window boundary. [`peek_sliding`] weights the previous
//! window by its overlap with the trailing window (the two-bucket estimate);
//! the frontline PIN brake uses it, because that boundary is exactly what a
//! roster walker would time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

/// What a limiter decided for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Over the limit; the current window ends in `retry_after`.
    Limited {
        retry_after: Duration,
    },
}

impl Decision {
    pub fn is_limited(&self) -> bool {
        matches!(self, Decision::Limited { .. })
    }

    /// The wait to advertise, at least one second, so a `Retry-After` is never
    /// zero at the very end of a window.
    pub fn retry_after_secs(&self) -> u64 {
        match self {
            Decision::Allowed => 0,
            Decision::Limited { retry_after } => retry_after.as_secs().max(1),
        }
    }
}

/// Start of the fixed window `now` is in: windows are aligned to the Unix
/// epoch so every replica derives the same one.
pub fn window_start(now: DateTime<Utc>, window: Duration) -> DateTime<Utc> {
    let secs = window.as_secs().max(1) as i64;
    let ts = now.timestamp();
    let start = ts - ts.rem_euclid(secs);
    DateTime::from_timestamp(start, 0).unwrap_or(now)
}

/// Time left in the window `now` is in.
pub fn time_left(now: DateTime<Utc>, window: Duration) -> Duration {
    let end = window_start(now, window) + chrono::Duration::seconds(window.as_secs() as i64);
    (end - now).to_std().unwrap_or(Duration::ZERO)
}

/// The verdict for a call that made the window's count `count`: the first
/// `limit` calls pass, the rest are refused until the window ends.
fn decide_after_increment(
    count: i64,
    limit: u64,
    now: DateTime<Utc>,
    window: Duration,
) -> Decision {
    if count > limit as i64 {
        Decision::Limited {
            retry_after: time_left(now, window),
        }
    } else {
        Decision::Allowed
    }
}

/// The verdict for a caller who has not yet been counted: `limit` recorded
/// calls close the window to the next one.
fn decide_before(count: i64, limit: u64, now: DateTime<Utc>, window: Duration) -> Decision {
    if count >= limit as i64 {
        Decision::Limited {
            retry_after: time_left(now, window),
        }
    } else {
        Decision::Allowed
    }
}

// ── Fail-open accounting ───────────────────────────────────────────────────

static FAIL_OPEN: AtomicU64 = AtomicU64::new(0);
const FAIL_OPEN_LOG_EVERY: u64 = 100;

/// Calls allowed because the database could not answer, since process start.
pub fn fail_open_total() -> u64 {
    FAIL_OPEN.load(Ordering::Relaxed)
}

fn fail_open(key: &str, what: &str, err: &dyn std::fmt::Display) -> Decision {
    let n = FAIL_OPEN.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_multiple_of(FAIL_OPEN_LOG_EVERY) {
        tracing::warn!(
            key,
            fail_open_total = n,
            error = %err,
            "shared rate limiter could not {what}; allowing the call (fail open)"
        );
    }
    Decision::Allowed
}

// ── The statements ─────────────────────────────────────────────────────────

const TAKE_SQL: &str = r#"
INSERT INTO rate_limit_windows (key, window_start, expires_at, count)
VALUES ($1, $2, $3, 1)
ON CONFLICT (key, window_start) DO UPDATE SET count = rate_limit_windows.count + 1
RETURNING count
"#;

const PEEK_SQL: &str = r#"
SELECT count FROM rate_limit_windows WHERE key = $1 AND window_start = $2
"#;

const GC_SQL: &str = r#"
DELETE FROM rate_limit_windows WHERE expires_at < $1
"#;

/// Count one call against `key`'s current window and say whether it is over
/// `limit`. A refused call is counted too, so the window stays closed.
pub async fn take(db: &DatabaseConnection, key: &str, limit: u64, window: Duration) -> Decision {
    take_at(db, key, limit, window, Utc::now()).await
}

/// [`take`] on the process's pooled connection, for a limiter that has no
/// `db` of its own at hand. No connection is the same fail-open as a refused
/// statement, counted and logged the same way.
pub async fn take_with_default_db(key: &str, limit: u64, window: Duration) -> Decision {
    match oxy::database::client::establish_connection().await {
        Ok(db) => take(&db, key, limit, window).await,
        Err(e) => fail_open(key, "connect", &e),
    }
}

/// [`take`] at an explicit `now`.
pub async fn take_at(
    db: &DatabaseConnection,
    key: &str,
    limit: u64,
    window: Duration,
    now: DateTime<Utc>,
) -> Decision {
    match increment(db, key, window, now).await {
        Ok(count) => {
            maybe_gc(db, window, now, 1);
            decide_after_increment(count, limit, now, window)
        }
        Err(e) => fail_open(key, "count a call", &e),
    }
}

/// Count one call against `key`'s current window without asking for a verdict:
/// for a limiter that records only failures and checks with [`peek`] first.
pub async fn record(db: &DatabaseConnection, key: &str, window: Duration) {
    let now = Utc::now();
    if let Err(e) = increment(db, key, window, now).await {
        fail_open(key, "record a call", &e);
    }
}

/// Is `key` already at `limit` in the current window? Reads only; nothing is
/// counted.
pub async fn peek(db: &DatabaseConnection, key: &str, limit: u64, window: Duration) -> Decision {
    let now = Utc::now();
    let start = window_start(now, window);
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            PEEK_SQL,
            [key.into(), start.into()],
        ))
        .await;
    match row {
        Ok(None) => Decision::Allowed,
        Ok(Some(row)) => match row.try_get::<i64>("", "count") {
            Ok(count) => decide_before(count, limit, now, window),
            Err(e) => fail_open(key, "read a count", &e),
        },
        Err(e) => fail_open(key, "read a count", &e),
    }
}

const PEEK_TWO_SQL: &str = r#"
SELECT window_start, count FROM rate_limit_windows WHERE key = $1 AND window_start IN ($2, $3)
"#;

/// Sliding-window estimate from two fixed windows: the current count plus the
/// previous window's count weighted by how much of it the trailing `window`
/// still covers. The standard two-bucket approximation: exact at a window
/// boundary, and never more than one window's worth of count wrong between.
pub fn sliding_estimate(current: i64, previous: i64, elapsed: Duration, window: Duration) -> f64 {
    let w = window.as_secs_f64().max(1.0);
    let overlap = (1.0 - elapsed.as_secs_f64() / w).clamp(0.0, 1.0);
    current as f64 + previous as f64 * overlap
}

/// How long until [`sliding_estimate`] drops below `limit` with no new calls:
/// the rest of the window when the current count alone is at the limit,
/// otherwise the moment enough of the previous window has aged out.
pub fn sliding_retry_after(
    current: i64,
    previous: i64,
    elapsed: Duration,
    window: Duration,
    limit: u64,
) -> Duration {
    if current >= limit as i64 || previous <= 0 {
        return window.saturating_sub(elapsed);
    }
    let w = window.as_secs_f64().max(1.0);
    // The previous window stops covering the shortfall once `aged` of its
    // `previous` calls have fallen out. Divide last so round numbers stay
    // round (60 * 10 / 30 is exactly 20; 60 * (1 - 20/30) is 20.000000000000004,
    // which a bare ceil would turn into one second too many), then ceil at
    // millisecond precision so a caller never retries a hair too early.
    let aged = (previous - (limit as i64 - current)) as f64;
    let until = w * aged / previous as f64 - elapsed.as_secs_f64();
    let millis = (until.clamp(0.0, w) * 1000.0).round() / 1000.0;
    Duration::from_secs_f64(millis.ceil())
}

/// Is `key` at `limit` over the trailing `window`, estimated from the current
/// and previous fixed windows? Reads only; nothing is counted. For a brake
/// whose boundary a caller could time, where a fixed window would hand them
/// two ceilings a second apart.
pub async fn peek_sliding(
    db: &DatabaseConnection,
    key: &str,
    limit: u64,
    window: Duration,
) -> Decision {
    let now = Utc::now();
    let start = window_start(now, window);
    let previous_start = start - chrono::Duration::seconds(window.as_secs() as i64);
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            PEEK_TWO_SQL,
            [key.into(), start.into(), previous_start.into()],
        ))
        .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return fail_open(key, "read two counts", &e),
    };
    let (mut current, mut previous) = (0i64, 0i64);
    for row in rows {
        let (Ok(ws), Ok(count)) = (
            row.try_get::<DateTime<Utc>>("", "window_start"),
            row.try_get::<i64>("", "count"),
        ) else {
            return fail_open(key, "read two counts", &"unexpected row shape");
        };
        if ws == start {
            current = count;
        } else {
            previous = count;
        }
    }
    let elapsed = (now - start).to_std().unwrap_or(Duration::ZERO);
    if sliding_estimate(current, previous, elapsed, window) >= limit as f64 {
        Decision::Limited {
            retry_after: sliding_retry_after(current, previous, elapsed, window, limit),
        }
    } else {
        Decision::Allowed
    }
}

async fn increment(
    db: &DatabaseConnection,
    key: &str,
    window: Duration,
    now: DateTime<Utc>,
) -> Result<i64, sea_orm::DbErr> {
    let start = window_start(now, window);
    let expires_at = start + chrono::Duration::seconds(window.as_secs() as i64);
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            TAKE_SQL,
            [key.into(), start.into(), expires_at.into()],
        ))
        .await?
        .ok_or_else(|| {
            sea_orm::DbErr::RecordNotFound("rate_limit_windows upsert returned no row".into())
        })?;
    row.try_get::<i64>("", "count")
}

// ── Opportunistic cleanup ──────────────────────────────────────────────────

/// Every this-many counted calls, one `DELETE` of rows whose window ended more
/// than one of the caller's windows ago. Any row past its own `expires_at` is
/// dead for every family, so the caller's window length is only slack. The
/// delete is spawned, never awaited by the request or flush that triggered it.
const GC_EVERY: u64 = 64;
static COUNTED: AtomicU64 = AtomicU64::new(0);

/// Did adding `calls` to a counter at `before` cross a multiple of
/// [`GC_EVERY`]? A synchronous take adds one; a deferred flush adds every call
/// it wrote, so the busiest families sweep as often as their traffic warrants.
fn crosses_gc_boundary(before: u64, calls: u64) -> bool {
    calls > 0 && (before / GC_EVERY) != ((before + calls) / GC_EVERY)
}

fn maybe_gc(db: &DatabaseConnection, window: Duration, now: DateTime<Utc>, calls: u64) {
    let before = COUNTED.fetch_add(calls, Ordering::Relaxed);
    if !crosses_gc_boundary(before, calls) {
        return;
    }
    // Outside a runtime (a unit test) there is nothing to spawn on; the rows
    // stay until a request on a running server sweeps them.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let cutoff = now - chrono::Duration::seconds(window.as_secs() as i64);
    let db = db.clone();
    handle.spawn(async move {
        if let Err(e) = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                GC_SQL,
                [cutoff.into()],
            ))
            .await
        {
            // Not a verdict, so not a fail-open: the rows stay until the next sweep.
            tracing::debug!(error = %e, "rate_limit_windows cleanup skipped");
        }
    });
}

// ── In-process pre-check for hot paths ─────────────────────────────────────

/// An epoch-aligned per-process count per key, kept so a flood costs this
/// replica one write per window rather than one per request. Over the limit
/// here implies over the limit in the shared table (this replica's calls are a
/// subset of the window's), so a `true` from [`Precheck::over`] is a verdict
/// the database would have given; a `false` means ask it.
pub struct Precheck {
    // `HashMap::new` is not const, so the map is made on first use.
    table: OnceLock<Mutex<HashMap<String, (i64, u64)>>>,
    calls: AtomicU64,
}

/// Sweep the pre-check table every this-many calls; it holds only keys seen in
/// the last two windows afterwards.
const PRECHECK_GC_EVERY: u64 = 256;

impl Precheck {
    pub const fn new() -> Self {
        Self {
            table: OnceLock::new(),
            calls: AtomicU64::new(0),
        }
    }

    /// Count the call locally; `true` when this replica alone has now seen more
    /// than `limit` calls for `key` in the current window.
    pub fn over(&self, key: &str, limit: u64, window: Duration) -> bool {
        self.over_at(key, limit, window, Utc::now())
    }

    pub fn over_at(&self, key: &str, limit: u64, window: Duration, now: DateTime<Utc>) -> bool {
        let start = window_start(now, window).timestamp();
        // A poisoned lock is a previous panic mid-insert; the map holds counts
        // with no invariant to protect, so recover rather than refuse forever.
        let mut table = self
            .table
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self
            .calls
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(PRECHECK_GC_EVERY)
        {
            let keep_from = start - window.as_secs() as i64;
            table.retain(|_, (s, _)| *s >= keep_from);
        }
        let entry = table.entry(key.to_string()).or_insert((start, 0));
        if entry.0 != start {
            *entry = (start, 0);
        }
        entry.1 += 1;
        entry.1 > limit
    }
}

impl Default for Precheck {
    fn default() -> Self {
        Self::new()
    }
}

// ── Never-wait limiter for telemetry routes ────────────────────────────────

/// How often a [`Deferred`] limiter's flusher writes the deltas it has
/// gathered: one upsert per key per tick, like the usage queue's one insert
/// per batch.
pub const DEFERRED_FLUSH_INTERVAL: Duration = Duration::from_millis(1000);

/// Distinct `(key, window)` pairs a [`Deferred`] limiter holds between
/// flushes. A flood of distinct keys past this is dropped from the shared
/// count (and counted in [`Deferred::dropped_total`]) rather than grown: the
/// in-process pre-check still limits each of them on this replica.
const DEFERRED_MAX_PENDING: usize = 4096;

/// Rows per multi-row upsert; four parameters each, well under Postgres's
/// parameter cap.
const DEFERRED_FLUSH_CHUNK: usize = 500;

/// One key's gathered calls for one window, waiting to be written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingDelta {
    delta: u64,
    limit: u64,
    window: Duration,
}

/// A limiter that never waits for the database and writes once per key per
/// tick.
///
/// For the routes whose contract is to answer at once — the beacon, whose
/// response holds the browser's unload path, and SDK event ingest. The verdict
/// is this replica's own count ([`Precheck`]) or the last verdict the shared
/// table gave for the key in this window. Each allowed call adds one to a
/// per-`(key, window start)` delta in memory; one flusher task per limiter
/// (never two: a compare-and-swap guards it) wakes every
/// [`DEFERRED_FLUSH_INTERVAL`], takes every pending delta, and writes them in
/// one `INSERT … ON CONFLICT DO UPDATE SET count = count + EXCLUDED.count
/// RETURNING` per [`DEFERRED_FLUSH_CHUNK`] keys, marking over the keys whose
/// returned count passed their limit. So the limit is enforced across replicas
/// one tick late, a request is never made to wait, and telemetry holds at most
/// one pool connection at a time.
pub struct Deferred {
    precheck: Precheck,
    /// Keys the shared table reported over, by the window start it said so for.
    over: OnceLock<Mutex<HashMap<String, i64>>>,
    /// Calls not yet written, per `(key, window start)`.
    pending: OnceLock<Mutex<HashMap<(String, i64), PendingDelta>>>,
    flushing: AtomicBool,
    dropped: AtomicU64,
}

impl Deferred {
    pub const fn new() -> Self {
        Self {
            precheck: Precheck::new(),
            over: OnceLock::new(),
            pending: OnceLock::new(),
            flushing: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    }

    /// Decide now, from memory; the call joins the next flush. `true` when the
    /// call is over the limit.
    pub fn over(&'static self, key: String, limit: u64, window: Duration) -> bool {
        let now = Utc::now();
        let start = window_start(now, window).timestamp();
        if self.precheck.over_at(&key, limit, window, now) {
            return true;
        }
        if self.marked_over(&key, start) {
            return true;
        }
        self.add_pending(key, start, limit, window);
        self.arm_flusher();
        false
    }

    /// Deltas dropped because the pending map was full.
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn over_table(&self) -> std::sync::MutexGuard<'_, HashMap<String, i64>> {
        self.over
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn pending_table(&self) -> std::sync::MutexGuard<'_, HashMap<(String, i64), PendingDelta>> {
        self.pending
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn marked_over(&self, key: &str, start: i64) -> bool {
        self.over_table().get(key).is_some_and(|s| *s == start)
    }

    fn mark_over(&self, key: String, start: i64) {
        let mut table = self.over_table();
        // Entries from earlier windows are dead; drop them on the way through
        // so the table holds only keys over in the current window.
        table.retain(|_, s| *s >= start);
        table.insert(key, start);
    }

    /// Add one call to the key's pending delta for this window. A new pair past
    /// [`DEFERRED_MAX_PENDING`] is dropped and counted.
    fn add_pending(&self, key: String, start: i64, limit: u64, window: Duration) {
        let mut table = self.pending_table();
        if let Some(p) = table.get_mut(&(key.clone(), start)) {
            p.delta += 1;
            return;
        }
        if table.len() >= DEFERRED_MAX_PENDING {
            let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(FAIL_OPEN_LOG_EVERY) {
                tracing::warn!(
                    dropped_total = n,
                    "deferred rate limiter: pending map full; a call was not counted"
                );
            }
            return;
        }
        table.insert(
            (key, start),
            PendingDelta {
                delta: 1,
                limit,
                window,
            },
        );
    }

    /// Take everything pending, leaving the map empty, **sorted by
    /// `(key, window_start)`**. The flush writes a chunk as one multi-row
    /// upsert, and Postgres locks the rows in the order the `VALUES` list them:
    /// two replicas flushing the same hot keys in different orders would
    /// deadlock, and the aborted chunk's deltas would be lost. One order on
    /// every replica means they queue instead.
    fn take_pending(&self) -> Vec<((String, i64), PendingDelta)> {
        let mut batch: Vec<_> = std::mem::take(&mut *self.pending_table())
            .into_iter()
            .collect();
        batch.sort_by(|a, b| a.0.cmp(&b.0));
        batch
    }

    fn has_pending(&self) -> bool {
        !self.pending_table().is_empty()
    }

    /// Mark over every key whose returned count passed its limit.
    fn apply_counts(&self, batch: &[((String, i64), PendingDelta)], counts: &[(String, i64, i64)]) {
        for (key, start, count) in counts {
            let limit = batch
                .iter()
                .find(|((k, s), _)| k == key && s == start)
                .map(|(_, p)| p.limit);
            if limit.is_some_and(|limit| *count > limit as i64) {
                self.mark_over(key.clone(), *start);
            }
        }
    }

    /// Start the one flusher task if none is running. It ticks while there is
    /// anything to write and stops when a tick finds nothing.
    fn arm_flusher(&'static self) {
        // Outside a runtime (a unit test) there is no background: the verdict
        // is this replica's alone, and the deltas stay in memory.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .flushing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        handle.spawn(async move {
            loop {
                tokio::time::sleep(DEFERRED_FLUSH_INTERVAL).await;
                if !self.flush_once().await {
                    break;
                }
            }
            self.flushing.store(false, Ordering::Release);
            // A delta that arrived between the empty tick and the store above
            // would otherwise wait for the next request to arm a flusher.
            if self.has_pending() {
                self.arm_flusher();
            }
        });
    }

    /// Write everything pending in one upsert per chunk; `false` when there
    /// was nothing to write.
    async fn flush_once(&self) -> bool {
        let batch = self.take_pending();
        if batch.is_empty() {
            return false;
        }
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                fail_open("deferred", "connect for a flush", &e);
                return true;
            }
        };
        let mut written_calls = 0u64;
        let mut widest_window = Duration::ZERO;
        for chunk in batch.chunks(DEFERRED_FLUSH_CHUNK) {
            let (sql, values) = flush_statement(chunk);
            match db
                .query_all_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    sql,
                    values,
                ))
                .await
            {
                Ok(rows) => {
                    let counts: Vec<(String, i64, i64)> = rows
                        .iter()
                        .filter_map(|row| {
                            let key = row.try_get::<String>("", "key").ok()?;
                            let ws = row.try_get::<DateTime<Utc>>("", "window_start").ok()?;
                            let count = row.try_get::<i64>("", "count").ok()?;
                            Some((key, ws.timestamp(), count))
                        })
                        .collect();
                    self.apply_counts(chunk, &counts);
                    for (_, p) in chunk {
                        written_calls += p.delta;
                        widest_window = widest_window.max(p.window);
                    }
                }
                Err(e) => {
                    fail_open("deferred", "flush a batch", &e);
                }
            }
        }
        // The calls this flush wrote count toward the sweep like synchronous
        // takes do; otherwise the busiest families (beacon, SDK events) would
        // grow a row per user, app and minute that nothing ever deleted.
        maybe_gc(&db, widest_window, Utc::now(), written_calls);
        true
    }
}

impl Default for Deferred {
    fn default() -> Self {
        Self::new()
    }
}

/// One multi-row upsert for a chunk of pending deltas: `(key, window_start,
/// expires_at, delta)` per row, counts added on conflict, the new counts
/// returned so the caller can mark keys over. Pairs in a chunk are distinct by
/// construction (they were map keys), which `ON CONFLICT` requires.
fn flush_statement(chunk: &[((String, i64), PendingDelta)]) -> (String, Vec<sea_orm::Value>) {
    let mut sql = String::from(
        "INSERT INTO rate_limit_windows (key, window_start, expires_at, count) VALUES ",
    );
    let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 4);
    for (i, ((key, start), p)) in chunk.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let n = i * 4;
        sql.push_str(&format!("(${}, ${}, ${}, ${})", n + 1, n + 2, n + 3, n + 4));
        let start_at = DateTime::from_timestamp(*start, 0).unwrap_or_else(Utc::now);
        let expires_at = start_at + chrono::Duration::seconds(p.window.as_secs() as i64);
        values.push(key.as_str().into());
        values.push(start_at.into());
        values.push(expires_at.into());
        values.push((p.delta as i64).into());
    }
    sql.push_str(
        " ON CONFLICT (key, window_start) DO UPDATE SET count = rate_limit_windows.count + \
         EXCLUDED.count RETURNING key, window_start, count",
    );
    (sql, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    const MINUTE: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn windows_are_aligned_to_the_epoch_so_replicas_agree() {
        assert_eq!(window_start(at(1_000_000_059), MINUTE), at(1_000_000_020));
        assert_eq!(window_start(at(1_000_000_020), MINUTE), at(1_000_000_020));
        assert_eq!(window_start(at(1_000_000_080), MINUTE), at(1_000_000_080));
        assert_eq!(window_start(at(7_199), HOUR), at(3_600));
    }

    #[test]
    fn time_left_counts_down_to_the_window_end() {
        assert_eq!(
            time_left(at(1_000_000_020), MINUTE),
            Duration::from_secs(60)
        );
        assert_eq!(
            time_left(at(1_000_000_059), MINUTE),
            Duration::from_secs(21)
        );
        assert_eq!(time_left(at(4_200), HOUR), Duration::from_secs(3_000));
    }

    #[test]
    fn the_first_limit_calls_pass_and_the_rest_are_refused_until_the_window_ends() {
        let now = at(4_200);
        for count in 1..=3 {
            assert_eq!(
                decide_after_increment(count, 3, now, HOUR),
                Decision::Allowed
            );
        }
        let refused = decide_after_increment(4, 3, now, HOUR);
        assert_eq!(
            refused,
            Decision::Limited {
                retry_after: Duration::from_secs(3_000)
            },
            "the rest of the hour"
        );
        assert_eq!(refused.retry_after_secs(), 3_000);
        assert!(
            decide_after_increment(40, 3, now, HOUR).is_limited(),
            "stays closed"
        );
    }

    #[test]
    fn a_peek_closes_at_the_ceiling_without_counting() {
        let now = at(90);
        assert_eq!(decide_before(29, 30, now, MINUTE), Decision::Allowed);
        assert!(decide_before(30, 30, now, MINUTE).is_limited());
    }

    #[test]
    fn retry_after_is_never_advertised_as_zero() {
        let d = Decision::Limited {
            retry_after: Duration::from_millis(10),
        };
        assert_eq!(d.retry_after_secs(), 1);
        assert_eq!(Decision::Allowed.retry_after_secs(), 0);
    }

    #[test]
    fn the_precheck_refuses_only_past_the_limit_and_resets_with_the_window() {
        let pre = Precheck::new();
        let t = at(1_000_000_020);
        for _ in 0..2 {
            assert!(!pre.over_at("k", 2, MINUTE, t));
        }
        assert!(pre.over_at("k", 2, MINUTE, t + chrono::Duration::seconds(30)));
        assert!(
            !pre.over_at("other", 2, MINUTE, t),
            "another key's own budget"
        );
        assert!(
            !pre.over_at("k", 2, MINUTE, t + chrono::Duration::seconds(60)),
            "the next window starts clean"
        );
    }

    #[test]
    fn a_sliding_estimate_counts_the_previous_window_by_its_overlap() {
        // 15 in the previous window and 15 in this one, at the boundary: 30.
        assert_eq!(sliding_estimate(15, 15, Duration::ZERO, MINUTE), 30.0);
        // Half a minute in, half the previous window has aged out.
        assert_eq!(
            sliding_estimate(0, 30, Duration::from_secs(30), MINUTE),
            15.0
        );
        // A full window in, none of it counts.
        assert_eq!(sliding_estimate(3, 30, MINUTE, MINUTE), 3.0);
    }

    #[test]
    fn a_sliding_retry_after_is_when_enough_of_the_previous_window_has_aged_out() {
        // Current alone at the limit: the rest of the window.
        assert_eq!(
            sliding_retry_after(30, 0, Duration::from_secs(10), MINUTE, 30),
            Duration::from_secs(50)
        );
        // 10 now + 30 before, limit 30: need 20/30 of the previous window to
        // still count, which it stops doing 20 s in; at 5 s that is 15 s away.
        assert_eq!(
            sliding_retry_after(10, 30, Duration::from_secs(5), MINUTE, 30),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn a_deferred_limiter_answers_from_memory_and_gathers_deltas_per_key_and_window() {
        static D: Deferred = Deferred::new();
        let start = window_start(Utc::now(), MINUTE).timestamp();
        for _ in 0..2 {
            assert!(
                !D.over("k".into(), 5, MINUTE),
                "within this replica's budget"
            );
        }
        assert!(!D.over("other".into(), 5, MINUTE));
        // Two calls for `k`, one for `other`, gathered as deltas, not written,
        // and handed over in key order.
        let pending = D.take_pending();
        assert_eq!(
            pending,
            vec![
                (
                    ("k".to_string(), start),
                    PendingDelta {
                        delta: 2,
                        limit: 5,
                        window: MINUTE
                    }
                ),
                (
                    ("other".to_string(), start),
                    PendingDelta {
                        delta: 1,
                        limit: 5,
                        window: MINUTE
                    }
                ),
            ]
        );
        assert!(!D.has_pending(), "take leaves the map empty");
        // The pre-check still refuses this replica's own flood, and a refused
        // call joins no delta.
        for _ in 0..3 {
            D.over("k".into(), 5, MINUTE);
        }
        assert!(
            D.over("k".into(), 5, MINUTE),
            "the sixth call in the window"
        );
        assert_eq!(D.take_pending().len(), 1);
        assert_eq!(D.dropped_total(), 0);
    }

    /// Every replica must lock the same hot keys in the same order, or two
    /// multi-row upserts deadlock: the batch a flush chunks is sorted by
    /// `(key, window_start)` whatever order the map yields.
    #[test]
    fn a_flush_receives_its_rows_sorted_by_key_and_window() {
        static D: Deferred = Deferred::new();
        let start = window_start(Utc::now(), MINUTE).timestamp();
        for key in ["zeta", "alpha", "mid", "beta", "omega", "gamma"] {
            assert!(!D.over(key.into(), 10, MINUTE));
        }
        D.add_pending("alpha".into(), start - 60, 10, MINUTE);
        let batch = D.take_pending();
        let order: Vec<(String, i64)> = batch.iter().map(|(k, _)| k.clone()).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
        assert_eq!(
            order[0],
            ("alpha".to_string(), start - 60),
            "earlier window first"
        );
        assert_eq!(order[1], ("alpha".to_string(), start));
        for chunk in batch.chunks(DEFERRED_FLUSH_CHUNK) {
            let (sql, _) = flush_statement(chunk);
            assert!(sql.starts_with("INSERT INTO rate_limit_windows"));
        }
    }

    #[test]
    fn a_flush_counts_every_call_it_wrote_toward_the_sweep() {
        // A synchronous take adds one: the 64th crosses the boundary.
        assert!(!crosses_gc_boundary(62, 1));
        assert!(crosses_gc_boundary(63, 1));
        // A flush of 40 calls from 30 crosses 64 at once; one of 10 does not.
        assert!(crosses_gc_boundary(30, 40));
        assert!(!crosses_gc_boundary(30, 10));
        // A flush that wrote nothing never sweeps.
        assert!(!crosses_gc_boundary(63, 0));
    }

    #[test]
    fn a_flush_marks_over_the_keys_whose_returned_count_passed_their_limit() {
        static D: Deferred = Deferred::new();
        let start = window_start(Utc::now(), MINUTE).timestamp();
        let batch = vec![
            (
                ("a".to_string(), start),
                PendingDelta {
                    delta: 3,
                    limit: 10,
                    window: MINUTE,
                },
            ),
            (
                ("b".to_string(), start),
                PendingDelta {
                    delta: 1,
                    limit: 10,
                    window: MINUTE,
                },
            ),
        ];
        // Other replicas had already counted 8 for `a`: 11 > 10, over. `b` is
        // at 10, not over (the eleventh call is the first refused).
        D.apply_counts(&batch, &[("a".into(), start, 11), ("b".into(), start, 10)]);
        assert!(
            D.over("a".into(), 10, MINUTE),
            "shared over, refused at once"
        );
        assert!(!D.over("b".into(), 10, MINUTE));
        // An over from an earlier window is dead.
        D.mark_over("stale".into(), start - 60);
        assert!(!D.marked_over("stale", start));
    }

    #[test]
    fn the_pending_map_is_bounded_and_drops_past_it() {
        static D: Deferred = Deferred::new();
        for i in 0..DEFERRED_MAX_PENDING {
            assert!(!D.over(format!("k{i}"), 5, MINUTE));
        }
        assert_eq!(D.dropped_total(), 0);
        assert!(!D.over("one-too-many".into(), 5, MINUTE), "still answered");
        assert_eq!(
            D.dropped_total(),
            1,
            "but not counted towards the shared table"
        );
        // An existing pair still accumulates.
        assert!(!D.over("k0".into(), 5, MINUTE));
        assert_eq!(D.take_pending().len(), DEFERRED_MAX_PENDING);
    }

    #[test]
    fn a_flush_statement_is_one_multi_row_upsert_with_four_parameters_per_row() {
        let start = window_start(at(1_000_000_020), MINUTE).timestamp();
        let chunk = vec![
            (
                ("a".to_string(), start),
                PendingDelta {
                    delta: 2,
                    limit: 5,
                    window: MINUTE,
                },
            ),
            (
                ("b".to_string(), start),
                PendingDelta {
                    delta: 1,
                    limit: 5,
                    window: MINUTE,
                },
            ),
        ];
        let (sql, values) = flush_statement(&chunk);
        assert!(sql.contains("VALUES ($1, $2, $3, $4), ($5, $6, $7, $8) ON CONFLICT"));
        assert!(sql.ends_with("RETURNING key, window_start, count"));
        assert_eq!(values.len(), 8);
    }

    #[test]
    fn the_precheck_sweeps_keys_older_than_two_windows() {
        let pre = Precheck::new();
        let t = at(1_000_000_020);
        pre.over_at("old", 5, MINUTE, t);
        for _ in 0..PRECHECK_GC_EVERY {
            pre.over_at("fresh", 5, MINUTE, t + chrono::Duration::seconds(300));
        }
        let table = pre.table.get().unwrap().lock().unwrap();
        assert!(!table.contains_key("old"));
        assert!(table.contains_key("fresh"));
    }
}
