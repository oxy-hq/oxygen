//! Schedule rows follow compiled config — and are re-read only when it
//! could have changed.
//!
//! A workspace's `health_eval`, `preagg_cycle` and `monitor_scan` schedule
//! rows are derived from its promoted compiled revision (`config.yml`'s
//! `health_check:` and `pre_aggregations:` blocks, `.monitor.yml`'s
//! `schedule:`). The steady-state writer is the compile worker, which
//! reconciles health and pre-aggregation when a compile promotes. This module
//! is the periodic safety net behind it, and it is gated: a claimed workspace
//! is reconciled when `workspaces.current_revision_id` differs from the
//! revision its row was last reconciled from, or when
//! [`FORCED_RECONCILE_INTERVAL`] has passed since then. Otherwise nothing is
//! read — the config cannot have changed without the revision changing, and
//! before the gate the three reads ran for every workspace on every node on
//! every tick.
//!
//! The one-time startup work that used to ride on the same loop — dropping
//! the legacy cross-tenant health row and pruning schedules of deleted
//! workspaces — is [`startup_housekeeping`], still run once per boot.

use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::claim::Claimed;
use super::{claim, metrics, monitors};

/// How long a workspace's schedules may go without a reconcile when its
/// promoted revision has not changed. A safety net for a row edited or
/// deleted by hand, and for a promote whose compile-time reconcile failed; an
/// hour keeps that net while costing each workspace three reads an hour
/// rather than three every tick.
pub(super) const FORCED_RECONCILE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Whether the schedules derived from `current` need re-reading, given that
/// they were last reconciled from `last` at `last_at`.
///
/// Pure, so the rule can be tested without a database: a different revision
/// (including no revision yet, or the first tick's `None` row) reconciles; the
/// same revision reconciles once [`FORCED_RECONCILE_INTERVAL`] has passed.
pub(super) fn reconcile_due(
    current: Option<Uuid>,
    last: Option<Uuid>,
    last_at: Option<DateTime<FixedOffset>>,
    now: DateTime<Utc>,
) -> bool {
    if current != last {
        return true;
    }
    match last_at {
        None => true,
        Some(at) => {
            let since = now.signed_duration_since(at.with_timezone(&Utc));
            since.to_std().is_ok_and(|d| d >= FORCED_RECONCILE_INTERVAL)
        }
    }
}

/// Reconcile a claimed workspace's schedule rows from its compiled revision
/// when [`reconcile_due`] says so; otherwise count the skip and do nothing.
///
/// The three reconciles keep their own contracts: health and pre-aggregation
/// through `compile_worker` (a read that says nothing leaves the row alone),
/// monitors through [`monitors::bootstrap_from_compiled`] (create-only plus
/// the timezone reconcile). Each reports whether it is **settled** — written,
/// or nothing a retry could write (no promoted revision, no `.monitor.yml`, a
/// definition or cron the revision got wrong) — and the row is stamped only
/// when all three are. A step that failed on a database blip
/// leaves the row unstamped, so the next claim (one interval later) retries
/// instead of waiting out [`FORCED_RECONCILE_INTERVAL`]; this matters most
/// for monitors, whose `monitor_scan` rows nothing else creates. A workspace
/// with nothing compiled is settled, so it is not re-read every tick — its
/// first promote changes the revision and reopens the gate.
pub(super) async fn reconcile_workspace(db: &DatabaseConnection, ws: &Claimed) {
    if !reconcile_due(
        ws.current_revision_id,
        ws.last_reconciled_revision_id,
        ws.last_reconciled_at,
        Utc::now(),
    ) {
        metrics::record_reconcile_skipped();
        return;
    }
    tracing::debug!(
        target: "recovery",
        workspace_id = %ws.workspace_id,
        revision = ?ws.current_revision_id,
        "periodic tick: reconciling schedules from the compiled revision"
    );
    let health =
        crate::server::compile_worker::reconcile_health_from_compiled(db, ws.workspace_id).await;
    // The resolved opt-in is the promote path's business, not the tick's:
    // this backfills cadences, it does not seed ticks.
    let preagg = crate::server::compile_worker::reconcile_preagg_from_compiled(db, ws.workspace_id)
        .await
        .settled();
    let monitors =
        monitors::bootstrap_from_compiled(db, ws.workspace_id, ws.current_revision_id).await;
    if health && preagg && monitors {
        claim::record_reconciled(db, ws.workspace_id, ws.current_revision_id).await;
    } else {
        tracing::info!(
            target: "recovery",
            workspace_id = %ws.workspace_id,
            health,
            preagg,
            monitors,
            "periodic tick: a schedule reconcile step did not settle; the row stays \
             unstamped so the next claim retries it"
        );
    }
}

/// One-time startup cleanup of `agentic_schedules`: drop the legacy
/// cross-tenant singleton health row (`target_ref = 'global'`, the old
/// 10-minute sweep) and, in Cloud mode, every schedule whose workspace no
/// longer exists.
///
/// `prune_orphans` gates the destructive sweep to Cloud mode: there the
/// `workspaces` table is the authoritative set, so a row pointing outside it
/// is genuinely orphaned (and an empty table genuinely means zero workspaces).
/// In Local mode the single workspace has no `workspaces` row, so the table is
/// empty and pruning would wrongly delete the local schedules.
///
/// The per-workspace cadence backfill that used to run here moved to the
/// claim-gated [`reconcile_workspace`]. Tick rows persist across restarts, so
/// a boot does not by itself re-reconcile anything: a workspace is reconciled
/// on its next claim when its row is new (no last-reconciled revision), when
/// its promoted revision changed, or when [`FORCED_RECONCILE_INTERVAL`] has
/// passed — the hourly net is what covers a promote whose compile-time
/// reconcile failed, not the restart.
pub(crate) async fn startup_housekeeping(db: &DatabaseConnection, prune_orphans: bool) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};

    if let Err(e) = db
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "DELETE FROM agentic_schedules \
             WHERE target_kind = 'health_eval' AND target_ref = 'global'",
        ))
        .await
    {
        tracing::warn!(
            target: "health_eval",
            error = %e,
            "startup reconcile: failed to delete legacy global health row"
        );
    }
    if !prune_orphans {
        return;
    }
    // Workspaces carry no FK to `agentic_schedules`, so a deleted workspace
    // leaves its rows behind — a `health_eval` (or `monitor_scan`) row keeps
    // enqueuing tasks for a workspace that no longer exists, which the worker
    // fails and eventually dead-letters, piling up. New deletes clean up inline
    // via `cleanup_workspace_schedules`; this drains rows orphaned before that.
    let live_ids: Vec<Uuid> = match entity::workspaces::Entity::find().all(db).await {
        Ok(ws) => ws.into_iter().map(|w| w.id).collect(),
        Err(e) => {
            tracing::warn!(
                target: "health_eval",
                error = %e,
                "startup reconcile: failed to list workspaces; skipping the orphan prune"
            );
            return;
        }
    };
    prune_orphaned_schedules(db, &live_ids).await;
}

/// Delete every schedule row (any `target_kind`) whose workspace is not in
/// `live_ids`. An empty `live_ids` means no workspaces exist, so every row is
/// an orphan — which is why the caller gates this to Cloud mode. Best-effort;
/// a failure is logged and skipped.
async fn prune_orphaned_schedules(db: &DatabaseConnection, live_ids: &[Uuid]) {
    use agentic_runtime::entity::schedule;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let mut q = schedule::Entity::delete_many();
    if !live_ids.is_empty() {
        q = q.filter(schedule::Column::WorkspaceId.is_not_in(live_ids.iter().copied()));
    }
    match q.exec(db).await {
        Ok(res) if res.rows_affected > 0 => tracing::info!(
            target: "health_eval",
            removed = res.rows_affected,
            "startup reconcile: pruned orphaned schedules for deleted workspaces"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(
            target: "health_eval",
            error = %e,
            "startup reconcile: failed to prune orphaned schedules"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now: DateTime<Utc>, ago: Duration) -> Option<DateTime<FixedOffset>> {
        Some((now - chrono::Duration::from_std(ago).unwrap()).fixed_offset())
    }

    #[test]
    fn a_changed_revision_reconciles_however_recent_the_last_one_was() {
        let now = Utc::now();
        let (a, b) = (Some(Uuid::new_v4()), Some(Uuid::new_v4()));
        assert!(reconcile_due(a, b, at(now, Duration::from_secs(1)), now));
        assert!(reconcile_due(a, None, at(now, Duration::from_secs(1)), now));
        assert!(reconcile_due(None, a, at(now, Duration::from_secs(1)), now));
    }

    #[test]
    fn a_fresh_row_reconciles_even_with_nothing_compiled() {
        assert!(reconcile_due(None, None, None, Utc::now()));
    }

    #[test]
    fn an_unchanged_revision_waits_for_the_forced_interval() {
        let now = Utc::now();
        let rev = Some(Uuid::new_v4());
        assert!(!reconcile_due(
            rev,
            rev,
            at(now, Duration::from_secs(30)),
            now
        ));
        assert!(!reconcile_due(
            rev,
            rev,
            at(now, FORCED_RECONCILE_INTERVAL - Duration::from_secs(1)),
            now
        ));
        assert!(reconcile_due(
            rev,
            rev,
            at(now, FORCED_RECONCILE_INTERVAL),
            now
        ));
        // Nothing compiled, and still nothing compiled: the same gate applies,
        // so an uncompiled workspace is not re-read every tick.
        assert!(!reconcile_due(
            None,
            None,
            at(now, Duration::from_secs(30)),
            now
        ));
    }
}
