//! The automatic compile enqueue: the lazy self-heal a request makes when it
//! finds nothing usable compiled, the GitHub import's first compile, and the
//! deduped, backed-off enqueue every automatic trigger (this loop's check and
//! the `push` webhook among them) goes through.
//!
//! Moved out of `api::middlewares::workspace_context` (2026-11-21), which
//! re-exports [`enqueue_lazy_compile`] and [`enqueue_import_compile`] so their
//! callers are unchanged.

use chrono::Utc;
use sea_orm::EntityTrait;
use uuid::Uuid;

/// Enqueue a promoting compile for an uncompiled workspace, deduped against any
/// compile already queued/claimed for it. Best-effort — failures are logged,
/// never surfaced (the caller is on a request hot path).
/// How long to wait after a FAILED compile before the lazy self-heal will
/// auto-retry the same workspace. Without this, a persistently-broken workspace
/// (e.g. a config the round-trip gate rejects) becomes a recompile storm: every
/// failed compile clears the in-flight dedup, so the very next request
/// re-enqueues. Operators can still force an immediate compile via the admin
/// "Run compile now". A const, not an env flag — keep the surface small.
///
/// Also the window `agentic_wiring::project_ctx::compile_cooldown` holds a
/// request's compile request back after a *successful* compile — the same
/// "not the same tree again sooner than this" judgement, read there.
pub const LAZY_COMPILE_BACKOFF_SECS: i64 = 300;

/// Ceiling for the self-heal backoff, however many compiles in a row failed.
const LAZY_COMPILE_BACKOFF_MAX_SECS: i64 = 6 * 60 * 60;

/// How many recent `main` revisions are read to count consecutive failures.
/// Enough to reach the ceiling: 300s doubled 7 times passes 6h.
const LAZY_COMPILE_FAILURE_LOOKBACK: u64 = 8;

/// Every terminal state a compile task can reach.
///
/// All four, deliberately — the backoff window must be "the last N terminal
/// compile tasks", not "the last N failed ones". Reading only the failures
/// makes the run unbreakable: `completed` and `cancelled` are purged after 7
/// days while `failed`/`dead` are kept for 30 (`purge_old_terminal_tasks`), so
/// a workspace that broke one afternoon, was fixed, and ran clean for three
/// weeks would have its eight old failures counted as consecutive with one
/// fresh one — six hours of suppressed self-heal from a single transient
/// failure. The revisions guard above reads every status and does not have
/// this hazard; this list is what keeps the two from drifting apart.
const COMPILE_TASK_TERMINAL_STATUSES: [&str; 4] = ["completed", "failed", "dead", "cancelled"];

/// The backoff window read, frozen against the index that serves it.
///
/// `idx_task_queue_compile_terminal` (runtime migration `AddCompileTaskBackoffIndex`)
/// is `((spec->>'workspace_id'), updated_at DESC) WHERE spec->>'type' = 'compile'`.
/// Without it this is a sequential scan plus a sort over every retained
/// terminal row in the table — 7 days of `completed` and 30 of `failed`/`dead`
/// for every workspace and every `TaskSpec` variant — run on the request hot
/// path while holding an advisory lock. The two existing indexes cannot help:
/// both are partial on `queued` / `claimed`, the non-terminal rows.
///
/// The index ships in the 0.5.154 release. The 0.5.153 *release* — tag
/// `0.5.153`, the carrier for ledger tolerance — held its registration back,
/// so a database that only ever ran that image serves this query with the
/// sequential scan described above. A `main` build still reporting 0.5.153
/// after the cut does carry it: the version names the release, not the build.
///
/// Change either predicate, the ordering, or the column list and the index
/// silently stops being used. That is a planner regression, not an error, so
/// `the_backoff_window_matches_the_index_that_serves_it` pins it.
///
/// Only compiles of the MAIN revision count (a residual filter on the same
/// index scan, not a new predicate): a staging compile — a staff preview, or
/// `oxyc publish --semantic-branch` — failing on its branch must not back off
/// the self-heal of the workspace's main revision.
const RECENT_COMPILE_TASKS_SQL: &str = "SELECT queue_status, updated_at FROM agentic_task_queue \
     WHERE spec->>'type' = 'compile' \
       AND spec->>'workspace_id' = $1 \
       AND queue_status IN ('completed', 'failed', 'dead', 'cancelled') \
       AND COALESCE(spec->>'kind', 'main') = 'main' \
     ORDER BY updated_at DESC \
     LIMIT $2";

/// How long a working-copy compile (`from_git: false`) may sit unclaimed before
/// it is treated as having no node that can take it.
///
/// Only a process that owns the workspace's files (an IDE node) claims one
/// (`worker`'s `!from_git && !process_can_compile()` refusal), and it
/// polls about every second. Unclaimed for ten minutes, there is no such node in
/// this deployment (prod has none: INC-164), so the row would sit `queued`
/// forever, its run showing `running`, and — counted as "in flight" — swallow
/// every later commit compile of a new head.
const WORKING_COPY_UNCLAIMED_BOUND_SECS: i64 = 10 * 60;

/// Why a retired working-copy compile's run says it ended.
const WORKING_COPY_UNCLAIMED_REASON: &str = "no node in this deployment holds a working copy; \
     unclaimed for 10 min";

/// The in-flight check for the lazy self-heal. A queued STAGING compile is not
/// a compile of the workspace's main revision, so it must not suppress one, and
/// neither does a working-copy compile nothing can claim (`$2` = the bound).
const IN_FLIGHT_MAIN_COMPILE_SQL: &str = "SELECT 1 FROM agentic_task_queue \
     WHERE queue_status IN ('queued', 'claimed') \
       AND spec->>'type' = 'compile' \
       AND spec->>'workspace_id' = $1 \
       AND COALESCE(spec->>'kind', 'main') = 'main' \
       AND NOT (COALESCE(spec->>'from_git', 'false') = 'false' \
                AND queue_status = 'queued' \
                AND claim_count = 0 \
                AND created_at < now() - make_interval(secs => $2::double precision)) \
     LIMIT 1";

/// Retire the stranded working-copy compiles of one workspace (`$1`), and end
/// their runs. The queue row goes `cancelled`, not `failed`: a failure would
/// start the self-heal backoff (`compile_task_status_is_failure`) and delay the
/// commit compile about to be queued, while a cancel breaks the failure run
/// (`leading_failure_count`) — nothing was attempted. The run is `cancelled`
/// too, with the reason in `error_message`: it keeps `failed_runs` in workspace
/// health honest, since no compile ran.
const RETIRE_UNCLAIMED_WORKING_COPY_SQL: &str = "WITH retired AS ( \
       UPDATE agentic_task_queue SET queue_status = 'cancelled', updated_at = now() \
       WHERE spec->>'type' = 'compile' \
         AND spec->>'workspace_id' = $1 \
         AND COALESCE(spec->>'kind', 'main') = 'main' \
         AND COALESCE(spec->>'from_git', 'false') = 'false' \
         AND queue_status = 'queued' \
         AND claim_count = 0 \
         AND created_at < now() - make_interval(secs => $2::double precision) \
       RETURNING run_id), \
     runs AS ( \
       UPDATE agentic_runs SET task_status = 'cancelled', error_message = $3, \
           driver_id = NULL, driver_heartbeat_at = NULL, updated_at = now() \
       WHERE id IN (SELECT run_id FROM retired) \
         AND (task_status IS NULL OR task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')) \
       RETURNING 1) \
     SELECT (SELECT count(*) FROM retired)::bigint AS n";

/// How long a retired working-copy compile counts as evidence that no node in
/// this deployment claims one. While it does, the working-copy self-heal queues
/// no replacement: the new row would be unclaimable too, and would count as in
/// flight for [`WORKING_COPY_UNCLAIMED_BOUND_SECS`], deduping every commit
/// compile hand-off in that window. An hour, so a deployment whose IDE node was
/// only briefly down resumes working-copy self-heal within the hour.
const RETIRED_WORKING_COPY_MEMORY_SECS: i64 = 60 * 60;

/// Whether `$1`'s workspace has a working-copy compile that was retired (or
/// cancelled) recently after waiting out the whole unclaimed bound (`$3`) with
/// no claim, within the memory window (`$2`).
const RECENTLY_RETIRED_WORKING_COPY_SQL: &str = "SELECT 1 FROM agentic_task_queue \
     WHERE spec->>'type' = 'compile' \
       AND spec->>'workspace_id' = $1 \
       AND COALESCE(spec->>'kind', 'main') = 'main' \
       AND COALESCE(spec->>'from_git', 'false') = 'false' \
       AND queue_status = 'cancelled' \
       AND claim_count = 0 \
       AND updated_at > now() - make_interval(secs => $2::double precision) \
       AND updated_at - created_at >= make_interval(secs => $3::double precision) \
     LIMIT 1";

/// Length of the leading run of failures in a NEWEST-FIRST status list.
///
/// Both backoff guards below count this way, over two different tables, so it
/// is one function: they must not drift. Leading, not total — a success (or a
/// cancel) breaks the run, because the backoff answers "is this workspace
/// failing right now", not "has it ever failed".
fn leading_failure_count<'a>(
    statuses: impl IntoIterator<Item = &'a str>,
    is_failure: impl Fn(&str) -> bool,
) -> u32 {
    statuses.into_iter().take_while(|s| is_failure(s)).count() as u32
}

/// Terminal `agentic_task_queue` states that mean the compile could not be done.
///
/// `cancelled` is terminal too and is deliberately NOT here: someone stopped
/// the work, which says nothing about whether it would have succeeded. It is
/// still *fetched*, so it breaks the run rather than being skipped over and
/// letting two failures separated by a cancel read as consecutive.
fn compile_task_status_is_failure(status: &str) -> bool {
    matches!(status, "failed" | "dead")
}

/// Backoff after `consecutive_failures` failed compiles in a row.
///
/// Self-heal (`content_change == false`) doubles per failure: a workspace whose
/// `config.yml` deterministically fails was otherwise recompiled every 5
/// minutes forever — measured on prod 2026-09-10 as 3 workspaces producing 37
/// of the fleet's 93 `ERROR` lines an hour, each retry identical to the last.
/// A content change (a pull brought new commits) keeps the flat window: new
/// source is exactly what might fix it, and the fix must not wait hours.
fn lazy_compile_backoff_secs(consecutive_failures: u32, content_change: bool) -> i64 {
    if consecutive_failures == 0 {
        return 0;
    }
    if content_change {
        return LAZY_COMPILE_BACKOFF_SECS;
    }
    let doublings = (consecutive_failures - 1).min(16);
    (LAZY_COMPILE_BACKOFF_SECS << doublings).min(LAZY_COMPILE_BACKOFF_MAX_SECS)
}

/// Queue a compile because a request found nothing usable compiled.
///
/// A workspace serving a revision of a real commit that GitHub can give back
/// recompiles that commit (`compile_git`, any worker can take it); anything
/// else compiles the working copy, as before. See
/// [`crate::git::self_heal`].
pub async fn enqueue_lazy_compile(db: &sea_orm::DatabaseConnection, workspace_id: Uuid) {
    use crate::git::self_heal::{Heal, heal};
    // A burst of requests for one workspace decides once, before any read
    // (see `lazy_compile_throttle`).
    if !super::lazy_compile_throttle::first_in_window(workspace_id) {
        return;
    }
    let target = match heal(db, workspace_id).await {
        Heal::Commit(target) => target,
        Heal::CheckBranch => {
            // Only when a check will look at the workspace. When it will not
            // (not ready, or the last check found something a person has to
            // fix) no commit compile is coming, and the working-copy compile
            // is the only way to a first revision.
            if crate::reconcile::request_check(db, workspace_id).await {
                return;
            }
            return enqueue_compile_deduped(db, workspace_id, "lazy self-heal").await;
        }
        Heal::WorkingCopy => {
            return enqueue_compile_deduped(db, workspace_id, "lazy self-heal").await;
        }
    };
    // The same content again, so a failure backs off the way a self-heal
    // always has, not on the short window a new commit gets.
    let how = CompileEnqueue {
        from_git: true,
        new_content: false,
    };
    enqueue_compile_deduped_as(
        db,
        workspace_id,
        Some(target.git_sha),
        target.branch,
        "lazy self-heal",
        how,
    )
    .await
}

/// Queue the first compile of a repository the GitHub import just connected:
/// commit `git_sha`, the head of the default branch, fetched from git by
/// whichever pod claims it. The same enqueue a push to that branch makes
/// (`compile_reconcile::push`), so the import needs no working copy.
pub async fn enqueue_import_compile(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    git_sha: String,
    branch: String,
) {
    let how = CompileEnqueue {
        from_git: true,
        new_content: true,
    };
    enqueue_compile_deduped_as(
        db,
        workspace_id,
        Some(git_sha),
        Some(branch),
        "github import",
        how,
    )
    .await
}

/// The lazy self-heal's enqueue: the working tree is the identity, so there is
/// no commit to dedupe on. `oxy-compile` mints a unique `local-<uuid>` and
/// opts out of the idempotency index, so every call produces a fresh revision;
/// the enqueue-side dedupe in [`enqueue_compile_deduped_as`] is what keeps a
/// burst of requests from queuing a burst of compiles.
pub(crate) async fn enqueue_compile_deduped(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    reason: &str,
) {
    let how = CompileEnqueue {
        from_git: false,
        new_content: false,
    };
    enqueue_compile_deduped_as(db, workspace_id, None, None, reason, how).await
}

/// The two things an automatic trigger decides about the compile it queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompileEnqueue {
    /// Queue a commit compile (`compile_git`, fetched by whichever pod claims
    /// it) rather than a working-copy one. Needs `git_sha` to be a commit.
    pub from_git: bool,
    /// Whether this is source the workspace has not tried to compile before.
    /// New content keeps the flat retry window after a failure — it may be the
    /// fix. The same content again backs off exponentially, so a commit that
    /// cannot compile is not retried every few minutes for good.
    pub new_content: bool,
}

impl CompileEnqueue {
    /// The `source_type` the run is stamped with, which is what decides which
    /// pods may take it.
    fn source_type(self) -> &'static str {
        if self.from_git {
            agentic_runtime::coordinator::COMPILE_GIT_SOURCE_TYPE
        } else {
            agentic_runtime::coordinator::COMPILE_SOURCE_TYPE
        }
    }
}

/// [`enqueue_compile_deduped`] with the kind and the backoff chosen by the
/// caller. Every check — the per-workspace lock, both failure backoffs, the
/// in-flight test — is the same for both kinds, and sees both kinds: a commit
/// compile is not queued behind a working-copy one, or the other way round.
pub(crate) async fn enqueue_compile_deduped_as(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    git_sha: Option<String>,
    branch: Option<String>,
    reason: &str,
    how: CompileEnqueue,
) {
    use sea_orm::{
        ColumnTrait, ConnectionTrait, DatabaseBackend, QueryFilter, QueryOrder, QuerySelect,
        Statement, TransactionTrait,
    };

    // Serialise concurrent self-heal enqueues for the SAME workspace across
    // every replica. A plain "SELECT in-flight? then INSERT" is a TOCTOU
    // race: N first-hits to an uncompiled/drifted workspace (across N serve
    // replicas, or a single page's parallel requests) all pass the SELECT
    // before any INSERT commits, and all enqueue a redundant compile + a
    // bloat row in agentic_runs. We close the window with a transaction-
    // scoped advisory lock keyed on the workspace, taken NON-blocking: only
    // the lock holder runs the dedup-check + inserts; a concurrent caller
    // that can't get the lock immediately knows someone else is already
    // doing it and just returns. Using `pg_try_advisory_xact_lock` (not the
    // blocking variant) matters precisely in the thundering-herd case this
    // guards — we don't want N request-hot-path connections parked on a lock
    // for the one workspace that's already unhealthy. The lock auto-releases
    // on commit/rollback; distinct workspaces hash to distinct keys.
    let txn = match db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: begin txn failed");
            return;
        }
    };

    // Deterministic per-workspace lock key (low 63 bits of the UUID). A
    // collision with another advisory-lock user only adds harmless extra
    // serialisation; it can't cause incorrectness.
    let lock_key = (workspace_id.as_u128() as u64 & 0x7fff_ffff_ffff_ffff) as i64;
    let got_lock = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_try_advisory_xact_lock($1) AS locked",
            [lock_key.into()],
        ))
        .await;
    match got_lock {
        Ok(Some(row)) if row.try_get::<bool>("", "locked").unwrap_or(false) => {}
        Ok(_) => {
            // Lock held by another caller → it owns the check+insert. Skip.
            let _ = txn.rollback().await;
            return;
        }
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: try-advisory-lock failed");
            let _ = txn.rollback().await;
            return;
        }
    }

    // Backoff: if a compile for this workspace FAILED recently, don't auto-retry
    // on every request. A persistently-broken workspace would otherwise become a
    // recompile storm (each failed compile clears the in-flight dedup below, so
    // the next request re-enqueues). Wait out the window. Checked INSIDE the lock
    // alongside the in-flight dedup so concurrent first-hits agree.
    // Two columns, not whole rows: a failed revision carries `error_summary`,
    // the per-file failure report, and failing workspaces are exactly the ones
    // this reads. `started_at` is NOT NULL, so the ordering has no NULL trap.
    let recent: Vec<(String, Option<sea_orm::prelude::DateTimeWithTimeZone>)> =
        match entity::revisions::Entity::find()
            .select_only()
            .column(entity::revisions::Column::Status)
            .column(entity::revisions::Column::FinishedAt)
            .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
            .filter(entity::revisions::Column::Kind.eq("main"))
            .order_by_desc(entity::revisions::Column::StartedAt)
            .limit(LAZY_COMPILE_FAILURE_LOOKBACK)
            .into_tuple()
            .all(&txn)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                // Same outcome as before this backoff read existed: no known
                // failures, so the enqueue proceeds. Said out loud, not silent.
                tracing::warn!(?e, %workspace_id, "lazy compile: reading recent revisions failed");
                Vec::new()
            }
        };
    let consecutive_failures =
        leading_failure_count(recent.iter().map(|(s, _)| s.as_str()), |s| s == "failed");
    let backoff_secs = lazy_compile_backoff_secs(consecutive_failures, how.new_content);
    let last_failed_at = recent.first().and_then(|(_, finished_at)| *finished_at);
    if let Some(finished_at) = last_failed_at
        && backoff_secs > 0
        && Utc::now().fixed_offset() - finished_at < chrono::Duration::seconds(backoff_secs)
    {
        tracing::debug!(
            %workspace_id,
            consecutive_failures,
            backoff_secs,
            "lazy compile: backing off (recent compiles failed)"
        );
        let _ = txn.rollback().await;
        return;
    }

    // SECOND BACKOFF, over the TASK QUEUE rather than revisions — because the
    // one above cannot see a compile that failed before it started.
    //
    // `insert_compiling_revision` runs inside the compile itself, so a compile
    // rejected by `OxyCompileDispatcher::dispatch` (the workspace working copy
    // is not on this node) writes NO revision row at all. The read above then
    // finds zero failures, `lazy_compile_backoff_secs(0, _)` returns 0, and the
    // next request enqueues again — forever, at whatever rate requests arrive.
    //
    // Measured on oxy-staging 2026-09-24, which is what prompted this: workspace
    // 64ac780e for 724 of 737 failures in six hours, a NEW run and task id every
    // single time, exactly every 30 seconds. 246 failures in two hours, 246
    // distinct task ids — not one task retrying, a fresh one each pass. The
    // backoff designed to stop precisely that was blind to it.
    //
    // So this reads the terminal states the queue DOES record — ALL of them,
    // not just the failures, so that a success truncates the run exactly as it
    // does in the revisions guard — through the same counter and the same
    // window function as above. The two guards are independent: a failure
    // visible to either one suppresses the enqueue.
    let recent_tasks: Vec<(String, Option<sea_orm::prelude::DateTimeWithTimeZone>)> = match txn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RECENT_COMPILE_TASKS_SQL,
            [
                workspace_id.to_string().into(),
                (LAZY_COMPILE_FAILURE_LOOKBACK as i64).into(),
            ],
        ))
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| {
                Some((
                    r.try_get::<String>("", "queue_status").ok()?,
                    r.try_get::<sea_orm::prelude::DateTimeWithTimeZone>("", "updated_at")
                        .ok(),
                ))
            })
            .collect(),
        Err(e) => {
            // Same posture as the revisions read: degrade to "no known
            // failures" and say so, rather than block an enqueue on a
            // bookkeeping query.
            tracing::warn!(?e, %workspace_id, "lazy compile: reading recent compile tasks failed");
            Vec::new()
        }
    };
    let consecutive_task_failures = leading_failure_count(
        recent_tasks.iter().map(|(s, _)| s.as_str()),
        compile_task_status_is_failure,
    );
    let task_backoff_secs = lazy_compile_backoff_secs(consecutive_task_failures, how.new_content);
    if let Some((_, Some(updated_at))) = recent_tasks.first()
        && task_backoff_secs > 0
        && Utc::now().fixed_offset() - *updated_at < chrono::Duration::seconds(task_backoff_secs)
    {
        tracing::debug!(
            %workspace_id,
            consecutive_task_failures,
            backoff_secs = task_backoff_secs,
            "lazy compile: backing off (recent compile TASKS failed before starting)"
        );
        let _ = txn.rollback().await;
        return;
    }

    let retired = retire_stranded_working_copy_compiles(&txn, workspace_id).await;
    // No node here claims a working-copy compile, so another would only be
    // retired in turn — and, queued, would dedupe every commit hand-off meanwhile.
    if !how.from_git && (retired > 0 || recently_retired_working_copy(&txn, workspace_id).await) {
        tracing::warn!(
            %workspace_id,
            %reason,
            retired,
            "lazy compile: no node claims a working-copy compile here; not queueing another"
        );
        if let Err(e) = txn.commit().await {
            tracing::warn!(?e, %workspace_id, "lazy compile: committing the retire failed");
        }
        return;
    }

    // Re-check for an in-flight compile INSIDE the lock.
    let already = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            IN_FLIGHT_MAIN_COMPILE_SQL,
            [
                workspace_id.to_string().into(),
                WORKING_COPY_UNCLAIMED_BOUND_SECS.into(),
            ],
        ))
        .await;
    if matches!(already, Ok(Some(_))) {
        let _ = txn.rollback().await;
        return;
    }

    let task_id = Uuid::new_v4().to_string();
    // `agentic_task_queue.run_id` FKs to `agentic_runs.id`, so the run row must
    // exist before the task insert (mirrors `compile_request::enqueue_task`). Both run on the
    // same txn so the advisory lock covers them.
    if let Err(e) = agentic_runtime::crud::insert_run(
        &txn,
        &task_id,
        &format!("compile main ({reason})"),
        None,
        how.source_type(),
        Some(serde_json::json!({
            "workspace_id": workspace_id,
            "lazy": true,
            "reason": reason,
            "git_sha": git_sha,
        })),
        workspace_id,
    )
    .await
    {
        tracing::warn!(?e, %workspace_id, %reason, "auto compile insert_run failed");
        let _ = txn.rollback().await;
        return;
    }
    let spec = agentic_core::delegation::TaskSpec::Compile {
        workspace_id,
        git_sha,
        branch,
        promote: true,
        kind: Some("main".to_string()),
        owner_user_id: None,
        from_git: how.from_git,
    };
    let retry = how.from_git.then(crate::git::retry_policy);
    if let Err(e) = agentic_runtime::crud::enqueue_task(
        &txn,
        &task_id,
        &task_id,
        None,
        &spec,
        // A commit compile rides out a brief GitHub or Postgres outage on the
        // queue itself; a working-copy compile has nothing to retry.
        retry.as_ref(),
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await
    {
        tracing::warn!(?e, %workspace_id, %reason, "auto compile enqueue failed");
        let _ = txn.rollback().await;
        return;
    }

    match txn.commit().await {
        Ok(_) => tracing::info!(%workspace_id, %reason, "enqueued auto compile (deduped)"),
        Err(e) => tracing::warn!(?e, %workspace_id, %reason, "auto compile commit failed"),
    }
}

/// End the workspace's working-copy compiles that no node has claimed within
/// [`WORKING_COPY_UNCLAIMED_BOUND_SECS`], in the caller's advisory-locked txn so
/// they are gone before the in-flight check reads. Returns how many queue rows
/// it retired.
///
/// Best-effort, and that has to be true of the transaction too: a failed
/// statement aborts a Postgres transaction, and the compile the caller queues
/// next would go with it. So the statement runs in a savepoint (a nested
/// `begin()`), released on success and rolled back alone on error.
async fn retire_stranded_working_copy_compiles(
    txn: &sea_orm::DatabaseTransaction,
    workspace_id: Uuid,
) -> u64 {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
    let savepoint = match txn.begin().await {
        Ok(sp) => sp,
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: could not open a savepoint to retire stranded compiles");
            return 0;
        }
    };
    let retired = savepoint
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RETIRE_UNCLAIMED_WORKING_COPY_SQL,
            [
                workspace_id.to_string().into(),
                WORKING_COPY_UNCLAIMED_BOUND_SECS.into(),
                WORKING_COPY_UNCLAIMED_REASON.into(),
            ],
        ))
        .await
        .map(|row| {
            row.and_then(|r| r.try_get::<i64>("", "n").ok())
                .unwrap_or(0)
        });
    match retired {
        Ok(n) => match savepoint.commit().await {
            Ok(()) => {
                if n > 0 {
                    tracing::warn!(%workspace_id, retired = n, "lazy compile: retired working-copy compiles no node could claim");
                }
                n as u64
            }
            Err(e) => {
                tracing::warn!(?e, %workspace_id, "lazy compile: releasing the retire savepoint failed");
                0
            }
        },
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: retiring stranded compiles failed");
            if let Err(e) = savepoint.rollback().await {
                tracing::warn!(?e, %workspace_id, "lazy compile: rolling back the retire savepoint failed");
            }
            0
        }
    }
}

/// Whether a working-copy compile was retired here within
/// [`RETIRED_WORKING_COPY_MEMORY_SECS`] after waiting out the whole unclaimed
/// bound. A failed read answers `false`: the self-heal then queues, as before.
///
/// That holds only because the read runs in a savepoint (a nested `begin()`),
/// as the retire does: a failed statement aborts a Postgres transaction, and
/// on the caller's advisory-locked txn it would take the enqueue down with it.
async fn recently_retired_working_copy(
    txn: &sea_orm::DatabaseTransaction,
    workspace_id: Uuid,
) -> bool {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
    let savepoint = match txn.begin().await {
        Ok(sp) => sp,
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: could not open a savepoint to read retired compiles");
            return false;
        }
    };
    let read = savepoint
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            RECENTLY_RETIRED_WORKING_COPY_SQL,
            [
                workspace_id.to_string().into(),
                RETIRED_WORKING_COPY_MEMORY_SECS.into(),
                WORKING_COPY_UNCLAIMED_BOUND_SECS.into(),
            ],
        ))
        .await;
    match read {
        Ok(row) => {
            if let Err(e) = savepoint.commit().await {
                tracing::warn!(?e, %workspace_id, "lazy compile: releasing the read savepoint failed");
            }
            row.is_some()
        }
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "lazy compile: reading recently retired compiles failed");
            if let Err(e) = savepoint.rollback().await {
                tracing::warn!(?e, %workspace_id, "lazy compile: rolling back the read savepoint failed");
            }
            false
        }
    }
}

#[cfg(test)]
#[path = "enqueue_tests.rs"]
mod tests;
