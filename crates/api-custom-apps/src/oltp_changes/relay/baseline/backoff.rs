//! A baseline backed off after a begin or a chunk could not take its locks,
//! its rows kept changing under the copy, or its key has no btree order
//! ([`BACKOFF_SQL`]): it waits out its `retry_at` without failing the writer,
//! and the pass wakes the mark after that delay (`settle`).

use std::time::Duration;

use oxy_oltp::changes::{BACKOFF_SQL, pin_session_sql};
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use super::step::{Step, failed_on};
use super::{Open, Sessions};
use crate::oltp_changes::relay::pass::{Pass, Stop};
use crate::oltp_changes::relay::store::pg;
use crate::oltp_changes::relay::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT};

/// What [`BACKOFF_SQL`] recorded.
struct Backoff {
    attempts: i32,
    /// Seconds until `retry_at`.
    delay: f64,
}

/// The delay [`BACKOFF_SQL`] gives a baseline that has backed off `attempts`
/// times before: `5 s · 2^attempts`, at most 30 minutes.
fn delay(attempts: i32) -> Duration {
    let doubled = 5.0 * 2f64.powi(attempts.clamp(0, 20));
    Duration::from_secs_f64(doubled.min(1800.0))
}

/// Why `step` backs a baseline of `table` off, for its log line; `None` for
/// a step that does not.
fn reason(step: Step, table: &str) -> Option<String> {
    match step {
        Step::LockTimeout => Some(format!("baseline of {table} could not take its lock")),
        Step::Conflicted => Some(format!(
            "could not copy {table}: rows kept changing under the copy"
        )),
        Step::Unordered => Some(format!(
            "could not copy {table}: its key has no btree order"
        )),
        Step::Began(_) | Step::NotYet | Step::Copied(_) | Step::Completed | Step::Abandoned => None,
    }
}

/// Back `b` off after `step`, in a pinned owner transaction of its own,
/// logged once per attempt. How long until it is retried, which the pass
/// wakes the mark after; `None` when nothing was backed off. A backoff that
/// cannot take its lock or outlasts its statement timeout is logged, and
/// still wakes the mark after the delay it would have had.
pub(super) async fn back_off(
    pass: &Pass<'_>,
    s: &mut Sessions<'_>,
    b: &Open,
    step: Step,
) -> Result<Option<Duration>, Stop> {
    let Some(reason) = reason(step, &b.table) else {
        return Ok(None);
    };
    let backoff = match recorded(s.owner, b.id).await {
        Ok(backoff) => backoff,
        Err(e) if timed_out(&e) => {
            let error = oxy_oltp::connect::pg_detail(&e);
            tracing::warn!(org_id = %pass.org_id, environment = pass.environment, database = pass.database, writer = %s.name, table = %b.table, %error, %reason, "oltp_changes: could not back a baseline off; trying it again after its delay");
            return Ok(Some(delay(b.attempts)));
        }
        Err(e) => return Err(failed_on(&b.table, pg("back the baseline off")(e))),
    };
    let Some(Backoff { attempts, delay }) = backoff else {
        return Ok(None);
    };
    tracing::warn!(org_id = %pass.org_id, environment = pass.environment, database = pass.database, writer = %s.name, table = %b.table, attempts, retry_in_seconds = delay, %reason, "oltp_changes: a baseline backs off");
    Ok(Duration::try_from_secs_f64(delay.max(0.0)).ok())
}

/// [`BACKOFF_SQL`] for baseline `id`, in a transaction pinned as every
/// step's is. `None` when it is no longer open.
async fn recorded(owner: &mut Client, id: i64) -> Result<Option<Backoff>, tokio_postgres::Error> {
    let t = owner.transaction().await?;
    t.batch_execute(&pin_session_sql(
        BEGIN_LOCK_TIMEOUT,
        BASELINE_STATEMENT_TIMEOUT,
    ))
    .await?;
    let row = t.query_opt(BACKOFF_SQL, &[&id]).await?;
    t.commit().await?;
    row.map(|row| {
        Ok(Backoff {
            attempts: row.try_get("attempts")?,
            delay: row.try_get("delay")?,
        })
    })
    .transpose()
}

/// A lock timeout (55P03) or a statement timeout (57014).
fn timed_out(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) || e.code() == Some(&SqlState::QUERY_CANCELED)
}
