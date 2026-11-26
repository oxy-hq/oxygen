//! A detected restore bumps the store's generation (OLTP change stream design
//! §5.6). The writer whose restore check found the restore triggers it; the
//! bump is store-wide:
//!
//! 0. while an earlier bump that timed out backs off
//!    ([`bump_retry_in`](super::generation::bump_retry_in)), none is tried:
//!    the writer fails for the pass, and no lock is queued;
//! 1. the next generation, from the control plane ([`next_generation`]), past
//!    both the stream's and the one Airhouse holds for the writer;
//! 2. one owner transaction, pinned ([`pin_session_sql`]), after renewing the
//!    lease: the stream's lock, which waits out every open capturing
//!    transaction for at most [`BUMP_LOCK_TIMEOUT`], then the stream moved,
//!    the open baselines abandoned, every writer's batches restarted, and a
//!    `bump` baseline requested of every installed table
//!    (`oxy_oltp::changes::generation`). A lock that times out changes
//!    nothing in the store: it backs the bump off on the control plane
//!    ([`BACKOFF_BASE`] doubling to [`BACKOFF_CAP`]) and fails only the
//!    writer, and the pass wakes the mark when the backoff ends;
//! 3. the log's rows of the old generation re-stamped ([`restamp`]), until
//!    the pass's budget runs out: a later pass finishes what it leaves;
//! 4. every restore hold of the store lifted.
//!
//! Any other failure stops the whole pass: the stream may have moved, so no
//! other writer may ship under the generation the pass read.
//!
//! [`restamp_older`] re-stamps the rows of a transaction that spanned a bump,
//! which capture logged under the generation before it.

use std::time::Instant;

use oxy_oltp::changes::{
    BUMP_ABANDON_SQL, BUMP_PROGRESS_SQL, BUMP_REQUEST_SQL, BUMP_STREAM_SQL, END_RESTAMP_SQL,
    RESTAMP_CHUNK_SQL, RESTAMP_OLDER_CHUNK_SQL, bump_lock_sql, pin_session_sql,
};
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use super::check::Shipped;
use super::generation::{bump_failed, bump_retry_in, bump_succeeded, next_generation};
use super::holds::{self, HoldKind, Release};
use super::pass::{Pass, Stop, renew};
use super::store::pg;
use super::{
    BACKOFF_BASE, BACKOFF_CAP, BASELINE_STATEMENT_TIMEOUT, BUMP_LOCK_TIMEOUT, RESTAMP_CHUNK_ROWS,
};

/// What a bump did.
pub(super) struct Bumped {
    /// The stream's generation before.
    pub from: i64,
    /// Its generation now.
    pub to: i64,
    /// The `bump` baselines requested, one per installed table.
    pub requested: u64,
}

/// Bump the store's generation past the stream's and `airhouse`'s, the last
/// batch Airhouse holds for the writer whose check found the restore.
/// `Stop::Failed` fails only that writer: the bump backs off, and the store
/// is unchanged.
pub(super) async fn bump(
    pass: &Pass<'_>,
    owner: &mut Client,
    airhouse: Option<Shipped>,
) -> Result<Bumped, Stop> {
    backing_off(pass).await?;
    let floor = pass.generation.max(airhouse.map_or(0, |s| s.generation));
    renew(pass).await?;
    let to = next_generation(pass.db, pass.org_id, pass.environment, floor)
        .await
        .map_err(|e| Stop::Abort(format!("hand out the store's next generation: {e}")))?;
    let (from, requested) = match move_stream(owner, to).await {
        Err(Stop::Failed(timed_out)) => return Err(back_off(pass, timed_out).await),
        moved => moved?,
    };
    if let Err(e) = bump_succeeded(pass.db, pass.org_id, pass.environment).await {
        tracing::warn!(org_id = %pass.org_id, environment = pass.environment, error = %e, "oltp_changes: could not end the generation bump's backoff");
    }
    if !restamp(owner, from, to, pass.deadline)
        .await
        .map_err(Stop::Abort)?
    {
        tracing::info!(org_id = %pass.org_id, environment = pass.environment, from, to, "oltp_changes: a generation bump's re-stamp ran out of budget; the next pass finishes it");
    }
    let release = Release {
        org_id: pass.org_id,
        environment: pass.environment,
        writer: None,
        tables: None,
        kinds: &[HoldKind::Restore],
    };
    holds::release(pass.db, release).await.map_err(|e| {
        Stop::Abort(format!(
            "lift the store's restore holds after its bump: {e}"
        ))
    })?;
    Ok(Bumped {
        from,
        to,
        requested,
    })
}

/// Fails the writer, with no bump tried and no lock queued, while an earlier
/// bump that timed out backs off.
async fn backing_off(pass: &Pass<'_>) -> Result<(), Stop> {
    let retry_in = bump_retry_in(pass.db, pass.org_id, pass.environment)
        .await
        .map_err(|e| Stop::Failed(format!("read the generation bump's backoff: {e}")))?;
    match retry_in {
        None => Ok(()),
        Some(after) => Err(Stop::Failed(format!(
            "a restore was found, and the generation bump backs off for {}s more after timing \
             out behind a long transaction",
            after.as_secs()
        ))),
    }
}

/// The bump's lock timed out: back the bump off on the control plane, and
/// fail only the writer whose check found the restore, with `timed_out`.
async fn back_off(pass: &Pass<'_>, timed_out: String) -> Stop {
    let backoff = bump_failed(
        pass.db,
        pass.org_id,
        pass.environment,
        BACKOFF_BASE,
        BACKOFF_CAP,
    )
    .await;
    match backoff {
        Ok(retry_in) => {
            tracing::warn!(org_id = %pass.org_id, environment = pass.environment, ?retry_in, "oltp_changes: a generation bump timed out behind a long transaction; it backs off");
            Stop::Failed(timed_out)
        }
        Err(e) => Stop::Failed(format!("{timed_out}; recording its backoff failed: {e}")),
    }
}

/// The bump's owner transaction: the stream moved to `to`, its open
/// baselines abandoned, every writer's batches restarted, and a `bump`
/// baseline requested of each installed table. The old generation, and how
/// many baselines were requested. Only a lock that timed out is
/// `Stop::Failed`.
async fn move_stream(owner: &mut Client, to: i64) -> Result<(i64, u64), Stop> {
    let doing = "bump the store's generation";
    let failed = |e: tokio_postgres::Error| {
        if e.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) {
            Stop::Failed(
                "the generation bump timed out behind a long transaction; it retries after backoff"
                    .into(),
            )
        } else {
            Stop::Abort(pg(doing)(e))
        }
    };
    let txn = owner.transaction().await.map_err(failed)?;
    let pin = pin_session_sql(BUMP_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    txn.batch_execute(&pin).await.map_err(failed)?;
    for sql in bump_lock_sql(BUMP_LOCK_TIMEOUT) {
        txn.batch_execute(&sql).await.map_err(failed)?;
    }
    let Some(row) = txn
        .query_opt(BUMP_STREAM_SQL, &[&to])
        .await
        .map_err(failed)?
    else {
        return Err(Stop::Abort(format!(
            "{doing}: the stream did not move to generation {to}: a re-stamp is running, \
             or it is already there"
        )));
    };
    let from: i64 = row.try_get(0).map_err(failed)?;
    txn.execute(BUMP_ABANDON_SQL, &[]).await.map_err(failed)?;
    txn.execute(BUMP_PROGRESS_SQL, &[]).await.map_err(failed)?;
    let requested = txn
        .execute(BUMP_REQUEST_SQL, &[&to])
        .await
        .map_err(failed)?;
    txn.commit().await.map_err(failed)?;
    Ok((from, requested))
}

/// Re-stamp the log's rows of generation `from` as `to`, at most
/// [`RESTAMP_CHUNK_ROWS`] a chunk, each chunk its own pinned owner
/// transaction, starting none after `deadline`. `Ok(true)` when every old row
/// moved and `restamping` was cleared.
///
/// Its chunks need no renewal of the lease: none starts after the pass's
/// budget, which ends inside the lease's TTL from the renewal or the
/// acquisition before it.
pub(super) async fn restamp(
    owner: &mut Client,
    from: i64,
    to: i64,
    deadline: Instant,
) -> Result<bool, String> {
    let doing = "re-stamp the log's rows of the old generation";
    while Instant::now() < deadline {
        let txn = owner.transaction().await.map_err(pg(doing))?;
        let pin = pin_session_sql(BUMP_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
        txn.batch_execute(&pin).await.map_err(pg(doing))?;
        let moved = txn
            .execute(RESTAMP_CHUNK_SQL, &[&from, &to, &RESTAMP_CHUNK_ROWS])
            .await
            .map_err(pg(doing))?;
        if moved == 0 {
            txn.execute(END_RESTAMP_SQL, &[&from])
                .await
                .map_err(pg(doing))?;
        }
        txn.commit().await.map_err(pg(doing))?;
        if moved == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Re-stamp every log row of a generation older than `to` as `to`, at most
/// [`RESTAMP_CHUNK_ROWS`] a chunk, each chunk its own pinned owner
/// transaction, starting none after `deadline`: the rows of a transaction
/// that spanned a bump. `Ok(true)` when none is left. Its chunks need no
/// renewal of the lease, as [`restamp`]'s do not.
pub(super) async fn restamp_older(
    owner: &mut Client,
    to: i64,
    deadline: Instant,
) -> Result<bool, String> {
    let doing = "re-stamp the log's rows of older generations";
    while Instant::now() < deadline {
        let txn = owner.transaction().await.map_err(pg(doing))?;
        let pin = pin_session_sql(BUMP_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
        txn.batch_execute(&pin).await.map_err(pg(doing))?;
        let moved = txn
            .execute(RESTAMP_OLDER_CHUNK_SQL, &[&to, &RESTAMP_CHUNK_ROWS])
            .await
            .map_err(pg(doing))?;
        txn.commit().await.map_err(pg(doing))?;
        if moved == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}
