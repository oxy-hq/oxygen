//! Durable task queue backing the coordinator/worker pipeline.

use std::sync::atomic::{AtomicU64, Ordering};

use sea_orm::{
    ActiveValue::*, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    Statement,
};

use crate::lifecycle::crud::now;
use crate::orchestrator::entity::task_queue;

/// Ownership regime for a queued task — decides whether the global/recovery
/// claim path may pick it up.
///
/// Stamped onto `agentic_task_queue.scope_owned` at INSERT and preserved
/// across `claimed -> queued` reaping. The global `claim_task` filters
/// `scope_owned = false`, so a [`TaskScope::Scoped`] task can never be
/// poached out from under the co-located coordinator that owns its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskScope {
    /// A co-located scoped coordinator owns this task's tree (every
    /// interactive HTTP/CLI run today). The global claim path must skip it.
    Scoped,
    /// No in-process driver owns this task — scheduler-seeded or
    /// crash-orphaned. The global/recovery loop is responsible for it.
    Global,
}

impl TaskScope {
    /// `true` for [`TaskScope::Scoped`] — the value written to
    /// `agentic_task_queue.scope_owned`.
    pub fn is_owned(self) -> bool {
        matches!(self, TaskScope::Scoped)
    }
}

/// Insert a new task into the durable queue with status `queued`.
pub async fn enqueue_task(
    db: &impl ConnectionTrait,
    task_id: &str,
    run_id: &str,
    parent_task_id: Option<&str>,
    spec: &agentic_core::delegation::TaskSpec,
    policy: Option<&agentic_core::delegation::TaskPolicy>,
    scope: TaskScope,
) -> Result<(), DbErr> {
    let now = now();
    let model = task_queue::ActiveModel {
        task_id: Set(task_id.to_string()),
        run_id: Set(run_id.to_string()),
        parent_task_id: Set(parent_task_id.map(String::from)),
        queue_status: Set("queued".to_string()),
        spec: Set(serde_json::to_value(spec).unwrap()),
        policy: Set(policy.map(|p| serde_json::to_value(p).unwrap())),
        worker_id: Set(None),
        last_heartbeat: Set(None),
        claimed_at: Set(None),
        visibility_timeout_secs: Set(60),
        claim_count: Set(0),
        max_claims: Set(3),
        scope_owned: Set(scope.is_owned()),
        // Explicitly `now()`, not `NotSet`. An enqueue MEANS "claimable now",
        // and saying so is what makes the upsert below correct: this row is
        // also the conflict path for re-enqueueing an existing `task_id`
        // (retry, reset). Leaving the column unwritten there would inherit a
        // prior `defer_task` deadline, so an explicit re-enqueue would sit
        // invisible for the remainder of a deferral its caller never chose.
        //
        // **This timestamp comes from the APP's clock, and the predicate that
        // reads it (`available_at <= now()` in `claim_task` /
        // `claim_task_under_root`) evaluates on the DATABASE's.** They are
        // different machines in any real deployment. If the database runs
        // behind the app by more than the enqueue→claim gap — milliseconds — a
        // freshly enqueued row is briefly invisible to a claim.
        //
        // Deliberately left as-is — but cost it honestly, because this comment
        // is what someone will re-decide from. The skew *window* is
        // sub-second; the *latency* is up to one poll interval, 10s by
        // default. A missed claim returns `Ok(None)` from `recv_assignment`,
        // which parks on `new_task_notify` / `router.wait_for_task` — and this
        // enqueue's NOTIFY was already consumed by the poll that missed it, so
        // absent unrelated traffic the next wake is `DEFAULT_POLL_INTERVAL`.
        //
        // Still latency rather than lost work, which is what keeps it out of
        // scope: a server-side `now()` would mean hand-writing this upsert
        // (SeaORM's `ActiveModel` cannot express it) and re-deriving the
        // conflict semantics above. It is written down because it is invisible
        // in the code and reads as a logic bug when it bites — the claim just
        // returns `None`, and in tests that surfaces as an unrelated assertion
        // failing several steps later.
        //
        // `created_at` below is stamped from the same clock and is
        // `claim_task`'s `ORDER BY` key, so multi-instance skew also perturbs
        // claim *order*. Fairness rather than correctness, and equally
        // invisible.
        available_at: Set(now),
        // Fresh work is not waiting on anything — clear any prior streak, on
        // the upsert path too (same reasoning as `available_at`).
        first_deferred_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    task_queue::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(task_queue::Column::TaskId)
                .update_columns([
                    task_queue::Column::QueueStatus,
                    task_queue::Column::Spec,
                    task_queue::Column::Policy,
                    task_queue::Column::WorkerId,
                    task_queue::Column::LastHeartbeat,
                    task_queue::Column::ClaimedAt,
                    task_queue::Column::ClaimCount,
                    // Clears any prior deferral — see `available_at` above.
                    task_queue::Column::AvailableAt,
                    task_queue::Column::FirstDeferredAt,
                    task_queue::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Atomically claim the oldest queued **root** task that no co-located
/// scoped coordinator owns (`scope_owned = false`). Returns `None` if no such
/// task is available. Uses `FOR UPDATE SKIP LOCKED` to avoid contention
/// between concurrent workers.
///
/// This is the unscoped/global claim path (the standalone worker and the
/// recovery loop). The `scope_owned = false` predicate is what prevents it
/// from poaching an interactive run's task out from under the per-request
/// coordinator that owns its tree — those rows are stamped `scope_owned =
/// true` at enqueue (see [`TaskScope`]) and are claimed only via
/// [`claim_task_under_root`], which deliberately ignores `scope_owned`.
///
/// Only **root** tasks (`parent_task_id IS NULL`) are eligible. A descendant
/// claimed in isolation routes its outcome to whichever coordinator holds the
/// transport, whose in-memory task map does not contain it — `handle_done`
/// early-returns and the result is silently dropped, leaving the queue row
/// `completed`, the run `delegating`, and the parent waiting forever. Subtrees
/// are recovered by re-driving their root; see [`claim_task_under_root`].
pub async fn claim_task(
    db: &DatabaseConnection,
    worker_id: &str,
) -> Result<Option<task_queue::Model>, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    // Single atomic UPDATE ... RETURNING using a subquery with FOR UPDATE SKIP LOCKED.
    let sql = "\
        UPDATE agentic_task_queue \
        SET queue_status = 'claimed', \
            worker_id = $1, \
            claimed_at = now(), \
            last_heartbeat = now(), \
            claim_count = claim_count + 1, \
            updated_at = now() \
        WHERE task_id = ( \
            SELECT task_id FROM agentic_task_queue \
            WHERE queue_status = 'queued' \
              AND available_at <= now() \
              AND scope_owned = false \
              AND parent_task_id IS NULL \
            ORDER BY created_at \
            LIMIT 1 \
            FOR UPDATE SKIP LOCKED \
        ) \
        RETURNING *";

    let result = task_queue::Model::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [worker_id.into()],
    ))
    .one(db)
    .await?;

    Ok(result)
}

/// Like [`claim_task`] but only considers tasks under the given root —
/// either the root itself (`task_id = $root`) or its descendants
/// (`task_id LIKE '$root.%'`).
///
/// Used by per-run scoped workers so they don't accidentally claim tasks
/// from a sibling run's coordinator. See `DurableTransport::task_id_root`
/// for the full context — without this, workers spawned per HTTP request
/// race for queued tasks across runs and the wrong coordinator silently
/// drops the outcome.
pub async fn claim_task_under_root(
    db: &DatabaseConnection,
    worker_id: &str,
    root_task_id: &str,
) -> Result<Option<task_queue::Model>, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    let sql = "\
        UPDATE agentic_task_queue \
        SET queue_status = 'claimed', \
            worker_id = $1, \
            claimed_at = now(), \
            last_heartbeat = now(), \
            claim_count = claim_count + 1, \
            updated_at = now() \
        WHERE task_id = ( \
            SELECT task_id FROM agentic_task_queue \
            WHERE queue_status = 'queued' \
              AND available_at <= now() \
              AND (task_id = $2 OR task_id LIKE $2 || '.%') \
            ORDER BY created_at \
            LIMIT 1 \
            FOR UPDATE SKIP LOCKED \
        ) \
        RETURNING *";

    let result = task_queue::Model::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [worker_id.into(), root_task_id.into()],
    ))
    .one(db)
    .await?;

    Ok(result)
}

pub async fn get_queue_entry(
    db: &DatabaseConnection,
    task_id: &str,
) -> Result<Option<task_queue::Model>, DbErr> {
    task_queue::Entity::find_by_id(task_id).one(db).await
}

/// Refresh the heartbeat on a task **this worker still holds**.
///
/// The `worker_id = $2 AND queue_status = 'claimed'` predicate is
/// load-bearing, not defensive. The heartbeat ticker is cancelled only when
/// `handle_task` returns (see `orchestrator/worker.rs`), so a tick can land
/// *after* graceful shutdown has already handed this process's claims back to
/// the queue via [`release_claims_for_worker`]. An unconditional UPDATE by
/// primary key would then either:
///
/// - re-stamp `last_heartbeat` on a row that is now `queued`, recreating the
///   "queued row carrying a dead owner's heartbeat" state that the reaper's
///   requeue path deliberately clears; or, worse,
/// - stamp a heartbeat onto a **successor's** claim if a peer already
///   re-claimed the row — masking that peer's death from the reaper for as
///   long as this process lives.
///
/// Returns `true` if the row was stamped, `false` if this worker no longer
/// owns it. A `false` is not an error: it is the normal outcome of a
/// heartbeat racing shutdown, and callers should simply stop ticking.
pub async fn update_queue_heartbeat(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
) -> Result<bool, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET last_heartbeat = now(), updated_at = now() \
             WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed'",
            [task_id.into(), worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected() > 0)
}

/// What an ownership-scoped terminal write actually did.
///
/// A plain `bool` conflated four genuinely different situations, and the one
/// that matters operationally — "a peer owns this row, so this process's work
/// is orphaned" — is the *rarest* of them. Callers that logged every `false`
/// as a lost claim warned on two routine, entirely healthy paths (an ordinary
/// user cancel, and any externally-driven root task), which is how a warning
/// stops being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalWrite {
    /// The row was live and owned by this worker; it now carries the requested
    /// status. The overwhelmingly common case.
    Stamped,
    /// The row is **already terminal and still owned by this worker** — most
    /// often because the requester cancelled it (`cancel_queued_task` leaves
    /// `worker_id` in place) a moment before the worker reported its own
    /// outcome. The first terminal status wins and this write is a no-op.
    /// Nothing is wrong and nothing is lost.
    AlreadyTerminal,
    /// No queue row exists for this task at all. Normal for root tasks
    /// registered via `Coordinator::register_root`, which publishes no
    /// assignment and therefore no queue row — every interactive
    /// analytics/builder run reports its outcome this way.
    NoRow,
    /// A **different** worker holds the row. This is the hazard the ownership
    /// predicate exists for: this process's outcome is being discarded by the
    /// queue, and it is the only variant worth a warning.
    NotOwned,
}

/// Stamp a terminal `queue_status` on a task **this worker still holds**.
///
/// The `worker_id = $2` predicate is the same ownership guard
/// [`update_queue_heartbeat`] carries, and it exists for the same reason —
/// only sharper here, because a terminal status is not self-correcting the way
/// a stale heartbeat is.
///
/// Graceful shutdown fires cancel tokens fire-and-forget and then immediately
/// hands this process's claims back via [`release_claims_for_worker`], whose
/// `claimed -> queued` transition NOTIFYs surviving peers. A peer can therefore
/// re-claim the row within milliseconds — that immediacy is the whole point of
/// the design — while this process's task is still winding down (an in-flight
/// LLM call can take seconds to observe the cancel). When it finally reports
/// its outcome, an UPDATE by primary key would stamp `completed`/`failed`/
/// `cancelled` onto **the peer's live claim**. The peer then executes a task
/// whose queue row is terminal, and if the peer dies the reaper skips it
/// (terminal rows are not reaped), so the task is never requeued and its parent
/// coordinator waits forever.
///
/// The `queue_status IN ('queued', 'claimed')` guard makes the **first**
/// terminal status win: a user cancel followed by the worker's own `Failed`
/// leaves the row `cancelled` rather than flipping to `failed` and then to
/// `completed` depending on arrival order. See [`TerminalWrite`] for how the
/// resulting no-op is distinguished from a genuinely lost claim — that
/// distinction is the whole reason this returns an enum and not a `bool`.
///
/// Generic over the connection so it can run inside a caller's transaction —
/// `agentic-automation`'s decision commit stamps the decision task's row in
/// the same transaction as the run-state patch, and must roll the whole thing
/// back on [`TerminalWrite::NotOwned`].
pub async fn set_terminal_status_owned<C: ConnectionTrait>(
    conn: &C,
    task_id: &str,
    worker_id: &str,
    status: &str,
) -> Result<TerminalWrite, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let res = conn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = $3, updated_at = now() \
             WHERE task_id = $1 AND worker_id = $2 \
               AND queue_status IN ('queued', 'claimed')",
            [task_id.into(), worker_id.into(), status.into()],
        ))
        .await?;
    if res.rows_affected() > 0 {
        return Ok(TerminalWrite::Stamped);
    }
    classify_terminal_miss(conn, task_id, worker_id).await
}

/// Work out *why* a terminal write matched nothing. Only ever runs on the miss
/// path, so the extra round-trip costs nothing in the common case.
async fn classify_terminal_miss<C: ConnectionTrait>(
    conn: &C,
    task_id: &str,
    worker_id: &str,
) -> Result<TerminalWrite, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT worker_id FROM agentic_task_queue WHERE task_id = $1",
            [task_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(TerminalWrite::NoRow);
    };
    let owner: Option<String> = row.try_get("", "worker_id")?;
    // We still own it, so the UPDATE can only have been blocked by the status
    // guard — the row is already terminal.
    if owner.as_deref() == Some(worker_id) {
        Ok(TerminalWrite::AlreadyTerminal)
    } else {
        Ok(TerminalWrite::NotOwned)
    }
}

/// Mark a task **this worker holds** as completed. See
/// [`set_terminal_status_owned`] for why the ownership predicate is
/// load-bearing and what each [`TerminalWrite`] means.
pub async fn complete_queue_task(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
) -> Result<TerminalWrite, DbErr> {
    set_terminal_status_owned(db, task_id, worker_id, "completed").await
}

/// Mark a task **this worker holds** as failed. See
/// [`set_terminal_status_owned`] for why the ownership predicate is
/// load-bearing and what each [`TerminalWrite`] means.
pub async fn fail_queue_task(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
) -> Result<TerminalWrite, DbErr> {
    set_terminal_status_owned(db, task_id, worker_id, "failed").await
}

/// Re-enqueue a task that was previously claimed or failed. Resets queue_status
/// to `queued`, clears worker_id/heartbeat, and updates the spec. Used during
/// recovery to re-launch tasks from their original spec.
pub async fn requeue_task(
    db: &DatabaseConnection,
    task_id: &str,
    spec: &agentic_core::delegation::TaskSpec,
) -> Result<(), DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    // Use upsert: INSERT if no row exists, UPDATE if it does.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO agentic_task_queue \
             (task_id, run_id, queue_status, spec, worker_id, last_heartbeat, \
              claimed_at, visibility_timeout_secs, claim_count, max_claims, created_at, updated_at) \
         VALUES ($1, $1, 'queued', $2, NULL, NULL, NULL, 60, 0, 3, now(), now()) \
         ON CONFLICT (task_id) DO UPDATE SET \
             queue_status = 'queued', \
             spec = $2, \
             worker_id = NULL, \
             last_heartbeat = NULL, \
             claimed_at = NULL, \
             claim_count = 0, \
             available_at = LEAST(agentic_task_queue.available_at, now()), \
             first_deferred_at = NULL, \
             updated_at = now()",
        [
            task_id.into(),
            serde_json::to_value(spec).unwrap().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Reset an EXISTING **terminal** task row back to `queued` in place — for a
/// reset-in-place retry. Unlike [`requeue_task`], this does NOT re-create the row
/// (no spec is re-supplied): it only revives a still-present task, keeping its
/// stored spec.
///
/// The `WHERE` guard excludes tasks that are already `queued` or `claimed`: a
/// retry only runs on a run-level-terminal run whose task is itself terminal, so
/// this never legitimately targets a live task — but the guard makes that intent
/// explicit and rules out revoking an in-flight worker's claim (a double-drive)
/// if this is ever called on a live task by mistake.
///
/// Returns rows affected; `0` means the task row was reaped **or** is still live
/// (queued/claimed) — either way the caller should fall back to a fresh run
/// rather than reset in place.
/// A revival is FRESH WORK: it clears the wait-streak and can only ever make
/// the task MORE visible (`LEAST(available_at, now())`, never a bare `now()`).
///
/// Not redundant, despite reviving only non-`queued`/`claimed` rows:
/// `defer_task` sets `queue_status = 'dead'` and `available_at = now() + delay`
/// in the SAME statement, so a dead-lettered task is terminal AND invisible
/// with a streak that already exceeds the ceiling. Reviving it without both
/// writes sends it straight back to `dead` on its first contention.
///
/// The same treatment is applied at the other three doors into `queued` —
/// `enqueue_task`, `requeue_task`, and the admin `reenqueue_dead` handler.
pub async fn reset_task_to_queued(db: &DatabaseConnection, task_id: &str) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET \
                 queue_status = 'queued', worker_id = NULL, last_heartbeat = NULL, \
                 claimed_at = NULL, claim_count = 0, \
                 available_at = LEAST(available_at, now()), \
                 first_deferred_at = NULL, \
                 updated_at = now() \
             WHERE task_id = $1 AND queue_status NOT IN ('queued', 'claimed')",
            [task_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Make a task globally claimable by the coordinator (`scope_owned = false`).
///
/// The global [`claim_task`] only picks up `scope_owned = false` tasks; a
/// SCOPE_OWNED task (e.g. a backfill chunk, normally driven under its range's
/// scope) is otherwise never re-claimed. A reset-in-place retry of such a run
/// must call this so the coordinator actually re-drives the re-queued task. A
/// no-op for an already-global run. Returns rows affected (`0` = task reaped).
pub async fn mark_task_global(db: &DatabaseConnection, task_id: &str) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET scope_owned = false, updated_at = now() \
             WHERE task_id = $1",
            [task_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// After [`release_claims_for_worker`], make the released **roots** eligible
/// for the global recovery path by clearing `scope_owned`.
///
/// Scoped rows are otherwise invisible to `claim_task` and excluded from
/// `find_stuck_runs`, so an orphan waits for a process restart. Clearing the
/// flag lets `find_pending_global_runs` select the root for
/// `recover_single_run`, which rebuilds a scoped transport and re-adopts the
/// whole subtree via [`claim_task_under_root`].
///
/// **Gated to `workflow` / `airway` on purpose.** Recovery re-executes the
/// interrupted unit from scratch. For automation/airway that is one step; for
/// an agent run it is a full turn, i.e. duplicate LLM calls. This mirrors the
/// `source_type` filter in `find_stuck_runs`. Agent runs stay stranded until
/// restart — the existing, deliberate trade.
///
/// Only roots are touched (`parent_task_id IS NULL`): a descendant claimed in
/// isolation has its outcome dropped. See [`claim_task`].
///
/// Matches on the task row's `worker_id` + `queue_status = 'claimed'`, so
/// this must run **before** [`release_claims_for_worker`] clears both —
/// order is not incidental.
///
/// Returns the number of roots made eligible.
pub async fn mark_released_roots_global(
    db: &DatabaseConnection,
    worker_id: &str,
) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue q \
             SET scope_owned = false, updated_at = now() \
             FROM agentic_runs r \
             WHERE q.run_id = r.id \
               AND q.worker_id = $1 \
               AND q.queue_status = 'claimed' \
               AND q.parent_task_id IS NULL \
               AND q.scope_owned = true \
               AND r.source_type IN ('workflow', 'airway')",
            [worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Cancel a live (`queued` or `claimed`) task **on someone else's behalf**.
///
/// This is the *requester* side of cancellation — `CoordinatorTransport::cancel`
/// and `cancel_subtree`, reached from a user pressing Stop or a parent tearing
/// down a subtree. The caller holds no claim on the row (the worker executing it
/// may not even be in this process), so an ownership predicate would break it.
/// Deliberately **not** used by the worker's own outcome path; that one must be
/// ownership-scoped — see [`cancel_queued_task_owned`].
pub async fn cancel_queued_task(db: &DatabaseConnection, task_id: &str) -> Result<(), DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_task_queue SET queue_status = 'cancelled', updated_at = now() \
         WHERE task_id = $1 AND queue_status IN ('queued', 'claimed')",
        [task_id.into()],
    ))
    .await?;
    Ok(())
}

/// Cancel every live (`queued`/`claimed`) row in `run_id`'s task tree — the
/// root row itself plus its `run_id.N` descendants.
///
/// The tree predicate mirrors [`super::recovery::find_pending_global_runs`]'s
/// own `q.task_id = r.id OR q.task_id LIKE r.id || '.%'`, so what this cancels
/// is exactly what makes a run selectable there.
///
/// Retiring a run is not enough on its own: [`claim_task`] has **no
/// run-status predicate**, so a row left `queued` under a run that has gone
/// terminal is still claimable by any global worker. Both halves have to move
/// together or the queue row outlives the run it belongs to.
pub async fn cancel_queued_tasks_for_run<C: ConnectionTrait>(
    db: &C,
    run_id: &str,
) -> Result<u64, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = 'cancelled', updated_at = now() \
             WHERE (task_id = $1 OR task_id LIKE $1 || '.%') \
               AND queue_status IN ('queued', 'claimed')",
            [run_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Fail the claim **this process** holds on `task_id`, without the caller
/// having to name the worker.
///
/// One caller: the coordinator receiving an outcome for a task absent from its
/// in-memory map. That outcome has nowhere to go, so the row must not be left
/// `claimed` — a suspended task keeps its heartbeat ticker running on purpose
/// (see `Worker::handle_task`), so a row abandoned this way looks alive to the
/// reaper forever and never reaches `find_stuck_runs` recovery.
///
/// **Ownership-scoped, despite the caller not supplying a `worker_id`.** The
/// outcome reached this coordinator over `DurableTransport`'s in-memory
/// channel, so the worker that produced it is in *this* process, and
/// [`crate::orchestrator::transport::process_worker_id`] names its claim
/// exactly. Without the fence the write
/// would also match a row a peer had since re-claimed (or one sitting
/// legitimately `queued`), silently killing live work — the same unfenced-write
/// hazard that makes [`update_queue_heartbeat`] unable to police stale tickers.
/// Matching nothing is the correct outcome in those cases: whoever owns the row
/// now is the one responsible for it.
///
/// Contrast [`cancel_queued_task`], which is genuinely unscoped because a user
/// pressing Stop really is a third party, and [`fail_queue_task`], which the
/// worker's own outcome path uses with a `worker_id` it holds directly.
///
/// Stamping the row terminal also stops the ticker without needing a handle on
/// it — [`update_queue_heartbeat`] requires `queue_status = 'claimed'`, so the
/// next tick returns `Ok(false)` and the loop breaks itself.
pub async fn fail_orphaned_claim(db: &DatabaseConnection, task_id: &str) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let worker_id = crate::orchestrator::transport::process_worker_id();
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET queue_status = 'failed', worker_id = NULL, claimed_at = NULL, \
                 last_heartbeat = NULL, updated_at = now() \
             WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed'",
            [task_id.into(), worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Cancel a task **this worker holds**, reporting its own `Cancelled` outcome.
///
/// The ownership-scoped counterpart to [`cancel_queued_task`]: the status guard
/// alone is not enough on this path, because after graceful release the row can
/// legitimately be `claimed` again — by a *peer*. See
/// [`set_terminal_status_owned`] for the full failure mode.
///
/// This is [`set_terminal_status_owned`] with `status = "cancelled"` and
/// nothing else. It reads as its own function because the *call site* is the
/// thing worth naming (the worker's own outcome path, as opposed to the
/// requester-side [`cancel_queued_task`]), but the SQL must not diverge:
/// the ordinary sequence is "requester stamps `cancelled`, then the worker
/// reports `Cancelled` a beat later", and any extra predicate here turns that
/// entirely healthy handshake into a spurious miss on every user-pressed Stop.
pub async fn cancel_queued_task_owned(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
) -> Result<TerminalWrite, DbErr> {
    set_terminal_status_owned(db, task_id, worker_id, "cancelled").await
}

/// A task this reaper cycle moved to `dead`, captured **as it was while still
/// claimed** — the dying worker's id is the whole point, and dead-lettering
/// nulls `worker_id` out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadTask {
    pub task_id: String,
    pub run_id: String,
    /// The worker that held the claim when it was reaped. `None` only for a
    /// row that was somehow `claimed` with no owner — itself worth seeing.
    pub worker_id: Option<String>,
}

/// What a reaper cycle did. Kept split because a requeue is routine churn
/// while a dead-letter is data leaving the pipeline — conflating them (as the
/// previous `u64` return did) made the dead-letter rate invisible in logs and
/// metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapOutcome {
    /// Stale claims returned to `queued` for another worker to pick up.
    pub requeued: u64,
    /// Claims that exhausted `max_claims` and were moved to `dead`.
    pub dead_lettered: u64,
    /// Identity of each row moved to `dead` this cycle.
    ///
    /// Production reads this only inside [`reap_stale_tasks`], which emits the
    /// per-row `warn!` before returning; no caller consumes it today. It is
    /// carried on the outcome anyway because it is the **only** assertion
    /// surface for that log line — the worker id appears nowhere else, and
    /// without it a regression that silently logs `worker_id = <none>` (which
    /// is exactly what the first cut of this code shipped) is invisible to
    /// every test. Requeues deliberately stay aggregate: they are routine and
    /// would flood a 30s loop.
    pub dead_tasks: Vec<DeadTask>,
}

impl ReapOutcome {
    /// Rows touched in total. Use only for "did anything happen" checks —
    /// prefer the split fields for logging and metrics.
    pub fn total(&self) -> u64 {
        self.requeued + self.dead_lettered
    }
}

/// Monotonic reap-event counters, read by `oxy-app`'s metrics endpoint
/// (`/metrics`, `oxy worker --health-port`).
///
/// Incremented inside [`reap_stale_tasks`] itself, not by any one caller.
/// This function is the single choke point every reap path funnels
/// through: the periodic `orchestrator::background::run_reaper_cycle` loop,
/// and — via `DurableTransport::run_reaper` — the `oxy worker` startup
/// pre-pass, the admin `/run-reaper` handler, and pipeline recovery.
/// Counting one call further up the stack (as a previous version of this
/// code did, in `run_reaper_cycle`) silently missed the other three;
/// counting here is correct by construction for all of them.
///
/// They live in `crud` rather than `background` on purpose: `background`
/// already depends on `crud` (it calls [`reap_stale_tasks`]), and a static
/// referenced from both directions would invert that — `crud` is the lower
/// layer, so the statics belong here and `background` (and everyone else)
/// reads them from this module.
pub static TASKS_REQUEUED: AtomicU64 = AtomicU64::new(0);
/// See [`TASKS_REQUEUED`] — same rationale, counts dead-letters instead of
/// requeues.
pub static TASKS_DEAD_LETTERED: AtomicU64 = AtomicU64::new(0);

/// Reap stale claimed tasks whose heartbeat has expired past their
/// visibility timeout. Tasks that have exceeded `max_claims` are
/// dead-lettered instead of re-queued.
pub async fn reap_stale_tasks(db: &DatabaseConnection) -> Result<ReapOutcome, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement, TransactionTrait};

    // Both statements run in one transaction so a failure of either leaves
    // *neither* applied. Without it, a dead-letter UPDATE that autocommits
    // followed by a failing requeue returns `Err` before `ReapOutcome` is
    // built, so the caller's `Err` arm never increments
    // `oxy_tasks_dead_lettered_total` — and those rows are now `'dead'`, so
    // no later cycle re-matches `queue_status = 'claimed'` to count them.
    // The batch would be permanently missing from the metric. Atomicity means
    // the retry on the next tick re-does and re-counts the whole cycle.
    let txn = db.begin().await?;

    // Dead-letter tasks that have been claimed too many times.
    //
    // The identity of the dying row is captured in a CTE *before* the update
    // rather than by a plain `RETURNING`, because Postgres `RETURNING`
    // reflects the row's **post-update** state: with `SET worker_id = NULL`
    // in the same statement, `RETURNING worker_id` yields NULL, silently
    // reducing the whole point of this log line to `worker_id = <none>`.
    // The `FOR UPDATE` both locks the doomed rows for the update that follows
    // and forces the CTE to materialize; it also re-checks the predicate
    // against the latest row version, so a task whose worker heartbeats
    // concurrently drops out of the batch instead of being wrongly reaped.
    #[derive(FromQueryResult)]
    struct DeadRow {
        task_id: String,
        run_id: String,
        worker_id: Option<String>,
    }

    let dead_rows = DeadRow::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        "WITH dying AS ( \
             SELECT task_id, run_id, worker_id \
             FROM agentic_task_queue \
             WHERE queue_status = 'claimed' \
               AND claim_count >= max_claims \
               AND last_heartbeat < now() - (visibility_timeout_secs || ' seconds')::interval \
             FOR UPDATE \
         ) \
         UPDATE agentic_task_queue q \
         SET queue_status = 'dead', worker_id = NULL, claimed_at = NULL, updated_at = now() \
         FROM dying d \
         WHERE q.task_id = d.task_id \
         RETURNING d.task_id, d.run_id, d.worker_id"
            .to_string(),
    ))
    .all(&txn)
    .await?;

    // Re-queue tasks that are still under max_claims. `last_heartbeat` is
    // nulled to match `requeue_task` / `reset_task_to_queued` / the admin
    // `reenqueue_dead` path — otherwise the row sits `queued` carrying its
    // dead owner's expired heartbeat and "how long has this been queued"
    // is unanswerable from the row.
    let requeued = txn
        .execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET queue_status = 'queued', worker_id = NULL, claimed_at = NULL, \
                 last_heartbeat = NULL, updated_at = now() \
             WHERE queue_status = 'claimed' \
               AND claim_count < max_claims \
               AND last_heartbeat < now() - (visibility_timeout_secs || ' seconds')::interval"
                .to_string(),
        ))
        .await?;

    txn.commit().await?;

    // Bump the counters here, after the commit — never before it and never
    // in the caller. After it: a cycle whose transaction rolled back (the
    // `?` above already returned `Err`) must not be counted, since the next
    // tick will re-do and re-count the whole cycle. In this function, not
    // the caller: see [`TASKS_REQUEUED`] for why counting here is what
    // makes all four production reap call sites observed, not just one.
    TASKS_REQUEUED.fetch_add(requeued.rows_affected(), Ordering::Relaxed);
    TASKS_DEAD_LETTERED.fetch_add(dead_rows.len() as u64, Ordering::Relaxed);

    // Logged only after the commit: a dead-letter that rolled back is not a
    // dead-letter, and an operator chasing these must not be sent after rows
    // that are still queued.
    let dead_tasks: Vec<DeadTask> = dead_rows
        .into_iter()
        .map(|r| DeadTask {
            task_id: r.task_id,
            run_id: r.run_id,
            worker_id: r.worker_id,
        })
        .collect();

    for t in &dead_tasks {
        tracing::warn!(
            target: "background",
            task_id = %t.task_id,
            run_id = %t.run_id,
            worker_id = t.worker_id.as_deref().unwrap_or("<none>"),
            "dead-lettered: task exhausted max_claims"
        );
    }

    Ok(ReapOutcome {
        requeued: requeued.rows_affected(),
        dead_lettered: dead_tasks.len() as u64,
        dead_tasks,
    })
}

/// Return every claim held by `worker_id` to the queue, **without charging
/// the retry budget**.
///
/// Called on graceful shutdown. Releasing a lease you provably hold is safe;
/// *stealing* one from a worker you merely believe is dead is not — which is
/// why this is keyed on the caller's own `worker_id` and never runs from a
/// successor sweeping on someone else's behalf.
///
/// `claim_count` is decremented (floored at 0) because the claim was given
/// back rather than spent. Without this, a task bounced by three rolling
/// deploys exhausts `max_claims` and dead-letters despite never failing.
/// A hard crash (SIGKILL/OOM) never reaches this path and still charges,
/// which is correct: an OOM may genuinely be the task's fault.
///
/// The `claimed -> queued` transition fires the `agentic_task_queue` NOTIFY
/// trigger, so surviving workers wake immediately instead of waiting out the
/// visibility timeout.
///
/// Returns the number of claims released.
pub async fn release_claims_for_worker(
    db: &DatabaseConnection,
    worker_id: &str,
) -> Result<u64, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE agentic_task_queue {RELEASE_SET_CLAUSE} \
                 WHERE worker_id = $1 AND queue_status = 'claimed'"
            ),
            [worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// What [`defer_task`] did.
///
/// Not a `bool`: "we deferred it", "it has waited too long and is now dead",
/// and "it is not ours to defer" are three different facts, and a caller that
/// cannot tell them apart cannot log or alert correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferOutcome {
    /// Returned to the queue, invisible until the delay elapses.
    Deferred,
    /// Waited longer than the caller's ceiling; moved to `dead` instead.
    DeadLettered,
    /// This worker no longer holds the claim — nothing was changed.
    NotHeld,
}

/// Return a claimed task to the queue, invisible until `delay_secs` from now.
///
/// The counterpart to a claim that neither completes nor fails: the worker
/// looked at the task, decided it cannot run *yet*, and wants it retried
/// without occupying a slot in the meantime.
///
/// `claim_count` is deliberately **decremented** back. A deferral is not an
/// attempt — it is the absence of one. Leaving it incremented would walk an
/// indefinitely-contended task toward `max_claims` and dead-letter it for
/// waiting its turn, which is the failure the caller is trying to avoid.
///
/// Two paths deliberately do NOT clear `first_deferred_at`: the REAPER's
/// re-queue and [`RELEASE_SET_CLAUSE`] (graceful shutdown refund). Both revive
/// a row that was CLAIMED, so they continue the same attempt rather than
/// expressing fresh intent — unlike the four doors listed on
/// `reset_task_to_queued`, which are all a caller asking for the work again.
///
/// Clearing the streak on a claim-side path would reset the clock on every
/// defer -> claim -> defer cycle, and the wall-clock bound below would never be
/// reached. (A reap itself is rare — it needs a worker to die — but it sits on
/// the claim side, and that is what decides it.) The accepted cost is that a
/// task reaped after genuinely running for hours carries that time into its
/// next contention.
///
/// STARVATION is bounded by `max_wait_secs` measured in WALL CLOCK from the
/// first defer of the current streak, not by a number of deferrals: the retry
/// interval is a domain's choice and can change, so N defers is not a bounded
/// amount of time. A task that has waited longer than the ceiling moves to
/// `dead` rather than waiting in silence — a permanently blocked pipeline has
/// to be visible as a failure, not as a queue that looks healthy.
///
/// Scoped to `worker_id` so a worker cannot defer a task another worker has
/// since claimed (the reaper may have reassigned it while this one was
/// deciding).
pub async fn defer_task(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
    delay_secs: i64,
    max_wait_secs: i64,
) -> Result<DeferOutcome, DbErr> {
    // One statement so the streak read and the write cannot interleave with a
    // peer's claim: `first_deferred_at` is both read and written here.
    let sql = "\
        UPDATE agentic_task_queue \
        SET queue_status = CASE \
                WHEN COALESCE(first_deferred_at, now()) <= now() - make_interval(secs => $4) \
                THEN 'dead' ELSE 'queued' END, \
            worker_id = NULL, \
            claimed_at = NULL, \
            last_heartbeat = NULL, \
            claim_count = GREATEST(claim_count - 1, 0), \
            first_deferred_at = COALESCE(first_deferred_at, now()), \
            available_at = now() + make_interval(secs => $3), \
            updated_at = now() \
        WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed' \
        RETURNING queue_status";

    use sea_orm::FromQueryResult;
    #[derive(FromQueryResult)]
    struct Row {
        queue_status: String,
    }

    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [
            task_id.into(),
            worker_id.into(),
            // `make_interval(secs => …)` takes double precision; bind it as
            // such rather than leaning on implicit cast resolution (matches
            // `pipeline_lease::try_acquire`).
            (delay_secs as f64).into(),
            (max_wait_secs as f64).into(),
        ],
    ))
    .one(db)
    .await?;

    Ok(match row {
        None => DeferOutcome::NotHeld,
        Some(r) if r.queue_status == "dead" => {
            TASKS_DEAD_LETTERED.fetch_add(1, Ordering::Relaxed);
            DeferOutcome::DeadLettered
        }
        Some(_) => DeferOutcome::Deferred,
    })
}

/// The `claimed -> queued` refund shared by both release paths.
///
/// **Two independent guards make a repeated release a no-op, and either one
/// alone would suffice.** The obvious one is the callers' `queue_status =
/// 'claimed'` predicate. The second is right here: `worker_id = NULL`. Once a
/// row has been released it matches neither `worker_id = $1` nor
/// `queue_status = 'claimed'`, so [`drain_claims_for_worker`]'s extra passes
/// cannot double-refund `claim_count`.
///
/// Do not "simplify" `worker_id = NULL` out of this clause on the grounds that
/// the status predicate already covers it. It does — today. Removing either
/// guard silently reduces a belt-and-braces property to a single point of
/// failure, and only the *combination* is what makes the drain safe to run
/// unconditionally. `eviction_safety_test.rs`'s successor-claim and
/// peer-takeover tests both fail if it goes.
const RELEASE_SET_CLAUSE: &str = "SET queue_status = 'queued', \
     worker_id = NULL, \
     claimed_at = NULL, \
     last_heartbeat = NULL, \
     claim_count = GREATEST(claim_count - 1, 0), \
     updated_at = now()";

/// Hand a **single** claim back, without charging the retry budget.
///
/// The per-row counterpart to [`release_claims_for_worker`], used by
/// `DurableTransport::recv_assignment` the instant it notices it has claimed a
/// task while the process is shutting down. Scoped to one row on purpose: at
/// that point sibling workers in this process are still legitimately executing
/// their own claims, and a blanket release would yank rows out from under work
/// that is going to finish.
///
/// Returns `true` if a claim was actually handed back.
pub async fn release_claim(
    db: &DatabaseConnection,
    task_id: &str,
    worker_id: &str,
) -> Result<bool, DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE agentic_task_queue {RELEASE_SET_CLAUSE} \
                 WHERE task_id = $1 AND worker_id = $2 AND queue_status = 'claimed'"
            ),
            [task_id.into(), worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected() > 0)
}

/// How many times [`drain_claims_for_worker`] will re-run the release before
/// giving up. Three is enough to close the gate-to-release race twice over;
/// the bound exists so a pathological producer can never hold shutdown open.
const RELEASE_DRAIN_ATTEMPTS: usize = 3;

/// Release this worker's claims repeatedly until a pass matches nothing.
///
/// **Defense in depth, not the primary fix.** The gate-to-claim race — a
/// worker that cleared `is_shutting_down()` and is awaiting its `claim_task`
/// round-trip when the flag flips — is closed at its cause, in
/// `DurableTransport::recv_assignment`, which re-checks the flag after the
/// claim lands and hands the row straight back via [`release_claim`]. That is
/// the only place with certain knowledge that the claim exists.
///
/// This loop cannot close that race on its own and should not be trusted to:
/// if the process held no *other* claims, pass 1 returns 0 and the loop exits
/// before the straggler ever lands. It is kept because it costs one cheap
/// UPDATE and covers the residual case where a claim lands between the
/// cause-level release and process exit.
///
/// Re-running is free of risk: the release is idempotent by construction — see
/// [`RELEASE_SET_CLAUSE`] for the two independent guards that make a second
/// pass match zero rows, which
/// `releasing_twice_is_a_no_op_and_does_not_double_refund` pins down. Stops
/// early on the first empty pass. Returns the total number of claims released.
///
/// On a mid-drain DB error the count from the passes that already succeeded is
/// logged before the error propagates — those claims really were released, and
/// an operator reading only "failed to release claims" would conclude the
/// opposite.
pub async fn drain_claims_for_worker(
    db: &DatabaseConnection,
    worker_id: &str,
) -> Result<u64, DbErr> {
    let mut total = 0;
    for attempt in 0..RELEASE_DRAIN_ATTEMPTS {
        match release_claims_for_worker(db, worker_id).await {
            Ok(0) => break,
            Ok(released) => total += released,
            Err(e) => {
                if total > 0 {
                    tracing::warn!(
                        target: "agentic",
                        worker_id,
                        released = total,
                        attempt,
                        "claim drain failed part-way: the claims counted here were \
                         already released and are back on the queue"
                    );
                }
                return Err(e);
            }
        }
    }
    Ok(total)
}

/// Delete terminal task-queue rows older than their retention window, so the
/// queue doesn't grow unbounded with the history of every job ever run.
///
/// Two windows on purpose: `completed_ttl` covers the bulk happy-path rows
/// (`completed`, `cancelled`), `dead_ttl` covers `failed`/`dead` rows which
/// stay longer because they're the dead-letter triage surface. A `None` window
/// disables pruning for that class (keep forever). Only ever touches TERMINAL
/// rows — never `queued`/`claimed` — so an in-flight task can't be deleted out
/// from under a worker. FK-safe: `agentic_task_outcomes` references
/// `agentic_runs`, not this table.
pub async fn purge_old_terminal_tasks(
    db: &DatabaseConnection,
    completed_ttl: Option<std::time::Duration>,
    dead_ttl: Option<std::time::Duration>,
) -> Result<u64, DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let mut total = 0u64;

    if let Some(ttl) = completed_ttl {
        let secs = ttl.as_secs() as i64;
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM agentic_task_queue \
                 WHERE queue_status IN ('completed', 'cancelled') \
                   AND updated_at < now() - ($1 * interval '1 second')",
                [secs.into()],
            ))
            .await?;
        total += res.rows_affected();
    }

    if let Some(ttl) = dead_ttl {
        let secs = ttl.as_secs() as i64;
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM agentic_task_queue \
                 WHERE queue_status IN ('failed', 'dead') \
                   AND updated_at < now() - ($1 * interval '1 second')",
                [secs.into()],
            ))
            .await?;
        total += res.rows_affected();
    }

    Ok(total)
}

/// A plain DTO for a task queue entry, avoiding leaking entity types.
pub struct QueueTaskRow {
    pub task_id: String,
    pub run_id: String,
    pub queue_status: String,
    pub worker_id: Option<String>,
    pub claim_count: i32,
    pub max_claims: i32,
    pub last_heartbeat: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<task_queue::Model> for QueueTaskRow {
    fn from(m: task_queue::Model) -> Self {
        Self {
            task_id: m.task_id,
            run_id: m.run_id,
            queue_status: m.queue_status,
            worker_id: m.worker_id,
            claim_count: m.claim_count,
            max_claims: m.max_claims,
            last_heartbeat: m.last_heartbeat,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// Queue status counts and stale/dead-lettered task details for the dashboard.
pub struct QueueStats {
    pub queued: u64,
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub dead: u64,
    /// Tasks claimed but with heartbeat older than their visibility timeout.
    pub stale_tasks: Vec<QueueTaskRow>,
    /// Tasks that have been dead-lettered.
    pub dead_tasks: Vec<QueueTaskRow>,
}

/// **Workspace-scoped** by joining `agentic_task_queue.run_id` →
/// `agentic_runs.workspace_id`. The queue table itself doesn't carry a
/// `workspace_id` column today; the JOIN keeps the dashboard
/// tenant-correct without a schema change.
///
/// **The customer's own work only.** This is what a workspace's members read
/// of the queue, so the join also applies the run feed's rule
/// (`visibility::customer_run_sql`): a task of a workspace preview's dry run,
/// or of a custom-app check run staff queued outside production, is neither
/// counted nor listed. A dead sandbox check is not the tenant's dead job, and
/// listing it would hand out the run id every other route hides.
pub async fn get_queue_stats(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<QueueStats, DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    // One predicate for all three statements, over the `r` they all join.
    let customers_own = crate::lifecycle::crud::visibility::customer_run_sql("r.");

    // Count by status in a single query.
    #[derive(Debug, FromQueryResult)]
    struct StatusCount {
        queue_status: String,
        cnt: i64,
    }

    let rows = StatusCount::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT q.queue_status, COUNT(*) as cnt \
             FROM agentic_task_queue q \
             INNER JOIN agentic_runs r ON r.id = q.run_id \
             WHERE r.workspace_id = $1 AND {customers_own} \
             GROUP BY q.queue_status"
        ),
        [workspace_id.into()],
    ))
    .all(db)
    .await?;

    let mut stats = QueueStats {
        queued: 0,
        claimed: 0,
        completed: 0,
        failed: 0,
        cancelled: 0,
        dead: 0,
        stale_tasks: vec![],
        dead_tasks: vec![],
    };

    for row in &rows {
        let count = row.cnt as u64;
        match row.queue_status.as_str() {
            "queued" => stats.queued = count,
            "claimed" => stats.claimed = count,
            "completed" => stats.completed = count,
            "failed" => stats.failed = count,
            "cancelled" => stats.cancelled = count,
            "dead" => stats.dead = count,
            _ => {}
        }
    }

    // Fetch stale tasks (claimed but heartbeat expired).
    stats.stale_tasks = task_queue::Model::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT q.* FROM agentic_task_queue q \
             INNER JOIN agentic_runs r ON r.id = q.run_id \
             WHERE r.workspace_id = $1 AND {customers_own} \
               AND q.queue_status = 'claimed' \
               AND q.last_heartbeat < now() - (q.visibility_timeout_secs || ' seconds')::interval \
             ORDER BY q.last_heartbeat \
             LIMIT 50"
        ),
        [workspace_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(QueueTaskRow::from)
    .collect();

    // Fetch dead-lettered tasks (most recent first).
    stats.dead_tasks = task_queue::Model::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT q.* FROM agentic_task_queue q \
             INNER JOIN agentic_runs r ON r.id = q.run_id \
             WHERE r.workspace_id = $1 AND {customers_own} AND q.queue_status = 'dead' \
             ORDER BY q.updated_at DESC"
        ),
        [workspace_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(QueueTaskRow::from)
    .collect();

    Ok(stats)
}
