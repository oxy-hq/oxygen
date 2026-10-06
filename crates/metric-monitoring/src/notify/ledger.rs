//! Which events are due an announcement, and the claim that makes each one
//! announced once.
//!
//! `metric_anomaly_notifications` holds one row per (workspace, event,
//! channel): ids and timestamps, never the values or the message. A task
//! [`claim`]s the due events before it posts, so two tasks racing for one
//! workspace announce disjoint sets; it [`mark_delivered`]s them once the post
//! is accepted and [`release`]s them when it is refused. A claim left
//! undelivered by a task that died goes stale after [`RECLAIM_GRACE_MINUTES`]
//! and is taken by the next one.
//!
//! "Once" is therefore once barring a crash between the post and its record —
//! the failure this trades for is a duplicate, not a lost announcement.
//!
//! # What is due
//!
//! An event is due while it holds a bucket that is all of:
//! - still `new` — nobody has acknowledged or dismissed it;
//! - at or above the file's `min_severity`;
//! - **first detected** within [`DETECTION_WINDOW_DAYS`], so turning `notify:`
//!   on does not replay the inbox, and a post Slack refused is retried by the
//!   scans of the next few days and then given up;
//! - **about** a period that ended within [`PERIOD_WINDOW_DAYS`], so a scan
//!   run with `?as_of=` against last spring announces nothing.
//!
//! Only events a scan grouped are announced (`event_id IS NOT NULL`). A row the
//! analytics agent recorded while answering a question has none: `notify:` is
//! written in `.monitor.yml` and speaks for the monitors declared there.

use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, FromQueryResult, Statement, Value};
use uuid::Uuid;

use super::message::Bucket;
use crate::detect::{Severity, severity_rank, severity_rank_case_sql};
use crate::persist::severity_to_str;

/// The only channel there is so far. In the key so that a second one — a
/// digest, an email — claims on its own.
pub const CHANNEL_SLACK: &str = "slack";

/// How long after a bucket is first detected it can still cause an
/// announcement.
pub const DETECTION_WINDOW_DAYS: i64 = 3;
/// How old a bucket's period may be and still be news.
pub const PERIOD_WINDOW_DAYS: i64 = 45;
/// How long an undelivered claim is honoured before another task may take it.
pub const RECLAIM_GRACE_MINUTES: i64 = 15;
/// One workspace's question to the ledger, as of `now`.
#[derive(Debug, Clone, Copy)]
pub struct Due {
    pub workspace_id: Uuid,
    pub min_severity: Severity,
    pub now: DateTime<Utc>,
}

/// The due events, as `$1`–`$4` of [`due_params`].
fn due_events_sql() -> String {
    format!(
        "SELECT DISTINCT event_id FROM metric_anomalies \
         WHERE workspace_id = $1 AND event_id IS NOT NULL AND status = 'new' \
           AND {rank} >= $2 AND detected_at >= $3 AND period_end >= $4",
        rank = severity_rank_case_sql(),
    )
}

fn due_params(due: Due) -> Vec<Value> {
    let min_rank = i32::from(severity_rank(severity_to_str(due.min_severity)));
    vec![
        due.workspace_id.into(),
        min_rank.into(),
        at(due.now - Duration::days(DETECTION_WINDOW_DAYS)),
        at(due.now - Duration::days(PERIOD_WINDOW_DAYS)),
    ]
}

/// Whether a claim made now would take anything — read by the scan to decide
/// if a delivery task is worth queueing. Advisory: [`claim`] decides.
pub async fn anything_due(db: &impl ConnectionTrait, due: Due) -> Result<bool, DbErr> {
    let sql = format!(
        "SELECT EXISTS (\
           SELECT 1 FROM ({due_events}) d \
           WHERE NOT EXISTS (\
             SELECT 1 FROM metric_anomaly_notifications n \
             WHERE n.workspace_id = $1 AND n.event_id = d.event_id AND n.channel = $5 \
               AND (n.delivered_at IS NOT NULL OR n.claimed_at >= $6))) AS found",
        due_events = due_events_sql(),
    );
    let mut params = due_params(due);
    params.push(CHANNEL_SLACK.into());
    params.push(at(grace_cutoff(due.now)));
    let row = db.query_one_raw(statement(sql, params)).await?;
    match row {
        Some(row) => row.try_get::<bool>("", "found"),
        None => Ok(false),
    }
}

/// Claim every due event not already announced, under `claim_id`, and return
/// how many were taken. Atomic per event: the upsert's `WHERE` is the only
/// thing that lets a second claimant in, and it admits only a stale,
/// undelivered row.
pub async fn claim(
    db: &impl ConnectionTrait,
    due: Due,
    destination: &str,
    claim_id: Uuid,
) -> Result<u64, DbErr> {
    let sql = format!(
        "INSERT INTO metric_anomaly_notifications \
           (workspace_id, event_id, channel, destination, claim_id, claimed_at) \
         SELECT $1, d.event_id, $5::text, $6::text, $7::uuid, $8::timestamptz \
         FROM ({due_events}) d \
         ON CONFLICT (workspace_id, event_id, channel) DO UPDATE \
           SET destination = EXCLUDED.destination, claim_id = EXCLUDED.claim_id, \
               claimed_at = EXCLUDED.claimed_at \
           WHERE metric_anomaly_notifications.delivered_at IS NULL \
             AND metric_anomaly_notifications.claimed_at < $9",
        due_events = due_events_sql(),
    );
    let mut params = due_params(due);
    params.extend([
        CHANNEL_SLACK.into(),
        destination.into(),
        claim_id.into(),
        at(due.now),
        at(grace_cutoff(due.now)),
    ]);
    let result = db.execute_raw(statement(sql, params)).await?;
    Ok(result.rows_affected())
}

/// Every live bucket of the events `claim_id` holds — what the message is
/// built from. Dismissed buckets are left out: someone retired them.
pub async fn claimed_buckets(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    claim_id: Uuid,
) -> Result<Vec<Bucket>, DbErr> {
    let sql = "SELECT a.event_id, a.measure, a.label, a.granularity, a.period_start, \
                      a.observed, a.expected, a.z_score, a.severity, a.dimension_key, \
                      a.cohort_id, a.cohort_label \
               FROM metric_anomalies a \
               JOIN metric_anomaly_notifications n \
                 ON n.workspace_id = a.workspace_id AND n.event_id = a.event_id \
               WHERE n.workspace_id = $1 AND n.claim_id = $2 AND a.status <> 'dismissed'";
    Bucket::find_by_statement(statement(
        sql.to_string(),
        vec![workspace_id.into(), claim_id.into()],
    ))
    .all(db)
    .await
}

/// Record that the claim's message was accepted, so its events are never due
/// again.
pub async fn mark_delivered(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    claim_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    db.execute_raw(statement(
        "UPDATE metric_anomaly_notifications SET delivered_at = $3 \
         WHERE workspace_id = $1 AND claim_id = $2"
            .to_string(),
        vec![workspace_id.into(), claim_id.into(), at(now)],
    ))
    .await
    .map(|_| ())
}

/// Give the claim's events back after a post that did not go out, so the next
/// scan can announce them without waiting out the grace period.
pub async fn release(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    claim_id: Uuid,
) -> Result<(), DbErr> {
    db.execute_raw(statement(
        "DELETE FROM metric_anomaly_notifications \
         WHERE workspace_id = $1 AND claim_id = $2 AND delivered_at IS NULL"
            .to_string(),
        vec![workspace_id.into(), claim_id.into()],
    ))
    .await
    .map(|_| ())
}

/// Drop this workspace's rows whose event no longer exists.
///
/// A row is kept for as long as its event has any bucket left, however old the
/// row is. An event is open-ended: a slide on a weekly or monthly monitor keeps
/// gaining buckets for months, each newly detected, and the ledger row is the
/// only thing that says the slide was already announced. Deleting rows by age
/// would announce such an event again every time its row aged out. So the
/// table is bounded by the inbox it describes — at most one row per event
/// still in `metric_anomalies` — rather than by time.
pub async fn prune(db: &impl ConnectionTrait, workspace_id: Uuid) -> Result<u64, DbErr> {
    db.execute_raw(statement(
        "DELETE FROM metric_anomaly_notifications n \
         WHERE n.workspace_id = $1 \
           AND NOT EXISTS (\
             SELECT 1 FROM metric_anomalies a \
             WHERE a.workspace_id = n.workspace_id AND a.event_id = n.event_id)"
            .to_string(),
        vec![workspace_id.into()],
    ))
    .await
    .map(|r| r.rows_affected())
}

fn grace_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::minutes(RECLAIM_GRACE_MINUTES)
}

fn statement(sql: String, params: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, params)
}

fn at(t: DateTime<Utc>) -> Value {
    t.fixed_offset().into()
}
