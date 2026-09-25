//! Whether a failed function should page ops, decided against the rows
//! finalization writes. The page itself is `failure_page`.
//!
//! Through the ClickHouse insert outage (0.5.140–0.5.144) every failure sat in
//! `app_function_invocations`, and nothing read the table until a customer
//! asked. This reads it — at the moment a failure is written.
//!
//! **What pages.** One (app, function, [fingerprint]) that is new — its first
//! occurrence in the last [`LOOKBACK_DAYS`] days — by either of two routes:
//!
//! - **Fast:** the first occurrence falls inside the last [`NEW_WITHIN_HOURS`]
//!   hours and it has happened [`THRESHOLD`] times. A deploy that breaks a busy
//!   path is exactly that shape: a failure the function never had, now on
//!   every call.
//! - **Persistent:** it has happened [`PERSISTENT_THRESHOLD`] times, the first at
//!   least [`PERSISTENT_AFTER_HOURS`] hours ago; it was absent the whole week
//!   before that, while the function *did* answer in that week; and it has not
//!   answered once since. It worked, then it broke, and it stayed broken — a
//!   function with no success on record is having its own failure, not a break.
//!   This is the low-traffic shape the fast route cannot see. In the Sep 2026
//!   warehouse incident `submit-receiving` failed on every call for days and
//!   paged only when a third call happened to land inside 24 hours of the
//!   first; a function called less often than that would never have paged.
//!
//! A function's usual failures — the validation error it throws every day — are
//! not new and never page. A one-off stays under both routes, and a flaky
//! failure that the function has succeeded past stays quiet on the persistent
//! route.
//!
//! **Held back** (recorded as `suppressed:<reason>`):
//! - `function_rate` — the function already paged in the last
//!   [`FUNCTION_PAGE_WINDOW_HOURS`] hours;
//! - `platform_rate` — [`PAGES_PER_HOUR`] pages already went out this hour;
//! - `unfingerprinted_history` — fingerprints are less than a week old and the
//!   function's lookback holds failures from before them, any of which might be
//!   this one, so a chronic failure would otherwise read as new on deploy day.
//!
//! A rate hold lasts only as long as its window. The next failure after it
//! re-checks the caps and pages — without being judged new again, because by
//! then its first occurrence may be older than a day, and a failure held back
//! for being one too many must not be silenced for being late. A first-week
//! hold lasts the week, as the page it stands in for would.
//!
//! **Once.** The page is claimed with an upsert on
//! `app_function_failure_alerts` before anything is sent, so N replicas
//! finalizing the same failure send one message. A claim left unsent by a
//! replica that died goes stale after [`DELIVERY_GRACE_MINUTES`] minutes and the
//! next failure retries it; if no failure follows, that page is lost.
//!
//! **Cost.** Inline at finalization, only for a failure. Every failure costs a
//! primary-key read. One already paged or held back stops there. Any other
//! failure also costs an index probe — so a familiar failure, first seen more
//! than a day ago, pays those two round-trips on every failed invocation, since
//! nothing is recorded for it. A new failure below the threshold adds a count
//! that stops at the threshold. None of it grows with how long a failure lasts.
//!
//! [fingerprint]: super::failure_signal::fingerprint

use chrono::{DateTime, Duration, FixedOffset, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement, Value};
use uuid::Uuid;

/// Occurrences of a new failure before it pages.
pub const THRESHOLD: i64 = 3;
/// Occurrences of a new failure before the persistent route pages it.
pub const PERSISTENT_THRESHOLD: i64 = 2;
/// How long a new failure must have gone on, with no success in between,
/// before the persistent route pages it. Long enough that a retry burst or a
/// minutes-long vendor blip is the fast route's to judge, not this one's.
pub const PERSISTENT_AFTER_HOURS: i64 = 6;
/// How recently a failure must first have appeared to count as new.
pub const NEW_WITHIN_HOURS: i64 = 24;
/// How far back "has this function failed this way before?" looks.
pub const LOOKBACK_DAYS: i64 = 7;
/// How long a claimed page may stay unsent before another replica may take it.
pub const DELIVERY_GRACE_MINUTES: i64 = 5;
/// A function pages at most once in this window, whatever the fingerprint.
pub const FUNCTION_PAGE_WINDOW_HOURS: i64 = 6;
/// Pages across the whole platform in any hour before the rest are held back.
pub const PAGES_PER_HOUR: i64 = 10;
/// Alert rows untouched this long are deleted; none of them decides anything
/// past [`LOOKBACK_DAYS`].
pub const RETENTION_DAYS: i64 = 30;

/// One failure signal: a fingerprint on one function of one app.
#[derive(Debug, Clone, Copy)]
pub struct FailureKey<'a> {
    pub app_id: Uuid,
    pub function_name: &'a str,
    pub fingerprint: &'a str,
}

/// What a failure calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not new, below the threshold, or already paged, held back or claimed.
    Quiet,
    /// New and due, but held back for the named reason, and recorded.
    Suppressed(&'static str),
    /// New and due; the caller now owns sending it, then [`mark_delivered`].
    /// `persistent` says which route found it, so the page can say why.
    Page {
        first_seen: DateTime<Utc>,
        persistent: bool,
    },
}

/// What the alerts table already says about a key.
enum Prior {
    /// No row, or one past its reach (a week after its page): judge afresh.
    Open,
    /// Paged or held back and still in force, or claimed by a replica that may
    /// still be sending.
    Handled,
    /// Judged new and due before but never sent — a rate hold whose window
    /// passed, or a claim gone stale unsent. Re-check the caps, not newness.
    Due,
}

/// Decide what the failure `key` calls for as of `now`, claiming the page (or
/// recording why it is held back) when one is due. `history_since` is when
/// invocations started carrying a fingerprint.
pub async fn claim(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
    history_since: DateTime<Utc>,
) -> Result<Verdict, DbErr> {
    let (first_seen, persistent) = match prior(db, key, now).await? {
        Prior::Handled => return Ok(Verdict::Quiet),
        Prior::Due => {
            // Re-checking the caps, not newness — but the page states facts,
            // and they must be true now. Under the threshold it is the
            // persistent route's page, which says nothing succeeded since; a
            // success during the hold makes that false, and by that route's own
            // rule a failure the function answered past is flaky, not broken.
            let first = first_seen(db, key, now).await?.unwrap_or(now);
            let persistent = !reached_threshold(db, key, first, THRESHOLD).await?;
            if persistent && answered_since(db, key, first).await? {
                return Ok(Verdict::Quiet);
            }
            (first, persistent)
        }
        Prior::Open => match new_and_due(db, key, now).await? {
            Some(due) => due,
            None => return Ok(Verdict::Quiet),
        },
    };
    let held_back = match rate_limited(db, key, now).await? {
        Some(reason) => Some(reason),
        None => unfingerprinted_history(db, key, now, history_since).await?,
    };
    if !record_claim(db, key, now, held_back).await? {
        return Ok(Verdict::Quiet);
    }
    let Some(reason) = held_back else {
        if let Err(e) = prune(db, now).await {
            tracing::warn!(target: "oxy::app_function", error = %e, "failure alert: prune failed");
        }
        return Ok(Verdict::Page {
            first_seen,
            persistent,
        });
    };
    Ok(Verdict::Suppressed(reason))
}

/// When an existing alert row may be taken over, as SQL over the columns of
/// `t` (a table prefix, possibly empty), with the four bounds of
/// [`reclaim_bounds`] bound from `$p`. One definition: `prior` reads it and
/// `record_claim`'s upsert enforces it.
fn reclaimable(t: &str, p: usize) -> String {
    let (lookback, grace, function_window, hour) = (p, p + 1, p + 2, p + 3);
    format!(
        "COALESCE(({t}delivered_at IS NULL AND {t}claimed_at < ${grace}) \
           OR ({t}outcome = 'suppressed:function_rate' AND {t}claimed_at < ${function_window}) \
           OR ({t}outcome = 'suppressed:platform_rate' AND {t}claimed_at < ${hour}) \
           OR {t}delivered_at < ${lookback}, false)"
    )
}

fn reclaim_bounds(now: DateTime<Utc>) -> [Value; 4] {
    [
        at(lookback(now)),
        at(grace_cutoff(now)),
        at(now - Duration::hours(FUNCTION_PAGE_WINDOW_HOURS)),
        at(now - Duration::hours(1)),
    ]
}

async fn prior(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
) -> Result<Prior, DbErr> {
    let sql = format!(
        "SELECT {reclaimable} AS reclaimable, \
                (claimed_at >= $4 AND (outcome IN ('suppressed:function_rate', \
                   'suppressed:platform_rate') OR (outcome = 'paged' AND delivered_at IS NULL))) \
                  AS due \
         FROM app_function_failure_alerts \
         WHERE app_id = $1 AND function_name = $2 AND failure_fingerprint = $3",
        reclaimable = reclaimable("", 4),
    );
    let Some(row) = db
        .query_one_raw(keyed(&sql, key, reclaim_bounds(now)))
        .await?
    else {
        return Ok(Prior::Open);
    };
    Ok(
        match (
            row.try_get::<bool>("", "reclaimable")?,
            row.try_get::<bool>("", "due")?,
        ) {
            (false, _) => Prior::Handled,
            (true, true) => Prior::Due,
            (true, false) => Prior::Open,
        },
    )
}

/// The first occurrence of `key`, and whether the persistent route found it,
/// when it is new and due by either route (module docs, **What pages**).
async fn new_and_due(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
) -> Result<Option<(DateTime<Utc>, bool)>, DbErr> {
    let new_since = now - Duration::hours(NEW_WITHIN_HOURS);
    let Some(first) = first_seen(db, key, now).await? else {
        return Ok(None);
    };
    if first >= new_since && reached_threshold(db, key, new_since, THRESHOLD).await? {
        return Ok(Some((first, false)));
    }
    if persistent(db, key, first, now).await? {
        return Ok(Some((first, true)));
    }
    Ok(None)
}

/// The persistent route, for any failure the fast route did not page — a
/// low-traffic one, or one whose first occurrence is over a day old. Checks
/// run cheapest first: the fingerprint probes use
/// `idx_app_function_invocations_failure`, the two success probes
/// `idx_app_function_invocations_success`, so neither scans the function's
/// history.
async fn persistent(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    first: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    if first > now - Duration::hours(PERSISTENT_AFTER_HOURS) {
        return Ok(false);
    }
    // New, not merely new to the lookback: a failure older than the window
    // shows its first in-window occurrence near the window's start, and must
    // not read as a fresh break every week.
    if occurred_between(db, key, first - Duration::days(LOOKBACK_DAYS), first).await? {
        return Ok(false);
    }
    if !reached_threshold(db, key, first, PERSISTENT_THRESHOLD).await? {
        return Ok(false);
    }
    // It worked before it broke. Without this, a function that has never once
    // succeeded — its own failure, every call — would read as a fresh break.
    if !answered_between(db, key, first - Duration::days(LOOKBACK_DAYS), first).await? {
        return Ok(false);
    }
    Ok(!answered_since(db, key, first).await?)
}

/// Whether `key` occurred in `[from, until)`: one probe of the partial index.
async fn occurred_between(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(keyed(
            "SELECT EXISTS (SELECT 1 FROM app_function_invocations \
               WHERE app_id = $1 AND function_name = $2 AND failure_fingerprint = $3 \
                 AND created_at >= $4 AND created_at < $5) AS occurred",
            key,
            [at(from), at(until)],
        ))
        .await?;
    Ok(match row {
        Some(row) => row.try_get::<bool>("", "occurred")?,
        None => false,
    })
}

/// Whether the function has succeeded since `since`. A success is a finished
/// call the pager did not count as a failure — no fingerprint — so a `success`
/// that answered 5xx, or caught a failed `ctx.*` call, is not one.
async fn answered_since(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    since: DateTime<Utc>,
) -> Result<bool, DbErr> {
    answered(db, key, since, None).await
}

/// Whether the function succeeded in `[from, until)` — the same definition of
/// a success as [`answered_since`].
async fn answered_between(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<bool, DbErr> {
    answered(db, key, from, Some(until)).await
}

async fn answered(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    from: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
) -> Result<bool, DbErr> {
    // `until` NULL means open-ended; one statement serves both callers.
    let until: Value = match until {
        Some(t) => at(t),
        None => Value::ChronoDateTimeWithTimeZone(None),
    };
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS (SELECT 1 FROM app_function_invocations \
               WHERE app_id = $1 AND function_name = $2 AND created_at >= $3 \
                 AND ($4::timestamptz IS NULL OR created_at < $4) \
                 AND status = 'success' AND failure_fingerprint IS NULL) AS answered",
            [key.app_id.into(), key.function_name.into(), at(from), until],
        ))
        .await?;
    Ok(match row {
        Some(row) => row.try_get::<bool>("", "answered")?,
        None => false,
    })
}

/// The first occurrence of `key` in the lookback: one probe of the partial index.
async fn first_seen(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    let row = db
        .query_one_raw(keyed(
            "SELECT created_at FROM app_function_invocations \
             WHERE app_id = $1 AND function_name = $2 AND failure_fingerprint = $3 \
               AND created_at >= $4 \
             ORDER BY created_at LIMIT 1",
            key,
            [at(lookback(now))],
        ))
        .await?;
    let Some(row) = row else { return Ok(None) };
    let first: DateTime<FixedOffset> = row.try_get("", "created_at")?;
    Ok(Some(first.with_timezone(&Utc)))
}

/// Whether `key` has happened `threshold` times since `since` — counting no
/// further than that, however long the failure has gone on.
async fn reached_threshold(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    since: DateTime<Utc>,
    threshold: i64,
) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(keyed(
            "SELECT count(*) AS n FROM (SELECT 1 FROM app_function_invocations \
               WHERE app_id = $1 AND function_name = $2 AND failure_fingerprint = $3 \
                 AND created_at >= $4 LIMIT $5) capped",
            key,
            [at(since), threshold.into()],
        ))
        .await?;
    let n: i64 = match row {
        Some(row) => row.try_get("", "n")?,
        None => 0,
    };
    Ok(n >= threshold)
}

/// The two caps, over the last [`FUNCTION_PAGE_WINDOW_HOURS`] of the alerts
/// table (indexed on `claimed_at`). A page counts once it was sent or may
/// still be sending. A claim that went stale unsent does not — or retrying it
/// would be held back by its own unsent claim.
async fn rate_limited(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
) -> Result<Option<&'static str>, DbErr> {
    let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "WITH pages AS (SELECT app_id, function_name, claimed_at \
                              FROM app_function_failure_alerts \
                             WHERE outcome = 'paged' AND claimed_at >= $4 \
                               AND (delivered_at IS NOT NULL OR claimed_at >= $3)) \
             SELECT EXISTS (SELECT 1 FROM pages WHERE app_id = $1 AND function_name = $2) \
                      AS function_paged, \
                    (SELECT count(*) FROM pages WHERE claimed_at >= $5) AS paged_last_hour",
            [
                key.app_id.into(),
                key.function_name.into(),
                at(grace_cutoff(now)),
                at(now - Duration::hours(FUNCTION_PAGE_WINDOW_HOURS)),
                at(now - Duration::hours(1)),
            ],
        ))
        .await?
    else {
        return Ok(None);
    };
    if row.try_get::<bool>("", "function_paged")? {
        return Ok(Some("suppressed:function_rate"));
    }
    if row.try_get::<i64>("", "paged_last_hour")? >= PAGES_PER_HOUR {
        return Ok(Some("suppressed:platform_rate"));
    }
    Ok(None)
}

/// Whether this function's lookback reaches back before fingerprints existed
/// and holds failures from then. Free once fingerprints are a lookback old.
///
/// Not complete: a route invocation that answered 5xx is a failure
/// (`Failure::of`), but its status is stored only when the caller sent an
/// idempotency key, so an unkeyed route's chronic 500s from before
/// fingerprints are invisible here and can still page on deploy day.
async fn unfingerprinted_history(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
    history_since: DateTime<Utc>,
) -> Result<Option<&'static str>, DbErr> {
    if lookback(now) >= history_since {
        return Ok(None);
    }
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS (SELECT 1 FROM app_function_invocations \
               WHERE app_id = $1 AND function_name = $2 \
                 AND (status IN ('error', 'timeout') \
                      OR (status = 'success' AND result_status >= 500)) \
                 AND created_at >= $3 AND created_at < $4) AS unfingerprinted",
            [
                key.app_id.into(),
                key.function_name.into(),
                at(lookback(now)),
                at(history_since),
            ],
        ))
        .await?;
    let found = match row {
        Some(row) => row.try_get::<bool>("", "unfingerprinted")?,
        None => false,
    };
    Ok(found.then_some("suppressed:unfingerprinted_history"))
}

/// Upsert the claim; `false` when another replica holds it or the existing row
/// is still in force. A hold is written delivered — there is nothing to send.
async fn record_claim(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
    held_back: Option<&'static str>,
) -> Result<bool, DbErr> {
    let sql = format!(
        "INSERT INTO app_function_failure_alerts \
           (app_id, function_name, failure_fingerprint, claimed_at, delivered_at, outcome) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (app_id, function_name, failure_fingerprint) DO UPDATE \
           SET claimed_at = EXCLUDED.claimed_at, delivered_at = EXCLUDED.delivered_at, \
               outcome = EXCLUDED.outcome \
           WHERE {reclaimable} \
         RETURNING app_id",
        reclaimable = reclaimable("app_function_failure_alerts.", 7),
    );
    let [lookback, grace, function_window, hour] = reclaim_bounds(now);
    let row = db
        .query_one_raw(keyed(
            &sql,
            key,
            [
                at(now),
                held_back.map(|_| now.fixed_offset()).into(),
                held_back.unwrap_or("paged").into(),
                lookback,
                grace,
                function_window,
                hour,
            ],
        ))
        .await?;
    Ok(row.is_some())
}

/// Record that a claimed page was sent, so it is not sent again for a week.
pub async fn mark_delivered(
    db: &impl ConnectionTrait,
    key: FailureKey<'_>,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    db.execute_raw(keyed(
        "UPDATE app_function_failure_alerts SET delivered_at = $4 \
         WHERE app_id = $1 AND function_name = $2 AND failure_fingerprint = $3",
        key,
        [at(now)],
    ))
    .await
    .map(|_| ())
}

/// Delete alert rows past [`RETENTION_DAYS`]. Run on a page, which is rare, so
/// the table stays bounded by recent failures without a job of its own.
async fn prune(db: &impl ConnectionTrait, now: DateTime<Utc>) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_function_failure_alerts WHERE claimed_at < $1",
        [at(now - Duration::days(RETENTION_DAYS))],
    ))
    .await
    .map(|_| ())
}

/// A statement binding `key` as `$1..$3` and `rest` as `$4…`.
fn keyed<const N: usize>(sql: &str, key: FailureKey<'_>, rest: [Value; N]) -> Statement {
    let mut values: Vec<Value> = vec![
        key.app_id.into(),
        key.function_name.into(),
        key.fingerprint.into(),
    ];
    values.extend(rest);
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn at(t: DateTime<Utc>) -> Value {
    t.fixed_offset().into()
}

fn lookback(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::days(LOOKBACK_DAYS)
}

fn grace_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::minutes(DELIVERY_GRACE_MINUTES)
}
