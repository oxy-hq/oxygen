//! Resume enumeration for the `agentic_runs` table.

use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;

use crate::lifecycle::crud::{DRIVER_LEASE_TTL_SECS, transition_run};
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

/// The `agentic_task_queue` rows (aliased `alias`) in **root** run `r`'s
/// tree: its own root task and every `<run_id>.<n>…` descendant.
///
/// `root_task_id` is indexed and set on every row by the queue's insert
/// trigger (`m20261105_000001_add_task_queue_root_task_id`), so this is a
/// plain equi-join the planner hashes, merges or probes. `r` must be a root
/// (`r.parent_run_id IS NULL`): a child run's id is no row's root. For a run
/// that may be a child, see [`queue_rows_of_subtree`].
pub(super) fn queue_rows_of_root(alias: &str) -> String {
    format!("{alias}.root_task_id = r.id")
}

/// The `agentic_task_queue` rows (aliased `alias`) in run `r`'s **subtree**,
/// for an `r` that may be a child: its own task and every `<r.id>.<n>…`
/// descendant. Only the boot cleanup asks this of child runs.
///
/// The leading `split_part` equality is the planner's join key, not a filter:
/// it is implied by the match after it, so it changes no result. Without it
/// the match is an `OR` with a `LIKE` whose pattern is an outer-row
/// expression, which no index or hash can take. (The key is also stricter
/// than the `LIKE` in one way, deliberately: a `_` in a run id is a `LIKE`
/// wildcard and could match a different run's `<id>.<n>` row; the key
/// cannot.)
pub(super) fn queue_rows_of_subtree(alias: &str) -> String {
    format!(
        "(split_part({alias}.task_id, '.', 1) = split_part(r.id, '.', 1) \
          AND ({alias}.task_id = r.id OR {alias}.task_id LIKE r.id || '.%'))"
    )
}

/// [`StuckRun::unclaimed_secs`] as a SELECT-list expression over run `r`.
/// `ttl` is the placeholder bound to [`DRIVER_LEASE_TTL_SECS`].
///
/// `queued` is the alias of the run's joined `queued` Global queue rows, for a
/// query that joins them and `GROUP BY r.id`.
///
/// One definition on purpose: every selection that reports the number hands it
/// to the same placement gate (or could), and a clock that ticked differently
/// on two paths would make the gate's answer depend on which loop happened to
/// select the run.
///
/// `GREATEST` skips NULLs, so a run with no lease on record — or, under a LEFT
/// JOIN, no queued row — simply drops that term. The lease term is gated on
/// `driver_id` because a cleanly released lease clears both columns while a
/// dead driver leaves both set. Clamped at 0 so the cast to `u64` cannot wrap.
fn unclaimed_secs_sql(queued: &str, ttl: &str) -> String {
    format!(
        "GREATEST(0, FLOOR(EXTRACT(EPOCH FROM now() - GREATEST( \
             r.updated_at, \
             MAX({queued}.updated_at), \
             CASE WHEN r.driver_id IS NOT NULL \
                  THEN r.driver_heartbeat_at + make_interval(secs => {ttl}) END \
         ))))::bigint AS unclaimed_secs"
    )
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

/// Find root runs that are resumable after a server restart.
///
/// Includes tasks marked `"shutdown"` (graceful shutdown — always resumable)
/// and `"needs_resume"` (crash recovery — best effort).
///
/// Excludes runs a *live* driver already owns: a run is only resumable if it
/// is unleased or its driver lease has gone stale past
/// [`DRIVER_LEASE_TTL_SECS`] (on the database clock). Every driver takes that
/// lease — recovery in `recover_single_run`, a direct drive through
/// [`crate::lifecycle::driver_lease::hold_direct_drive_lease`] — so this is the
/// whole liveness test: a run a peer process is driving right now is never
/// selected, whichever process asks. The flip side is that a driver which died
/// moments ago is still "live" for up to the TTL, which is why the startup
/// pass (`oxy-app`'s `StartupPass::Run`) runs once more after the TTL.
///
/// `workspace_id` — when `Some`, only return runs owned by that workspace.
/// Cloud-mode startup recovery iterates per workspace and passes the
/// current workspace id; local mode passes `None` (the single workspace
/// is identified by the nil UUID and every other row would also be nil).
pub async fn get_resumable_root_runs(
    db: &DatabaseConnection,
    workspace_id: Option<Uuid>,
) -> Result<Vec<run::Model>, DbErr> {
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
                .add(Expr::cust(format!(
                    "driver_heartbeat_at < now() - make_interval(secs => {DRIVER_LEASE_TTL_SECS})"
                ))),
        );
    if let Some(ws) = workspace_id {
        query = query.filter(run::Column::WorkspaceId.eq(ws));
    }
    query.all(db).await
}

// ── Stranded-run selection ───────────────────────────────────────────────────

/// A run that has no active queue entry driving it forward.
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
    /// Whole seconds this run has been selectable with nobody taking it.
    ///
    /// Every selection that returns a `StuckRun` measures it with one SQL
    /// definition (`unclaimed_secs_sql`): the two a placement gate sits behind
    /// — [`find_pending_global_runs`] (the latency worker) and
    /// [`find_stuck_runs`] (the periodic stranded tick).
    ///
    /// The clock restarts at the LATEST of three moments, so that any sign of
    /// life postpones it rather than any sign of age advancing it:
    ///
    /// - the run row's `updated_at` (a status change — submit, `shutdown`,
    ///   `needs_resume`);
    /// - the newest `updated_at` among its `queued` Global queue rows (a
    ///   release, a reaper re-queue, a deferral — each is a node handing the
    ///   row back *now*, whatever the run's own age);
    /// - the instant a dead driver's lease lapsed (`driver_heartbeat_at` +
    ///   [`DRIVER_LEASE_TTL_SECS`]), since the run was not selectable before.
    ///
    /// A driver that prefers to leave a kind for another node reads this to
    /// tell "just arrived, the fleet will take it" from "nobody has taken this"
    /// — see `agentic_pipeline::recovery::may_drive`. Using the run's age alone
    /// would call an hour-old pipeline whose worker just died "unclaimed for an
    /// hour" and hand it to the deferring node in the same tick the fleet first
    /// sees it.
    pub unclaimed_secs: u64,
}

/// Find runs that are stranded **and** safe for the periodic global
/// driver loop to pick up — `workflow` + `airway` (the Phase 1/2
/// schedulable targets).
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
///
/// **`unclaimed_secs`:** every row also reports how long it has been
/// selectable with nobody taking it (see [`StuckRun::unclaimed_secs`]). It is
/// a column, not a predicate: the selection is the one above, grace window
/// included. The `queued` Global rows it is measured over are LEFT JOINed
/// (a stranded run may have none — `needs_resume` after a crash), on the same
/// keyed match as the `NOT EXISTS`, so the join is a hash over the queued set
/// rather than a test of every (run, row) pair.
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
        unclaimed_secs: i64,
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
    let held_by_a_live_coordinator = queue_rows_of_root("q");
    let queued_global = queue_rows_of_root("g");
    let unclaimed_secs = unclaimed_secs_sql("g", "$2");
    // `GROUP BY r.id` alone is enough for the other `r.*` columns: `id` is the
    // table's primary key, so Postgres treats them as functionally dependent.
    let sql = format!(
        "\
        SELECT r.id, r.task_status, r.workspace_id, r.source_type, \
               {unclaimed_secs} \
        FROM agentic_runs r \
        LEFT JOIN agentic_task_queue g \
          ON {queued_global} \
         AND g.queue_status = 'queued' \
         AND g.scope_owned = false \
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
              WHERE {held_by_a_live_coordinator} \
                AND ( \
                  q.queue_status = 'claimed' \
                  OR (q.queue_status = 'queued' AND q.scope_owned = true) \
                ) \
          ) \
        GROUP BY r.id"
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
            // Clamped to 0 in SQL, so the cast cannot wrap.
            unclaimed_secs: r.unclaimed_secs as u64,
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
///
/// **`unclaimed_secs`:** every row also reports how long it has been
/// selectable with nobody taking it (see [`StuckRun::unclaimed_secs`] for the
/// clock). It is a column, not a predicate — the selection is unchanged and
/// still has no grace window.
///
/// **Shape:** an inner join to the run's `queued` Global rows plus `GROUP BY`,
/// not `EXISTS` plus a scalar subquery. The two are the same set (a run is
/// returned iff it has at least one such row, and `MAX` ranges over exactly
/// those rows), but the scalar form re-scanned the queued set once per
/// returned row — 100k buffers where the query had read 280. The join reads
/// it once, and `queue_rows_of_root`'s key lets the planner merge or hash the
/// two sides rather than test every pair. This runs every second on every
/// node; the measurements are in `internal-docs/worker-fleet.md`.
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
        unclaimed_secs: i64,
    }

    let mut values: Vec<Value> = vec![(DRIVER_LEASE_TTL_SECS as i32).into()];
    let workspace_clause = if let Some(ws) = workspace_id {
        values.push(ws.into());
        " AND r.workspace_id = $2 "
    } else {
        ""
    };
    let statuses = pending_global_status_sql();
    let queued_global = queue_rows_of_root("q");
    let unclaimed_secs = unclaimed_secs_sql("q", "$1");
    // `GROUP BY r.id` alone is enough for the other `r.*` columns: `id` is the
    // table's primary key, so Postgres treats them as functionally dependent.
    let sql = format!(
        "\
        SELECT r.id, r.task_status, r.workspace_id, r.source_type, \
               {unclaimed_secs} \
        FROM agentic_runs r \
        JOIN agentic_task_queue q \
          ON {queued_global} \
         AND q.queue_status = 'queued' \
         AND q.scope_owned = false \
        WHERE r.parent_run_id IS NULL \
          AND r.task_status IN ({statuses}) \
          AND (r.driver_id IS NULL \
               OR r.driver_heartbeat_at IS NULL \
               OR r.driver_heartbeat_at < now() - make_interval(secs => $1)) \
          {workspace_clause} \
        GROUP BY r.id"
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
            // Clamped to 0 in SQL, so the cast cannot wrap.
            unclaimed_secs: r.unclaimed_secs as u64,
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
/// Both writes or neither. The order matters (queue rows first:
/// [`claim_task_under_root`] has no run-status predicate, so a terminal run
/// with `queued` rows stays claimable), but ordering alone is not enough — cancelling the queue rows
/// removes the run from [`find_pending_global_runs`], whose selection requires a
/// `queued`, non-scope-owned row. So if the terminal write then failed, nothing
/// would ever select that run again and it would sit non-terminal forever.
/// Rolling both back leaves the run exactly as it was, and the next tick
/// genuinely retries the whole step.
///
/// [`claim_task_under_root`]: super::queue::claim_task_under_root
pub async fn retire_run(db: &DatabaseConnection, run_id: &str, reason: &str) -> Result<(), DbErr> {
    retire_run_with_message(db, run_id, &format!("recovery failed: {reason}")).await
}

/// Finish a root run whose work completed but whose `Done` outcome was lost.
/// Returns whether the run was changed.
///
/// The window this closes: `DurableTransport` stamps the task's queue row
/// `completed` and only then hands the outcome to the in-process coordinator,
/// which is what writes `task_status = done`. A process that dies between the
/// two leaves a finished run non-terminal with nothing to resume, and recovery
/// used to retire it as `failed` — a visible boundary over a run whose work had
/// finished.
///
/// Domain-agnostic on purpose: deciding that the work *did* finish needs the
/// domain's own terminal events, so that is the caller's job (for airway,
/// `agentic_pipeline::airway_recovery`). This only writes the result, with the
/// same `metadata` the coordinator's `update_run_done` would have written.
///
/// **Refuses — returns `false` and writes nothing — while any task in the run's
/// tree is still `queued` or `claimed`.** Live work means the evidence the
/// caller read may describe an earlier attempt (a reset-in-place retry whose
/// event delete failed, say), and finishing the run would drop that work. The
/// check sits in the same statement as the write, so a row queued in between
/// cannot slip past it; and since nothing live is ever cancelled here, the run
/// row and its queue rows cannot end up disagreeing, which is the asymmetry
/// [`retire_run`] documents. Also `false` for a run that is already terminal.
pub async fn complete_run_with_lost_outcome(
    db: &DatabaseConnection,
    run_id: &str,
    metadata: Option<serde_json::Value>,
) -> Result<bool, DbErr> {
    // `COALESCE($2, task_metadata)`: `None` keeps what is there, matching
    // `transition_run`'s "None means leave unchanged". `answer = ''` matches
    // what a root `Done` with an empty answer writes (airway's always is). The
    // tree predicate is `cancel_queued_tasks_for_run`'s.
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_runs \
                SET task_status = 'done', answer = COALESCE(answer, ''), \
                    task_metadata = COALESCE($2, task_metadata), error_message = NULL, \
                    driver_id = NULL, driver_heartbeat_at = NULL, updated_at = now() \
              WHERE id = $1 \
                AND (task_status IS NULL \
                     OR task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM agentic_task_queue q \
                     WHERE q.root_task_id = $1 \
                       AND q.queue_status IN ('queued', 'claimed'))",
            [run_id.into(), metadata.into()],
        ))
        .await?;
    let changed = res.rows_affected() > 0;
    if changed {
        // Row-only terminal transition: wake the run's streams to it.
        // Best-effort: the row is committed, and the caller's lease release
        // must not hinge on a second acquire.
        crate::lifecycle::crud::notify_run_events_best_effort(db, run_id).await;
    }
    Ok(changed)
}

/// The **root** run id that owns `task_id` — itself for a root task, the
/// prefix for a `run_id.N` descendant.
///
/// Reads `agentic_runs`, which has no root column, so it matches by id shape
/// rather than by the queue's `root_task_id` (which the dead-lettered row
/// could also answer). `None` means the run row is gone, which is not an
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
    // budget failing to run. Some run ids do carry `_` (the custom-app task
    // kinds, `custom_app_sandbox_oltp:…`), a LIKE wildcard; a wrong match needs
    // another run whose id differs only where this one has a `_`, which no
    // producer mints (`tree_scope_test` pins the queue side of this).
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

/// The body of [`retire_run`]: both writes in one transaction, with the
/// caller supplying the whole `error_message`.
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
    // Row-only terminal transition for the whole tree: wake its streams.
    crate::lifecycle::crud::notify_run_events(&txn, run_id).await?;
    txn.commit().await?;
    // Retired or dead-lettered is terminal. The observer reads the row, so a
    // root that had already finished (and kept its own outcome) is told apart.
    crate::lifecycle::failure_hook::root_run_failed(run_id).await;
    Ok(())
}
