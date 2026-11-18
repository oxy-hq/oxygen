//! Outbox marks: what the relay sweep knows about an org's outboxes without
//! connecting to them (app events design §6.2).
//!
//! An org's outbox is in its own OLTP store, often a scale-to-zero database a
//! connection wakes. So the platform keeps one row per (org, environment) in
//! `app_event_outbox_marks`:
//!
//! - `published_at`: the host sets it on every accepted publish
//!   ([`mark_published`]) — after the row commits, and inside an app's
//!   transaction too, where a rollback leaves a harmless mark — and a relay
//!   pass that failed sets it too ([`reopen`]), since its rows wait again;
//! - `drained_at`: a relay pass whose claim left no row waiting (read in the
//!   claim's own snapshot) sets it to when the pass began claiming
//!   ([`mark_drained`]): nothing committed (or released by a failed pass)
//!   before then is left waiting;
//! - `pending_since`: since when the oldest work still waiting has waited —
//!   set when the mark goes from new or drained to due, never moved back by
//!   publishes or failed passes, moved up by a successful pass to the oldest
//!   row it saw still waiting ([`move_wait_up`], or `started` in
//!   [`mark_drained`]), and cleared when a pass drains it. The stale alarm
//!   reads it, so a relay that fails on every pass, a hung holder or a
//!   backlog it cannot keep up with alarms, and a healthy relay under steady
//!   publishing never does.
//!
//! A mark whose `published_at` is after its `drained_at` (or has none) may
//! have rows waiting: it is **due**. The sweep reads [`due`] marks only — from
//! the platform database — so it never opens a connection to a store with
//! nothing waiting.
//!
//! Both instants are the platform database's clock, so a pass's start and a
//! publish's mark compare on one clock: a row the pass did not see committed
//! after it began, and was marked later still.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// Record a publish into `org_id`'s `environment` outbox. Never moves
/// `published_at` back: two marks committing out of order keep the later.
/// A new or drained mark becomes due, waiting from now; a due one keeps the
/// time its wait began.
pub async fn mark_published(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_event_outbox_marks AS m \
                (org_id, environment, published_at, pending_since) \
         VALUES ($1, $2, now(), now()) \
         ON CONFLICT (org_id, environment) DO UPDATE \
            SET published_at = GREATEST(m.published_at, EXCLUDED.published_at), \
                pending_since = CASE WHEN m.drained_at >= m.published_at \
                                     THEN EXCLUDED.pending_since \
                                     ELSE COALESCE(m.pending_since, EXCLUDED.pending_since) END",
        [org_id.into(), environment.into()],
    ))
    .await
    .map(|_| ())
}

/// A relay pass of `org_id`'s `environment` outbox failed, after releasing
/// its rows: they wait again, exactly as if just published, so the mark is
/// published now ([`mark_published`]). A concurrent pass that began before
/// this — it skipped the rows the failed pass held — drains only to its own
/// start, before this `published_at`, so the mark stays due and the sweep
/// queues another pass after the nudge's minute; the rows are not left for
/// the org's next publish. The wait (`pending_since`) keeps the time it
/// began, or begins now if the mark was drained, so the stale alarm still
/// sees how long the work has waited: the restamp hides nothing. `drained_at`
/// is left as it was (never moved back). No mark yet (its publish's mark
/// failed): one is made.
pub async fn reopen(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
) -> Result<(), DbErr> {
    mark_published(db, org_id, environment).await
}

/// The platform database's clock: when a relay pass begins claiming.
pub async fn platform_now(db: &impl ConnectionTrait) -> Result<DateTime<Utc>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT now() AS now",
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("SELECT now() returned no row".into()))?;
    row.try_get("", "now")
}

/// Record that a relay pass which began claiming at `started` (from
/// [`platform_now`]) left nothing older than `started` waiting in `org_id`'s
/// `environment` outbox. Never moves `drained_at` back. A mark this drains
/// (nothing published since `started`) stops waiting: `pending_since` is
/// cleared. One a later publish keeps due waits from `started` at the
/// earliest, so its `pending_since` moves up to it (never back): steady
/// publishing into a healthy outbox, where nearly every pass sees a publish
/// land after it began, never looks stale. No mark, nothing to record.
///
/// **A pass that skipped locked rows never drains** (R40, R42). Its claim is
/// `FOR UPDATE SKIP LOCKED`, so the claim statement also finds, in its own
/// snapshot and without skipping locks, the oldest waiting row it did not
/// take and how long it has waited (`OutboxClaim::others_waiting`,
/// `oldest_left_waiting`; a row committed after the claim never counts).
/// Finding one, the pass leaves the mark due and moves its wait only up to
/// that row ([`move_wait_up`]). So a pass that holds rows and fails slowly,
/// hangs while still heartbeating, or sits behind a poison row keeps the
/// outbox due, its row keeps ageing, and the stale alarm fires; while passes
/// that merely overlap, holding rows seconds old, never pin the wait.
///
/// Should the holder's process die, the task queue also runs the dead pass
/// again (R37, read 2026-11-18):
///
/// - a worker heartbeats its claimed task every 15 s
///   (`crates/agentic/runtime/src/orchestrator/worker.rs:66`); a task is
///   enqueued with a 60 s visibility timeout and `max_claims = 3`
///   (`crates/agentic/runtime/src/orchestrator/crud/queue.rs:117,120`);
/// - the reaper, every 30 s (`orchestrator/background.rs:60`, its loop
///   spawned at `:192`, calling `reap_stale_tasks` at `:267`), puts a
///   claimed task whose heartbeat is older than that back to `queued`
///   (`crud/queue.rs:856-864`), whatever its kind — the relay's included;
/// - the queued root is claimed and its spec run again from the top
///   (`crates/agentic/pipeline/src/recovery.rs:1050-1061`, "reaper-requeued
///   roots — the worker re-executes"), so the rows the dead pass held are
///   claimed and relayed (relaying a stored event again adds nothing).
///
/// Past a third claim whose worker died the reaper dead-letters the task
/// (`crud/queue.rs:831-844`), and a pass that *fails* rather than dies is
/// not retried by the queue (the relay task has no retry policy; it reopens
/// the mark instead, [`reopen`]). Either way no pass drained past the rows
/// it held, so the mark is still due and the sweep's next pass relays them.
pub async fn mark_drained(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    started: DateTime<Utc>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_outbox_marks \
            SET drained_at = GREATEST(COALESCE(drained_at, '-infinity'), $3), \
                pending_since = CASE \
                    WHEN GREATEST(COALESCE(drained_at, '-infinity'), $3) >= published_at \
                    THEN NULL ELSE GREATEST(pending_since, $3) END \
          WHERE org_id = $1 AND environment = $2",
        [org_id.into(), environment.into(), started.into()],
    ))
    .await
    .map(|_| ())
}

/// Move `org_id`'s `environment` wait up to `since`, never back: a pass that
/// succeeded left rows waiting — held by another pass, or past a full batch —
/// the oldest of which began waiting at `since` (the pass's start less that
/// row's age, measured in the store). The mark stays due; only its wait
/// follows the oldest row actually waiting. A mark not waiting
/// (`pending_since` null: drained) is left alone.
pub async fn move_wait_up(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    environment: &str,
    since: DateTime<Utc>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_event_outbox_marks \
            SET pending_since = GREATEST(pending_since, $3) \
          WHERE org_id = $1 AND environment = $2 AND pending_since IS NOT NULL",
        [org_id.into(), environment.into(), since.into()],
    ))
    .await
    .map(|_| ())
}

/// A due mark the sweep acts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub org_id: Uuid,
    /// How long work has waited in it: since it went from new or drained to
    /// due (`pending_since`).
    pub waited: Duration,
    /// Published more than the sweep's `settle` ago, so the sweep queues a
    /// relay; a younger one is its publish's nudge to handle.
    pub settled: bool,
}

/// The `environment` marks that are due and either settled (published more
/// than `settle` ago) or waiting longer than `stale_after`, longest waiting
/// first. Reads the platform database only.
pub async fn due(
    db: &impl ConnectionTrait,
    environment: &str,
    settle: Duration,
    stale_after: Duration,
) -> Result<Vec<Due>, DbErr> {
    // A due mark always has `pending_since`; one written before it existed
    // waits from its publish, as the alarm read before.
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT org_id, \
                    EXTRACT(EPOCH FROM now() - COALESCE(pending_since, published_at))::float8 \
                        AS waited, \
                    published_at < now() - make_interval(secs => $2) AS settled \
               FROM app_event_outbox_marks \
              WHERE environment = $1 \
                AND published_at > COALESCE(drained_at, '-infinity') \
                AND (published_at < now() - make_interval(secs => $2) \
                     OR COALESCE(pending_since, published_at) \
                        < now() - make_interval(secs => $3)) \
              ORDER BY COALESCE(pending_since, published_at)",
            [
                environment.into(),
                settle.as_secs_f64().into(),
                stale_after.as_secs_f64().into(),
            ],
        ))
        .await?;
    rows.iter()
        .map(|row| {
            let waited: f64 = row.try_get("", "waited")?;
            Ok(Due {
                org_id: row.try_get("", "org_id")?,
                waited: Duration::try_from_secs_f64(waited.max(0.0)).unwrap_or_default(),
                settled: row.try_get("", "settled")?,
            })
        })
        .collect()
}
