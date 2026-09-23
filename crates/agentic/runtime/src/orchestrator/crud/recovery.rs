//! Startup cleanup and resume enumeration for the `agentic_runs` table.

use sea_orm::{
    ActiveValue::*, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DbErr, EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;

use crate::lifecycle::crud::events::get_max_seq;
use crate::lifecycle::crud::{DRIVER_LEASE_TTL_SECS, now, transition_run};
use crate::lifecycle::entity::run;

/// The non-terminal statuses the driver loops' poll selects on.
///
/// **Coupled to `idx_agentic_runs_pending_global`**, the partial index that
/// makes that poll cheap (`migration::AddPendingGlobalRunsIndex`). Postgres only
/// uses a partial index when it can prove the query implies the index
/// predicate, so a status added or removed here without a matching *new*
/// migration silently drops the index and returns the poll to a full index scan
/// — 17ms per execution at ~13/second, which is the shape of the 2026-09-01
/// outage. A shipped migration cannot be edited, so the coupling is enforced by
/// `migration`'s `pending_global_index_covers_every_polled_status` test.
pub const PENDING_GLOBAL_STATUSES: [&str; 6] = [
    "running",
    "delegating",
    "waiting_on_child",
    "waiting_on_children",
    "needs_resume",
    "shutdown",
];

/// Render [`PENDING_GLOBAL_STATUSES`] as the body of a SQL `IN (...)` list.
pub fn pending_global_status_sql() -> String {
    PENDING_GLOBAL_STATUSES
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Find root runs that are still active (not terminal) for restart recovery.
pub async fn get_active_root_runs(db: &DatabaseConnection) -> Result<Vec<run::Model>, DbErr> {
    run::Entity::find()
        .filter(run::Column::ParentRunId.is_null())
        .filter(run::Column::TaskStatus.is_in([
            "running",
            "suspended_human",
            "waiting_on_child",
            "waiting_on_children",
        ]))
        .all(db)
        .await
}

pub async fn cleanup_stale_runs(db: &DatabaseConnection) -> Result<u64, DbErr> {
    // Find all runs with non-terminal task_status.
    let stale_runs = run::Entity::find()
        .filter(
            Condition::any()
                .add(run::Column::TaskStatus.eq("running"))
                .add(run::Column::TaskStatus.eq("awaiting_input"))
                .add(run::Column::TaskStatus.eq("delegating"))
                .add(run::Column::TaskStatus.eq("waiting_on_child"))
                .add(run::Column::TaskStatus.eq("waiting_on_children"))
                .add(run::Column::TaskStatus.eq("needs_resume"))
                .add(run::Column::TaskStatus.eq("shutdown")),
        )
        .all(db)
        .await?;

    let mut reconciled = 0;
    for r in stale_runs {
        // Runs waiting on children whose delegation was interrupted (e.g. server
        // crash) should be failed — the child task is gone and won't complete.
        if matches!(
            r.task_status.as_deref(),
            Some("waiting_on_child") | Some("waiting_on_children")
        ) {
            let update = run::ActiveModel {
                id: Set(r.id.clone()),
                task_status: Set(Some("failed".to_string())),
                error_message: Set(Some(
                    "server restarted: delegation was interrupted".to_string(),
                )),
                updated_at: Set(now()),
                ..Default::default()
            };
            run::Entity::update(update).exec(db).await?;
            reconciled += 1;
            continue;
        }

        // Suspended runs: leave as-is for recovery.
        if matches!(r.task_status.as_deref(), Some("awaiting_input")) {
            continue;
        }

        let event_count = get_max_seq(db, &r.id).await.unwrap_or(-1) + 1;
        if event_count == 0 && r.parent_run_id.is_none() {
            // Root with zero events. Two sub-cases:
            //
            // (a) A scheduler-seeded or run-now-seeded Global run that
            //     hasn't been driven yet: its queue entry is still
            //     `queued` with `scope_owned = false`, waiting for the
            //     latency worker / periodic loop to pick it up. Force-
            //     failing this is a regression — the run is valid pending
            //     work, not an orphan.
            //
            // (b) Any other zero-event root with no queued entry: stale
            //     placeholder from a request that died before enqueuing.
            //     Safe to fail.
            //
            // The discriminator is whether a `queued` queue row exists
            // for this run id.
            let has_queued = crate::orchestrator::crud::queue::get_queue_entry(db, &r.id)
                .await
                .ok()
                .flatten()
                .map(|q| q.queue_status == "queued")
                .unwrap_or(false);
            if has_queued {
                // Leave as-is; the recovery loop / latency worker will
                // drive it on the next tick.
                continue;
            }
            // (b): never started AND no queued entry — fail it.
            let update = run::ActiveModel {
                id: Set(r.id.clone()),
                task_status: Set(Some("failed".to_string())),
                error_message: Set(Some("server restarted: run never started".to_string())),
                updated_at: Set(now()),
                ..Default::default()
            };
            run::Entity::update(update).exec(db).await?;
            reconciled += 1;
        } else {
            // Has events — mark for resume.
            let update = run::ActiveModel {
                id: Set(r.id.clone()),
                task_status: Set(Some("needs_resume".to_string())),
                error_message: Set(Some(
                    "server restarted: run will be resumed automatically".to_string(),
                )),
                updated_at: Set(now()),
                ..Default::default()
            };
            run::Entity::update(update).exec(db).await?;
            reconciled += 1;
        }
    }

    // Second pass: clean up orphaned child tasks whose parent is terminal.
    let orphans = run::Entity::find()
        .filter(run::Column::ParentRunId.is_not_null())
        .filter(
            Condition::any()
                .add(run::Column::TaskStatus.eq("needs_resume"))
                .add(run::Column::TaskStatus.eq("running"))
                .add(run::Column::TaskStatus.eq("shutdown"))
                .add(run::Column::TaskStatus.eq("waiting_on_children"))
                .add(run::Column::TaskStatus.eq("waiting_on_child"))
                .add(run::Column::TaskStatus.eq("awaiting_input"))
                .add(run::Column::TaskStatus.eq("delegating")),
        )
        .all(db)
        .await?;

    for orphan in orphans {
        // Check if the parent is terminal.
        if let Some(ref parent_id) = orphan.parent_run_id
            && let Some(parent) = run::Entity::find_by_id(parent_id.clone()).one(db).await?
        {
            let parent_terminal =
                matches!(parent.task_status.as_deref(), Some("done") | Some("failed"));
            if parent_terminal {
                let update = run::ActiveModel {
                    id: Set(orphan.id.clone()),
                    task_status: Set(Some("failed".to_string())),
                    error_message: Set(Some(
                        "parent task completed; orphaned child cleaned up".to_string(),
                    )),
                    updated_at: Set(now()),
                    ..Default::default()
                };
                run::Entity::update(update).exec(db).await?;
                reconciled += 1;
            }
        }
    }

    Ok(reconciled)
}

/// Find root runs that are resumable after a server restart.
///
/// Includes tasks marked `"shutdown"` (graceful shutdown — always resumable)
/// and `"needs_resume"` (crash recovery — best effort).
///
/// Excludes runs a *live* driver already owns: a run is only resumable if it
/// is unleased or its driver lease has gone stale past
/// [`DRIVER_LEASE_TTL_SECS`]. This is the F1 guard — without it, calling this
/// on an interval (the Phase 2 global loop) would re-select and double-drive
/// runs that are still in flight.
///
/// `workspace_id` — when `Some`, only return runs owned by that workspace.
/// Cloud-mode startup recovery iterates per workspace and passes the
/// current workspace id; local mode passes `None` (the single workspace
/// is identified by the nil UUID and every other row would also be nil).
pub async fn get_resumable_root_runs(
    db: &DatabaseConnection,
    workspace_id: Option<Uuid>,
) -> Result<Vec<run::Model>, DbErr> {
    let lease_cutoff = now() - chrono::Duration::seconds(DRIVER_LEASE_TTL_SECS);
    let mut query = run::Entity::find()
        .filter(run::Column::ParentRunId.is_null())
        .filter(run::Column::TaskStatus.is_in([
            "running",
            "awaiting_input",
            "delegating",
            "needs_resume",
            "shutdown",
        ]))
        .filter(
            Condition::any()
                .add(run::Column::DriverId.is_null())
                .add(run::Column::DriverHeartbeatAt.is_null())
                .add(run::Column::DriverHeartbeatAt.lt(lease_cutoff)),
        );
    if let Some(ws) = workspace_id {
        query = query.filter(run::Column::WorkspaceId.eq(ws));
    }
    query.all(db).await
}

// ── Stuck-automation-run sweeper ─────────────────────────────────────────────

/// An automation run that has no active queue entry driving it forward.
#[derive(Debug, Clone)]
pub struct StuckRun {
    pub run_id: String,
    pub task_status: Option<String>,
    /// Owning workspace — used by recovery loops to look up the right
    /// cached `PlatformContext` before driving the run. Nil UUID for
    /// local serve mode (== `LOCAL_WORKSPACE_ID`).
    pub workspace_id: Uuid,
    /// What kind of work this run is (`compile`, `airway`, `workflow`, …).
    ///
    /// Carried so a driver can decline a run it is structurally unable to
    /// execute BEFORE acquiring the lease. Declining after the claim does not
    /// work: only the lease-holder can claim the row, so handing it back
    /// re-selects it in the same process while the live heartbeat excludes
    /// every other node.
    pub source_type: Option<String>,
}

/// Find automation runs that are stranded: `task_status` is non-terminal but no
/// queue entry for the run or any descendant is in `queued`/`claimed`. These
/// runs cannot make progress on their own — nothing will re-drive them.
///
/// `grace_secs` is a lower bound on `updated_at` age to avoid racing with a
/// worker that is mid-commit (e.g. has already advanced state but has not yet
/// enqueued the follow-up).
///
/// Intentionally scoped to `source_type = 'workflow'`. Agent/analytics runs
/// that get into this state are typically unrecoverable (no idempotent
/// re-drive primitive), and a blanket sweep could false-positive on
/// long-running LLM calls. Automation decisions are pure + `decision_version`
/// gated, so a spurious re-enqueue is always safe.
pub async fn find_stuck_automation_runs(
    db: &DatabaseConnection,
    grace_secs: u64,
) -> Result<Vec<StuckRun>, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        task_status: Option<String>,
        workspace_id: Uuid,
        source_type: Option<String>,
    }

    // Active statuses from `get_active_root_runs` / `cleanup_stale_runs` — a
    // run in any of these is presumed "still supposed to be making progress".
    // We intentionally exclude `awaiting_input` (HITL suspension — driven by
    // a user action, not a queue row).
    let sql = "\
        SELECT r.id, r.task_status, r.workspace_id, r.source_type \
        FROM agentic_runs r \
        WHERE r.source_type = 'workflow' \
          AND r.task_status IN ('running', 'delegating', 'waiting_on_child', 'waiting_on_children') \
          AND r.updated_at < now() - ($1 || ' seconds')::interval \
          AND NOT EXISTS ( \
              SELECT 1 FROM agentic_task_queue q \
              WHERE (q.task_id = r.id OR q.task_id LIKE r.id || '.%') \
                AND q.queue_status IN ('queued', 'claimed') \
          )";

    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [(grace_secs as i64).into()],
    ))
    .all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StuckRun {
            run_id: r.id,
            task_status: r.task_status,
            workspace_id: r.workspace_id,
            source_type: r.source_type,
        })
        .collect())
}

/// Find runs that are stranded **and** safe for the periodic global
/// driver loop to pick up — generalized over `find_stuck_automation_runs`
/// for `workflow` + `airway` (the Phase 1/2 schedulable targets).
///
/// "Stranded" means: no queue entry is `claimed` (a live worker — the
/// dead case is the reaper's job) **and** no entry is `queued` *and*
/// `scope_owned = true`. The `scope_owned` split is load-bearing:
///
/// - `claimed` (any scope) → a live coordinator owns it → exclude (this is
///   the rung-2 anti-poaching invariant; a live per-request coordinator
///   always has a `claimed`, heart-beating entry, and a live coordinator's
///   transient not-yet-claimed children are `queued scope_owned = true`).
/// - `queued` + `scope_owned = true` → an interactive run's not-yet-claimed
///   task; its live coordinator is about to claim it (the grace window
///   covers the enqueue→start gap) → exclude.
/// - `queued` + `scope_owned = false` → a Global orphan / scheduler-seeded
///   task with **no consumer** in Phase 1 (no standalone unscoped claim
///   worker). This does NOT shield the run — it is exactly the rung-4 /
///   Phase-2 case the periodic driver must pick up and drive.
///
/// Also excludes runs whose driver lease is still fresh, so two ticks /
/// replicas don't both grab the same stranded run.
///
/// `get_resumable_root_runs` is still correct for *startup* recovery: a
/// process restart kills every in-flight coordinator, so everything
/// resumable is genuinely orphaned.
///
/// `workspace_id` — when `Some`, only return runs owned by that workspace.
/// `None` returns every workspace's stranded runs. The recovery loop in
/// cloud mode passes the per-iteration workspace_id so it doesn't try to
/// drive workspace-B's run with workspace-A's `PlatformContext`; the
/// startup pass + tests pass `None`.
pub async fn find_stuck_runs(
    db: &DatabaseConnection,
    grace_secs: u64,
    workspace_id: Option<Uuid>,
) -> Result<Vec<StuckRun>, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement, Value};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        task_status: Option<String>,
        workspace_id: Uuid,
        source_type: Option<String>,
    }

    // The workspace filter is conditional, but every binding must be the
    // same number of placeholders across paths — branch on whether
    // `workspace_id` is set and append the extra clause + value.
    let mut values: Vec<Value> = vec![
        (grace_secs as i32).into(),
        (DRIVER_LEASE_TTL_SECS as i32).into(),
    ];
    let workspace_clause = if let Some(ws) = workspace_id {
        values.push(ws.into());
        " AND r.workspace_id = $3 "
    } else {
        ""
    };
    let sql = format!(
        "\
        SELECT r.id, r.task_status, r.workspace_id, r.source_type \
        FROM agentic_runs r \
        WHERE r.source_type IN ('workflow', 'airway') \
          AND r.parent_run_id IS NULL \
          AND r.task_status IN ('running', 'delegating', 'waiting_on_child', 'waiting_on_children', 'needs_resume', 'shutdown') \
          AND r.updated_at < now() - make_interval(secs => $1) \
          AND (r.driver_id IS NULL \
               OR r.driver_heartbeat_at IS NULL \
               OR r.driver_heartbeat_at < now() - make_interval(secs => $2)) \
          {workspace_clause} \
          AND NOT EXISTS ( \
              SELECT 1 FROM agentic_task_queue q \
              WHERE (q.task_id = r.id OR q.task_id LIKE r.id || '.%') \
                AND ( \
                  q.queue_status = 'claimed' \
                  OR (q.queue_status = 'queued' AND q.scope_owned = true) \
                ) \
          )"
    );

    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StuckRun {
            run_id: r.id,
            task_status: r.task_status,
            workspace_id: r.workspace_id,
            source_type: r.source_type,
        })
        .collect())
}

/// Find runs that have a `queued` + `scope_owned = false` queue entry
/// AND are not currently lease-held — the §12 FU4c latency-worker
/// selection. No grace window: a queued row only exists after the seed
/// function fully commits, so there's no mid-commit race to wait out
/// (unlike `find_stuck_runs` which guards against an in-flight worker
/// that's about to enqueue).
///
/// This is intentionally narrower than `find_stuck_runs`: it picks up
/// freshly-seeded Global runs at claim-time (cron / `run-now`) so the
/// periodic loop's grace window doesn't gate them.
///
/// **Source-type policy:** this query is type-agnostic on purpose. The
/// "freshly seeded, never claimed" precondition (`queue_status='queued'`
/// + `scope_owned=false`) means no worker has yet executed the spec, so
/// the LLM-double-spend concern that justifies `find_stuck_runs`'s
/// `('workflow', 'airway')` filter does not apply here. Any new top-level
/// source type (analytics agents, future kinds) must be picked up by
/// this latency worker — otherwise scheduled / run-now runs sit
/// `queued` forever. Tests in `latency_worker_picks_up_all_source_types`
/// enforce this contract.
///
/// `workspace_id` — when `Some`, only return pending rows owned by that
/// workspace. When `None`, returns every workspace's pending rows; the
/// caller (e.g. the cloud-mode latency worker) is responsible for
/// grouping by `StuckRun.workspace_id` and routing each row to the
/// correct cached `PlatformContext`.
pub async fn find_pending_global_runs(
    db: &DatabaseConnection,
    workspace_id: Option<Uuid>,
) -> Result<Vec<StuckRun>, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement, Value};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        task_status: Option<String>,
        workspace_id: Uuid,
        source_type: Option<String>,
    }

    let mut values: Vec<Value> = vec![(DRIVER_LEASE_TTL_SECS as i32).into()];
    let workspace_clause = if let Some(ws) = workspace_id {
        values.push(ws.into());
        " AND r.workspace_id = $2 "
    } else {
        ""
    };
    let statuses = pending_global_status_sql();
    let sql = format!(
        "\
        SELECT r.id, r.task_status, r.workspace_id, r.source_type \
        FROM agentic_runs r \
        WHERE r.parent_run_id IS NULL \
          AND r.task_status IN ({statuses}) \
          AND (r.driver_id IS NULL \
               OR r.driver_heartbeat_at IS NULL \
               OR r.driver_heartbeat_at < now() - make_interval(secs => $1)) \
          {workspace_clause} \
          AND EXISTS ( \
              SELECT 1 FROM agentic_task_queue q \
              WHERE (q.task_id = r.id OR q.task_id LIKE r.id || '.%') \
                AND q.queue_status = 'queued' \
                AND q.scope_owned = false \
          )"
    );

    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| StuckRun {
            run_id: r.id,
            task_status: r.task_status,
            workspace_id: r.workspace_id,
            source_type: r.source_type,
        })
        .collect())
}

/// Mark a run as failed during recovery (when resume itself fails).
pub async fn mark_recovery_failed(
    db: &DatabaseConnection,
    run_id: &str,
    error: &str,
) -> Result<(), DbErr> {
    transition_run(
        db,
        run_id,
        "failed",
        None,
        None,
        Some(&format!("recovery failed: {error}")),
    )
    .await
}

/// Get the max child counter across all runs in a task tree.
///
/// Scans all `agentic_runs` whose ID starts with `root_run_id` and extracts
/// the numeric counter suffix to determine the next safe counter value.
/// This queries the DB directly rather than relying on the in-memory task tree,
/// ensuring we account for children that may have been created by previous
/// recovery attempts even if they're not in the loaded tree.
pub async fn get_max_child_counter(
    db: &DatabaseConnection,
    root_run_id: &str,
) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    // Query all run IDs that are descendants of this root.
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id FROM agentic_runs WHERE id LIKE $1 AND id != $2",
        [format!("{root_run_id}.%").into(), root_run_id.into()],
    );
    let rows = db.query_all_raw(stmt).await?;

    let mut max_counter: u64 = 0;
    for row in rows {
        let id: String = row.try_get("", "id")?;
        // Check every segment, not just the last, to catch nested IDs.
        for segment in id.split('.') {
            if let Some(gen_str) = segment.strip_prefix('a') {
                if let Some((_, c)) = gen_str.split_once('_')
                    && let Ok(c) = c.parse::<u64>()
                {
                    max_counter = max_counter.max(c);
                }
            } else if let Some(gen_str) = segment.strip_prefix('g') {
                if let Some((_, c)) = gen_str.split_once('_')
                    && let Ok(c) = c.parse::<u64>()
                {
                    max_counter = max_counter.max(c);
                }
            } else if let Ok(n) = segment.parse::<u64>() {
                max_counter = max_counter.max(n);
            }
        }
    }

    Ok(max_counter)
}

/// Increment the attempt counter for a run and return the new value.
pub async fn increment_attempt(db: &DatabaseConnection, run_id: &str) -> Result<i32, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct Row {
        attempt: i32,
    }

    // Single atomic statement rather than SELECT-then-UPDATE. The caller holds
    // the driver lease, so a concurrent bump is not reachable today — but a lost
    // update here would silently *un-bound* the recovery budget this feeds, and
    // that is not a property worth resting on a second mechanism being correct.
    // Same round-trip count either way.
    Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET attempt = attempt + 1, updated_at = now() \
         WHERE id = $1 RETURNING attempt",
        [run_id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.attempt)
    .ok_or_else(|| DbErr::RecordNotFound(run_id.to_string()))
}

/// Retire a run and its live queue rows **in one transaction**.
///
/// Both writes or neither. The order matters (queue rows first: [`claim_task`]
/// has no run-status predicate, so a terminal run with `queued` rows stays
/// claimable), but ordering alone is not enough — cancelling the queue rows
/// removes the run from [`find_pending_global_runs`], whose selection requires a
/// `queued`, non-scope-owned row. So if the terminal write then failed, nothing
/// would ever select that run again and it would sit non-terminal forever.
/// Rolling both back leaves the run exactly as it was, and the next tick
/// genuinely retries the whole step.
///
/// [`claim_task`]: super::queue::claim_task
pub async fn retire_run(db: &DatabaseConnection, run_id: &str, reason: &str) -> Result<(), DbErr> {
    retire_run_with_message(db, run_id, &format!("recovery failed: {reason}")).await
}

/// Retire the run owning a task that was **dead-lettered** by the queue.
///
/// [`super::queue::defer_task`] moves a row to `dead` once it has waited past
/// its domain's ceiling, and until this existed that was the end of it: the
/// queue row went terminal, an ERROR line was logged, and the RUN stayed
/// `running` for ever. Nothing alerts on a run that reads `running`, and
/// `find_pending_global_runs` requires a `queued` row, so nothing selected it
/// again either — a customer's daily load could stop permanently while every
/// surface reported it as in flight. That is the shape the ceiling exists to
/// prevent, and the ceiling could not do it alone.
///
/// The message is the operator's only account of what happened, so it carries
/// the domain's own deferral reason rather than a generic "timed out".
pub async fn dead_letter_run(
    db: &DatabaseConnection,
    run_id: &str,
    reason: &str,
) -> Result<(), DbErr> {
    retire_run_with_message(
        db,
        run_id,
        &format!("dead-lettered after waiting past its queue ceiling: {reason}"),
    )
    .await
}

/// The **root** run id that owns `task_id` — itself for a root task, the
/// prefix for a `run_id.N` descendant.
///
/// The predicate mirrors [`find_pending_global_runs`] and
/// [`super::queue::cancel_queued_tasks_for_run`] rather than splitting the id
/// on `.` here: a third spelling of "which run owns this task" is a third
/// thing to keep in step. `None` means the run row is gone, which is not an
/// error — the caller has nothing left to retire.
///
/// **`ORDER BY length(id) ASC` is the whole correctness of this function.**
/// A delegated child's RUN id is its TASK id (`coordinator::suspension` sets
/// `child_run_id = child_id = format!("{task_id}.{n}")`), so for `root.1` both
/// `root.1` and `root` satisfy the predicate and the ordering alone decides
/// which comes back. `DESC` returns the child — which retires the child run
/// and leaves the parent in `delegating`/`waiting_on_child` with no live queue
/// row and nothing to wake it (`find_stuck_runs` only sweeps
/// `source_type = 'workflow'`). That is the same "reads as in flight for ever"
/// one level up, which is what the caller exists to stop.
///
/// Shortest match is the root because a root id never contains `.` — every
/// dot in a task id was put there by the delegation counter.
pub async fn owning_run_id(
    db: &DatabaseConnection,
    task_id: &str,
) -> Result<Option<String>, DbErr> {
    use sea_orm::FromQueryResult;

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
    }

    // `$1 LIKE id || '.%'` is not sargable, so this is a scan of
    // `agentic_runs`. Acceptable here and nowhere hotter: it runs once per
    // dead-letter, which is a task that has already spent its entire wait
    // budget failing to run. Run ids are UUIDs (or `<prefix>-<uuid>`) and
    // carry no `_` or `%`, so the LIKE metacharacters are not reachable from
    // an id — if that ever stops being true this needs an ESCAPE clause.
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id FROM agentic_runs \
          WHERE id = $1 OR $1 LIKE id || '.%' \
          ORDER BY length(id) ASC LIMIT 1",
        [task_id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.id))
}

/// The shared body of [`retire_run`] and [`dead_letter_run`]: both writes in
/// one transaction, with the caller supplying the whole `error_message`.
async fn retire_run_with_message(
    db: &DatabaseConnection,
    run_id: &str,
    error_message: &str,
) -> Result<(), DbErr> {
    use sea_orm::TransactionTrait;

    let txn = db.begin().await?;
    super::queue::cancel_queued_tasks_for_run(&txn, run_id).await?;
    // Mirrors `transition_run`'s terminal path, including releasing the driver
    // lease — a terminal run needs no driver.
    //
    // **The whole task tree, not just the root**, and on the same predicate
    // the queue cancel above already uses. The two halves were asymmetric: the
    // queue side has always cancelled every row under `run_id`, while this one
    // touched a single `agentic_runs` row. That left a retired root's children
    // non-terminal — `running`, with a `dead` or `cancelled` queue row and
    // nothing in the steady state that could advance them.
    //
    // Not permanent, and the earlier version of this comment overstated it:
    // `cleanup_stale_runs`' SECOND pass selects children with a non-null
    // `parent_run_id` in the non-terminal set and fails them once the parent
    // is terminal, so a stranded child does converge to `failed`. But that
    // runs at **`oxy serve` startup only** (`router::entry`'s
    // `new_agentic_state`; `WorkerRuntime` deliberately skips it), so the
    // reconciliation is a restart, not a sweep on any cadence — the child is
    // non-terminal for however long this deployment stays up.
    //
    // Closing it here removes the dependency on that restart entirely: the row
    // goes terminal in the same transaction that retires its root. Leaving a
    // run row stuck non-terminal is the exact defect this function exists to
    // fix, so leaving one behind in the fix is not a trade worth making.
    //
    // `NOT IN (<terminal>)` so a child that already finished keeps its own
    // outcome: a delegation can complete and report before a later sibling
    // strands the root, and overwriting that `done` with `failed` would
    // rewrite history rather than close it. The terminal set is
    // `lifecycle::crud`'s (`done | failed | cancelled | timed_out`). The
    // `IS NULL` disjunct is load-bearing, not defensive: `x NOT IN (…)` is
    // NULL — not true — for a NULL column, so without it a run with no status
    // yet would never be retired.
    //
    // Same sargability cost as `owning_run_id` above, and bounded for a
    // different reason: `id LIKE $1 || '.%'` cannot use the PK btree, so this
    // scans `agentic_runs` where it used to be a PK lookup. What bounds it is
    // that the queue cancel one line up is what de-selects the run from
    // `find_pending_global_runs` (which requires a `queued` row), so a caller
    // like `retire_orphaned_runs` fires once per orphan rather than once per
    // latency tick. `cancel_queued_tasks_for_run` already pays the identical
    // cost on the larger `agentic_task_queue`.
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs \
            SET task_status = 'failed', error_message = $2, \
                driver_id = NULL, driver_heartbeat_at = NULL, updated_at = now() \
          WHERE (id = $1 OR id LIKE $1 || '.%') \
            AND (task_status IS NULL \
                 OR task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out'))",
        [run_id.into(), error_message.into()],
    ))
    .await?;
    txn.commit().await
}
