//! Run retention: finished runs older than `OXY_RUN_RETENTION_DAYS` (90 by
//! default) are deleted, with everything that hangs off them.
//!
//! `agentic_runs` kept every run ever made, and with it every event, outcome
//! and extension row; nothing else bounded it. What goes is a **finished root
//! run** (`done`, `failed`, `cancelled`, `timed_out`) last touched before the
//! window, together with its whole tree of child runs. Deleting the tree here,
//! rather than leaning on the `parent_run_id` cascade, is deliberate: that
//! foreign key is only created on a fresh table (`agentic-runtime`'s
//! `migration.rs`), so an older database may not have it.
//!
//! A run that is still part of something live is kept, whatever its age:
//!
//! * **A chat thread is all or nothing.** Its runs go only once the whole
//!   thread has been idle for the window: every run in it finished, none
//!   touched since. A thread in use keeps every turn, because the UI and the
//!   model's follow-up context both read the thread's runs.
//! * **A preview's runs** (`workspace_preview_runs`): a check's verdict and a
//!   build's report are read from the run row, and preview rows outlive the
//!   window.
//! * **An experiment's estimate** (`experiment_runs.estimate_task_id`): a
//!   missing estimate run reads as still running.
//! * **A dispatched experiment flip** and **a pending experiment proposal**:
//!   both settle by joining their run.
//! * **A custom-app automation run still `running`**: its settle sweep reads
//!   the run.
//!
//! Every other reference to a run id is a link or a record, and dangles
//! harmlessly (`agentic_schedules.last_run_id`, `notification_sends.run_id`,
//! `backfill_checkpoints.run_id`, which reseeds, and the preview schema drop
//! claims, which a missing run releases).
//!
//! **Where it runs.** Its own loop ([`spawn`]), started beside the Airhouse
//! tenant sweeper on the nodes that drive the queue, and stopped by the same
//! shutdown token. Not inside the periodic driver's tick: a pass can take
//! [`BUDGET`], and every second of it would make the schedules and stranded
//! runs that tick drives that much late. A pass deletes in batches of
//! [`BATCH`] roots under `FOR UPDATE SKIP LOCKED`, so nodes prune disjoint
//! rows; one that ends on its budget with more to do is followed by another
//! after [`BACKLOG_PAUSE`], otherwise the next is an [`IDLE_INTERVAL`] away.
//! Each batch is one statement, so a pass cut short by shutdown loses nothing.
//! It is not a `TaskSpec`: a pass keeps no state, a failed one is simply the
//! next one, and a queue row per pass would add a durable record of nothing.

use std::time::{Duration, Instant};

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use tokio_util::sync::CancellationToken;

/// Root runs claimed and deleted per statement.
pub const BATCH: i64 = 200;
/// How long one pass may keep deleting.
pub const BUDGET: Duration = Duration::from_secs(10);
/// Pause between passes while a backlog drains.
const BACKLOG_PAUSE: Duration = Duration::from_secs(5);
/// Pause between passes once a pass found nothing more to delete.
const IDLE_INTERVAL: Duration = Duration::from_secs(3600);
/// Before the first pass, so it doesn't land in the boot storm.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(300);

/// What one pass deleted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pruned {
    /// Root runs deleted.
    pub roots: u64,
    /// Runs deleted, roots and their children together.
    pub runs: u64,
    /// The pass stopped on its budget with a full last batch: more is due.
    pub more: bool,
}

/// Start the retention loop, unless `OXY_RUN_RETENTION_DAYS` turns it off.
/// The setting is read once, here: it is fixed for the process's life.
pub(crate) fn spawn(db: DatabaseConnection, shutdown: CancellationToken) {
    let Some(days) = oxy_app_core::config::BootConfig::current()
        .run_retention
        .days()
    else {
        tracing::info!(target: "background", "run retention is off; every run is kept");
        return;
    };
    tokio::spawn(async move {
        let mut pause = FIRST_PASS_DELAY;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(pause) => {}
            }
            let outcome = tokio::select! {
                _ = shutdown.cancelled() => break,
                outcome = prune(&db, days, BATCH, BUDGET) => outcome,
            };
            pause = next_pause(&outcome);
            report(days, outcome);
        }
    });
}

fn next_pause(outcome: &Result<Pruned, DbErr>) -> Duration {
    match outcome {
        Ok(Pruned { more: true, .. }) => BACKLOG_PAUSE,
        _ => IDLE_INTERVAL,
    }
}

fn report(days: u32, outcome: Result<Pruned, DbErr>) {
    match outcome {
        Ok(p) if p.runs > 0 => tracing::info!(
            target: "background",
            roots = p.roots,
            runs = p.runs,
            more = p.more,
            retention_days = days,
            "run retention: deleted finished runs past the window"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(
            target: "background",
            error = %e,
            "run retention pass failed; it is retried in an hour"
        ),
    }
}

/// Delete finished runs past `days`, `batch` roots at a time, until a batch
/// comes back short or `budget` is spent.
pub async fn prune(
    db: &DatabaseConnection,
    days: u32,
    batch: i64,
    budget: Duration,
) -> Result<Pruned, DbErr> {
    // A zero window is every finished run. The setting maps 0 to "off", so
    // this is a caller's bug; refuse it rather than empty the table.
    if days == 0 {
        return Err(DbErr::Custom(
            "run retention: a 0-day window would delete every run".into(),
        ));
    }
    let days = i32::try_from(days)
        .map_err(|_| DbErr::Custom(format!("run retention: {days} days is out of range")))?;
    let started = Instant::now();
    let mut pruned = Pruned::default();
    loop {
        let (roots, runs) = delete_batch(db, days, batch).await?;
        pruned.roots += roots;
        pruned.runs += runs;
        if (roots as i64) < batch {
            return Ok(pruned);
        }
        if started.elapsed() >= budget {
            pruned.more = true;
            return Ok(pruned);
        }
    }
}

/// One batch: claim up to `batch` prunable roots, delete them and their trees.
/// Returns `(roots, runs)` deleted.
async fn delete_batch(db: &DatabaseConnection, days: i32, batch: i64) -> Result<(u64, u64), DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            DELETE_BATCH_SQL,
            [days.into(), batch.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("run retention: the delete returned no row".into()))?;
    let roots: i64 = row.try_get("", "roots")?;
    let runs: i64 = row.try_get("", "runs")?;
    Ok((roots as u64, runs as u64))
}

/// `$1` the window in days, `$2` the batch size. The status list and the
/// `parent_run_id IS NULL` predicate match `idx_agentic_runs_retention`
/// exactly, which is what lets Postgres walk that index oldest first.
///
/// The thread rule has no `r.thread_id IS NULL OR`: a NULL thread matches no
/// `t.thread_id = r.thread_id`, so a threadless run passes anyway, and the OR
/// turned the anti-join into a hashed subplan that read the whole table on
/// every batch. As written it probes `idx_agentic_runs_thread_updated` per
/// candidate.
const DELETE_BATCH_SQL: &str = "\
WITH RECURSIVE roots AS (
    SELECT r.id
    FROM agentic_runs r
    WHERE r.parent_run_id IS NULL
      AND r.task_status IN ('done', 'failed', 'cancelled', 'timed_out')
      AND r.updated_at < now() - make_interval(days => $1)
      AND NOT EXISTS (SELECT 1 FROM agentic_runs t
                      WHERE t.thread_id = r.thread_id
                        AND (t.updated_at >= now() - make_interval(days => $1)
                             OR t.task_status IS NULL
                             OR t.task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')))
      AND NOT EXISTS (SELECT 1 FROM workspace_preview_runs p WHERE p.run_id = r.id)
      AND NOT EXISTS (SELECT 1 FROM experiment_runs e WHERE e.estimate_task_id::text = r.id)
      AND NOT EXISTS (SELECT 1 FROM experiment_flips f
                      WHERE f.agentic_run_id::text = r.id AND f.status = 'dispatched')
      AND NOT EXISTS (SELECT 1 FROM experiment_proposals x
                      WHERE x.id::text = r.id AND x.status = 'pending')
      AND NOT EXISTS (SELECT 1 FROM customer_app_automation_runs c
                      WHERE c.id::text = r.id AND c.status = 'running')
    ORDER BY r.updated_at
    LIMIT $2
    FOR UPDATE OF r SKIP LOCKED
),
tree AS (
    SELECT id FROM roots
    UNION
    SELECT c.id FROM agentic_runs c JOIN tree ON c.parent_run_id = tree.id
),
gone AS (
    DELETE FROM agentic_runs WHERE id IN (SELECT id FROM tree)
    RETURNING parent_run_id IS NULL AS root
)
SELECT count(*) FILTER (WHERE root) AS roots, count(*) AS runs FROM gone";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backlog_is_followed_up_soon_and_a_drained_table_waits_an_hour() {
        let more = Ok(Pruned {
            roots: 200,
            runs: 200,
            more: true,
        });
        let done = Ok(Pruned::default());
        let failed = Err(DbErr::Custom("down".into()));
        assert_eq!(next_pause(&more), BACKLOG_PAUSE);
        assert_eq!(next_pause(&done), IDLE_INTERVAL);
        assert_eq!(next_pause(&failed), IDLE_INTERVAL);
    }

    /// The prune only walks `idx_agentic_runs_retention` if it states the
    /// index's predicate; a status added to one and not the other turns every
    /// pass into a table scan.
    #[test]
    fn the_prune_states_the_retention_index_predicate() {
        let index = agentic_runtime::migration::RUN_RETENTION_INDEX_SQL;
        for clause in [
            "parent_run_id IS NULL",
            "task_status IN ('done', 'failed', 'cancelled', 'timed_out')",
        ] {
            assert!(index.contains(clause), "index lost {clause}");
            assert!(
                DELETE_BATCH_SQL.contains(&format!("r.{clause}")),
                "prune lost {clause}"
            );
        }
    }
}
