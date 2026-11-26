//! The pass under the lease: steps 1 to 5 of a pass, in the module docs of
//! `relay`. A bump's unfinished re-stamp and the rows of older generations
//! re-stamped, the drift check, the baselines' requests, each waiting
//! writer's batches, then the baselines, unless a writer's restore check
//! bumped the generation.

use std::time::Instant;

use chrono::{DateTime, Utc};
use oxy_oltp::changes::StreamState;
use sea_orm::DatabaseConnection;
use tokio_postgres::Client;
use uuid::Uuid;

use super::bump::{restamp, restamp_older};
use super::pass::{self, Pass, Stop};
use super::settle::Ran;
use super::store::{self, Opened, Waiting};
use super::{Limits, SINK_TIMEOUT, baseline, drift, generation, holds, marks, sink};
use crate::app_events::marks::platform_now;

/// The pass under the lease. With no store that captures (none, inactive, or
/// below step 8) or no `stream` row, nothing can be waiting, since no log row
/// exists without a stream row: the pass is a drain that shipped nothing, so
/// the sweep stops waking the store. A bump's re-stamp an earlier pass left
/// is finished first, then any row of an older generation re-stamped; one
/// the budget cuts short ends the pass with rows left.
pub(super) async fn run(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    limits: &Limits,
    lease: &marks::Lease,
) -> Result<Ran, String> {
    let deadline = Instant::now() + limits.budget;
    let started = platform_now(db)
        .await
        .map_err(|e| format!("read the platform clock: {e}"))?;
    let Some(Opened {
        mut owner,
        database,
    }) = store::open(db, org_id).await?
    else {
        tracing::debug!(%org_id, "oltp_changes: the org has no store that captures; nothing to relay");
        return Ok(Ran::nothing(started));
    };
    let Some(stream) = store::stream(&owner).await? else {
        tracing::debug!(%org_id, "oltp_changes: capture was never installed; nothing to relay");
        return Ok(Ran::nothing(started));
    };
    if !restamped(&mut owner, org_id, environment, &stream, deadline).await? {
        return Ok(Ran {
            left: true,
            ..Ran::nothing(started)
        });
    }
    let holds = holds::read(db, org_id, environment)
        .await
        .map_err(|e| format!("read the relay's holds: {e}"))?;
    let sink = sink::Bounded::new(sink::sink(), SINK_TIMEOUT);
    let pass = Pass {
        db,
        org_id,
        environment,
        database: &database,
        lease,
        limits,
        deadline,
        generation: stream.generation,
        holds: &holds,
        sink: &sink,
    };
    check_drift(&pass, &mut owner, &stream).await;
    let requests_failed = request_baselines(&owner, org_id, environment, &database).await;
    let waiting = store::waiting(&owner, stream.generation).await?;
    let mut ran = ship_all(&pass, &mut owner, &waiting, started).await?;
    ran.failed.extend(requests_failed);
    rerender_views(&pass, &mut owner).await?;
    if !ran.stats.bumped {
        take_baselines(&pass, &mut owner, &mut ran).await?;
    }
    wake_for_bump(&pass, &mut ran).await;
    Ok(ran)
}

/// Steps 1 and 2: a bump's re-stamp an earlier pass left unfinished, then
/// any row of an older generation (a transaction that spanned a bump), which
/// logs `oltp_changes_old_generation_row`, each re-stamped into the stream's
/// generation to ship with it. `false` when the budget ran out first.
async fn restamped(
    owner: &mut Client,
    org_id: Uuid,
    environment: &str,
    stream: &StreamState,
    deadline: Instant,
) -> Result<bool, String> {
    let generation = stream.generation;
    if let Some(from) = stream.restamping
        && !restamp(owner, from, generation, deadline).await?
    {
        tracing::info!(%org_id, from, to = generation, "oltp_changes: a generation bump's re-stamp ran out of budget; the next pass finishes it");
        return Ok(false);
    }
    let older = store::older_generation_rows(owner, generation).await?;
    if older == 0 {
        return Ok(true);
    }
    alarm!(environment, %org_id, environment, rows = older, generation, "oltp_changes_old_generation_row");
    let done = restamp_older(owner, generation, deadline).await?;
    if !done {
        tracing::info!(%org_id, generation, "oltp_changes: re-stamping the rows of older generations ran out of budget; the next pass finishes it");
    }
    Ok(done)
}

/// A restore whose bump backs off wakes the mark when it may be tried again
/// ([`generation::bump_retry_in`]), so a quiet store retries it. Only a pass
/// with a writer failed can have found one.
async fn wake_for_bump(pass: &Pass<'_>, ran: &mut Ran) {
    if ran.failed.is_empty() {
        return;
    }
    match generation::bump_retry_in(pass.db, pass.org_id, pass.environment).await {
        Ok(Some(retry_in)) => {
            ran.wake_in = Some(ran.wake_in.map_or(retry_in, |wake| wake.min(retry_in)));
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(org_id = %pass.org_id, environment = pass.environment, error = %e, "oltp_changes: could not read the generation bump's backoff to wake the mark");
        }
    }
}

/// The start of step 3: the store's capture checked for drift, and what
/// drifted repaired. A failure never stops the shipping: it is logged.
async fn check_drift(pass: &Pass<'_>, owner: &mut Client, stream: &StreamState) {
    if let Err(error) = drift::check_and_repair(pass, owner, stream).await {
        tracing::warn!(org_id = %pass.org_id, environment = pass.environment, database = pass.database, %error, "oltp_changes: could not check the store's capture for drift; shipping anyway");
    }
}

/// Step 3: the orphaned baselines abandoned and the missing ones requested.
/// A failure never stops the shipping: it is logged, and returned as
/// `baselines: <error>` for the pass's `failed`, which backs the mark off.
async fn request_baselines(
    owner: &Client,
    org_id: Uuid,
    environment: &str,
    database: &str,
) -> Option<String> {
    let error = requests(owner, org_id, environment, database).await.err()?;
    tracing::warn!(%org_id, environment, database, %error, "oltp_changes: could not request the store's baselines; shipping anyway");
    Some(format!("baselines: {error}"))
}

/// [`request_baselines`]'s statements, each request logged.
async fn requests(
    owner: &Client,
    org_id: Uuid,
    environment: &str,
    database: &str,
) -> Result<(), String> {
    let abandoned = baseline::abandon_orphans(owner).await?;
    if abandoned > 0 {
        tracing::info!(%org_id, environment, database, abandoned, "oltp_changes: abandoned the baselines of tables no longer captured");
    }
    for (writer, table) in baseline::request_missing(owner).await? {
        tracing::info!(%org_id, environment, database, writer, table, "oltp_changes: requested a baseline of a table with none in the stream's generation");
    }
    Ok(())
}

/// Step 4 of a pass: each waiting writer's batches, while the budget lasts.
/// A lost lease stops them all, and so does a generation bump, with rows
/// left: the writers after it ship in the new generation, next pass.
async fn ship_all(
    pass: &Pass<'_>,
    owner: &mut Client,
    waiting: &[Waiting],
    started: DateTime<Utc>,
) -> Result<Ran, String> {
    let mut ran = Ran::nothing(started);
    for writer in waiting {
        if Instant::now() >= pass.deadline {
            ran.left = true;
            break;
        }
        match pass::ship_writer(pass, owner, writer).await {
            Ok(one) => {
                ran.stats.add(one);
                ran.left |= one.left;
                if one.bumped {
                    ran.left = true;
                    ran.stats.bumped = true;
                    break;
                }
            }
            Err(Stop::Failed(error)) => {
                tracing::warn!(org_id = %pass.org_id, environment = pass.environment, writer = %writer.writer, %error, "oltp_changes: could not relay one writer; relaying the others");
                ran.failed.push(format!("{}: {error}", writer.writer));
            }
            Err(Stop::Abort(error)) => return Err(error),
        }
    }
    Ok(ran)
}

/// After the shipping: the current-state views an earlier render left
/// pending, made again, best effort. Only a lost lease stops the pass.
async fn rerender_views(pass: &Pass<'_>, owner: &mut Client) -> Result<(), String> {
    match pass::rerender_pending(pass, owner).await {
        Err(Stop::Abort(error)) => Err(error),
        Ok(()) | Err(Stop::Failed(_)) => Ok(()),
    }
}

/// Step 5 of a pass: the store's baselines, after its shipping, added to
/// `ran`. A baseline left open, or one that wrote log rows, is rows left. A
/// lost lease stops the pass; any other failure is a failure of what it
/// names, and backs the mark off.
async fn take_baselines(pass: &Pass<'_>, owner: &mut Client, ran: &mut Ran) -> Result<(), String> {
    let baselines = match baseline::run(pass, owner).await {
        Ok(baselines) => baselines,
        Err(Stop::Abort(error)) => return Err(error),
        Err(Stop::Failed(error)) => {
            tracing::warn!(org_id = %pass.org_id, environment = pass.environment, database = pass.database, %error, "oltp_changes: could not take the store's baselines");
            ran.failed.push(format!("baselines: {error}"));
            return Ok(());
        }
    };
    ran.stats.add_baselines(&baselines);
    ran.left |= baselines.open;
    ran.failed.extend(baselines.failed);
    ran.wake_in = baselines.wake_in;
    Ok(())
}
