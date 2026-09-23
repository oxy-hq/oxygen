//! `airway_pipeline_leases` — single-flight lease for airway pipeline runs.
//!
//! At most one *active* run per `(workspace_id, pipeline_name)`. See the
//! `CreatePipelineLeases` migration for why overlapping runs are incorrect
//! rather than merely wasteful.
//!
//! # Why a lease and not an in-process gate
//!
//! Oxy runs multiple `oxy-serve` and `oxy-worker` replicas, so a `static`
//! mutex only serializes runs that happen to land on the same pod. The
//! external airway engine already learned this the expensive way: its
//! `COMPACTION_GATE` is process-local and justifies itself with "the airhouse
//! data plane is single-writer", which stopped being true when the data plane
//! scaled past one replica.
//!
//! # Why acquisition is one statement
//!
//! `SELECT … then INSERT` leaves a window in which two replicas both observe
//! "no active run" and both proceed. Acquisition is therefore a single
//! `INSERT … ON CONFLICT DO UPDATE … WHERE expires_at < now()` whose
//! `RETURNING` tells the caller whether *it* won — the database resolves the
//! race, not application code.

use sea_orm::entity::prelude::*;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement,
};

/// Rows are keyed by workspace **and** pipeline name: `pipeline_name` comes
/// from the `.airway.yml` and is not globally unique, so two tenants can both
/// ship `restaurant_analytics` and neither may gate the other.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "airway_pipeline_leases")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub workspace_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub pipeline_name: String,
    /// Run currently holding the lease.
    pub run_id: String,
    pub acquired_at: ChronoDateTimeUtc,
    /// Crash backstop. A worker that dies mid-run never releases, so the lease
    /// must be able to lapse on its own.
    pub expires_at: ChronoDateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// How long a lease stays valid without being released.
///
/// This is a **backstop for crashed workers**, not a run-duration budget, so
/// it is deliberately generous: expiring mid-run would re-admit exactly the
/// concurrency the lease exists to prevent, and a slow Toast backfill can run
/// for hours. The cost of erring long is a pipeline that stays blocked after a
/// hard crash until the lease lapses; the cost of erring short is silent
/// duplicate rows. Prefer the former — but only once it is genuinely clearable;
/// see the coverage note below.
///
/// ## Which terminal paths release, and which rely on this TTL
///
/// Released explicitly:
/// - normal completion and failure (tail of the worker's `drive`)
/// - worker panic / abort (the JoinHandle watcher — `drive`'s own release sits
///   inside the future that just panicked, so it needs its own call)
/// - cancel with no live channel (the HTTP handler, via the pipeline facade —
///   a queued-but-unclaimed run, or a cancel landing on a different replica
///   than the one driving)
///
/// **NOT released — covered only by this TTL:**
/// - **reaper dead-letter**, when a task exhausts its attempts.
///   `agentic-runtime` deliberately carries no dependency on domain crates (it
///   inlines the string `"airway"` rather than importing `SOURCE_TYPE`), so the
///   generic reaper cannot touch `airway_pipeline_leases` without leaking a
///   domain table into the runtime layer. Closing it properly needs a
///   domain-registered terminal hook in the runtime — a larger change than this
///   lease.
///
/// That gap is why a manual release command is a **prerequisite** for shipping a
/// TTL this long, not a nice-to-have: without one, a dead-lettered run blocks
/// its pipeline for six hours with no recourse but raw SQL against prod.
/// TTL is the BACKSTOP, not the reclaim policy.
///
/// Acquisition also reclaims a lease whose holder has reached a TERMINAL
/// status — `done`, `failed`, `cancelled`, `timed_out`.
///
/// Deliberately `EXISTS (… terminal)` and not `NOT EXISTS (… live)`: a MISSING
/// run row must not make a lease stealable. Callers may acquire before the run
/// row is written, and treating absence as death would let a concurrent
/// acquirer take a lease out from under one of them. Absence falls back to the
/// TTL, which is what a backstop is for. Without that, a holder
/// that died — or that was force-failed by crash recovery — blocked its
/// pipeline for the full six hours even though nothing was running, which is
/// how a wedged `quickbooks_financials` lease was found on dev held by a run
/// that had already failed.
///
/// The predicate is widened on the EXISTING atomic CAS rather than added as a
/// separate reaper. A second mechanism racing the first over the same rows is
/// how this gets subtly wrong; the `INSERT … ON CONFLICT DO UPDATE … WHERE`
/// already serializes two racers correctly, so it only needed a bigger WHERE.
pub const LEASE_TTL_SECS: i64 = 6 * 60 * 60;

/// Outcome of [`try_acquire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAcquisition {
    /// Caller holds the lease and may start the run.
    Acquired,
    /// Another run holds it. Carries that run's id so callers can point at
    /// what is already in flight rather than reporting a bare "busy".
    Held {
        run_id: String,
        expires_at: ChronoDateTimeUtc,
    },
}

#[derive(Debug, FromQueryResult)]
struct AcquireRow {
    run_id: String,
    expires_at: ChronoDateTimeUtc,
}

/// Try to take the single-flight lease for `(workspace_id, pipeline_name)`.
///
/// Atomic: the `ON CONFLICT` target is the table's composite primary key, and
/// the `DO UPDATE … WHERE` predicate only steals a lease that has already
/// lapsed. `RETURNING` always yields exactly one row — the winner's — so
/// comparing its `run_id` to ours is what distinguishes "acquired" from
/// "someone else holds it".
async fn try_acquire_once<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    pipeline_name: &str,
    run_id: &str,
    ttl_secs: i64,
) -> Result<LeaseAcquisition, DbErr> {
    // `now()` is evaluated server-side so a replica with a skewed clock cannot
    // mint a lease that outlives (or predates) everyone else's view of time.
    let sql = r#"
        INSERT INTO airway_pipeline_leases
            (workspace_id, pipeline_name, run_id, acquired_at, expires_at)
        VALUES ($1, $2, $3, now(), now() + make_interval(secs => $4))
        ON CONFLICT (workspace_id, pipeline_name) DO UPDATE
            SET run_id      = EXCLUDED.run_id,
                acquired_at = EXCLUDED.acquired_at,
                expires_at  = EXCLUDED.expires_at
            WHERE airway_pipeline_leases.expires_at < now()
               -- RE-ENTRANT: a run re-taking its OWN lease is not contention.
               -- `retry_airway` and the backfill re-drive both re-acquire under
               -- the original run_id on purpose, and the executor acquires
               -- again at claim. Without this the run is `Held` by itself, the
               -- executor defers, and the task loops until it dead-letters.
               -- Re-taking also refreshes `expires_at`, which a re-drive wants.
               OR airway_pipeline_leases.run_id = EXCLUDED.run_id
               OR EXISTS (
                    SELECT 1 FROM agentic_runs r
                    WHERE r.id = airway_pipeline_leases.run_id
                      AND r.task_status IN ('done', 'failed', 'cancelled', 'timed_out')
               )
        RETURNING run_id, expires_at
    "#;

    let row = AcquireRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [
            workspace_id.into(),
            pipeline_name.into(),
            run_id.into(),
            (ttl_secs as f64).into(),
        ],
    ))
    .one(db)
    .await?;

    match row {
        // We won the insert (or stole a lapsed lease).
        Some(r) if r.run_id == run_id => Ok(LeaseAcquisition::Acquired),
        // Someone else's lease is live: `DO UPDATE` was skipped by the WHERE,
        // so RETURNING gave us the incumbent row.
        Some(r) => Ok(LeaseAcquisition::Held {
            run_id: r.run_id,
            expires_at: r.expires_at,
        }),
        // Postgres suppresses RETURNING when DO UPDATE's WHERE filters the
        // row out, so `None` also means "held" — we just don't know by whom.
        // Read it back rather than reporting a lease-free state we did not
        // observe.
        None => {
            let held = Entity::find_by_id((workspace_id, pipeline_name.to_string()))
                .one(db)
                .await?;
            Ok(match held {
                Some(m) => LeaseAcquisition::Held {
                    run_id: m.run_id,
                    expires_at: m.expires_at,
                },
                // Raced with a release between the upsert and this read: the
                // lease is demonstrably free. Signalled with an empty `run_id`
                // for `try_acquire` to retry on — never returned to a caller.
                None => LeaseAcquisition::Held {
                    run_id: String::new(),
                    expires_at: chrono::Utc::now(),
                },
            })
        }
    }
}

/// The holder [`try_acquire`] reports when it lost the acquire race every time
/// and the read-back found no row. It names no run and no reset — only that
/// something contended — so a caller rendering it must not claim either.
pub const CONTENDED_HOLDER: &str = "<unknown: lost the acquire race repeatedly>";

/// Try to take the single-flight lease for `(workspace_id, pipeline_name)`.
///
/// Retries the one race the upsert cannot resolve in a single statement: when
/// `DO UPDATE`'s `WHERE` filters the row out, Postgres suppresses `RETURNING`,
/// and if a concurrent release then removes the row before the read-back, there
/// is no holder to report *and* no lease held. Earlier this returned
/// `Held { run_id: "" }`, which was never a real state — and the empty string
/// escaped: the HTTP 409 carried `"run_id": ""`, and worse, `run_schedule_now`
/// returned `Ok("")`, so "Run now" reported SUCCESS and sent the UI to a
/// nonexistent run while nothing had been started.
///
/// That branch is the one case where the lease is provably free, so retrying is
/// exactly right. Bounded at three attempts: the race is a narrow window
/// between two statements, so if it loses three times something else is wrong
/// and blocking forever would be worse than reporting contention.
pub async fn try_acquire<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    pipeline_name: &str,
    run_id: &str,
    ttl_secs: i64,
) -> Result<LeaseAcquisition, DbErr> {
    const ATTEMPTS: usize = 3;
    let mut last = LeaseAcquisition::Acquired;
    for _ in 0..ATTEMPTS {
        last = try_acquire_once(db, workspace_id, pipeline_name, run_id, ttl_secs).await?;
        match &last {
            // Only the sentinel retries; a real holder is a real answer.
            LeaseAcquisition::Held { run_id, .. } if run_id.is_empty() => continue,
            _ => return Ok(last),
        }
    }
    // Exhausted. `last` still carries the empty-`run_id` sentinel here, and
    // that was the whole bug: it reaches the 409 body as `"run_id": ""` and
    // makes `run_schedule_now` return `Ok("")`, so "Run now" reports SUCCESS
    // and navigates to a run that does not exist. Retrying bounded it but did
    // not stop it escaping on exhaustion.
    //
    // Read the row back one final time; if a holder is there, name it. If not,
    // the lease is genuinely free and losing three races means something is
    // wrong with the acquire path — say so in the holder field rather than
    // handing a caller an empty string it will render as a link.
    if matches!(&last, LeaseAcquisition::Held { run_id, .. } if run_id.is_empty()) {
        if let Some(m) = Entity::find_by_id((workspace_id, pipeline_name.to_string()))
            .one(db)
            .await?
        {
            last = LeaseAcquisition::Held {
                run_id: m.run_id,
                expires_at: m.expires_at,
            };
        } else {
            last = LeaseAcquisition::Held {
                run_id: CONTENDED_HOLDER.to_string(),
                expires_at: chrono::Utc::now(),
            };
        }
    }
    tracing::warn!(
        pipeline = %pipeline_name,
        "airway lease acquire lost the release race {ATTEMPTS} times; reporting contention"
    );
    Ok(last)
}

/// Release whatever lease `run_id` holds, without needing to know which
/// pipeline or workspace it was for.
///
/// This is the release the *worker* uses: at task-completion it has the run id
/// in hand but not the workspace, and threading those through the task spec
/// purely to unlock would widen the contract for no gain. `run_id` is a v4
/// UUID, so matching on it alone is exact — and the predicate still guarantees
/// a stale run cannot free a successor's lease, because a taken-over lease no
/// longer carries the old run's id.
pub async fn release_by_run<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM airway_pipeline_leases WHERE run_id = $1",
        [run_id.into()],
    ))
    .await?;
    Ok(())
}

/// Release the lease iff `run_id` still holds it, reporting rows removed.
///
/// The operator-facing CLI needs the count to distinguish "the holder you were
/// shown released on its own" (0) from "released it" (1) — and it needs the
/// `run_id` guard, because the confirmation prompt waits on human latency while
/// airway pipelines are cron-driven. An unguarded, pipeline-scoped delete would
/// let a `y` typed after the holder finished remove the lease of whichever run
/// the next tick started, re-admitting exactly the concurrency this table
/// prevents.
pub async fn release_counted<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    pipeline_name: &str,
    run_id: &str,
) -> Result<u64, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"DELETE FROM airway_pipeline_leases
           WHERE workspace_id = $1 AND pipeline_name = $2 AND run_id = $3"#,
            [workspace_id.into(), pipeline_name.into(), run_id.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Every lease currently held in `workspace_id`, newest first.
///
/// The operator surface for "why won't this pipeline start?". Includes lapsed
/// leases (`expires_at` in the past) rather than filtering them out: a lapsed
/// row is still what a reader sees in the table, and hiding it would make the
/// listing disagree with the DB for no gain.
pub async fn list_for_workspace<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
) -> Result<Vec<Model>, DbErr> {
    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(Column::AcquiredAt)
        .all(db)
        .await
}

/// Force-release a pipeline's lease regardless of which run holds it.
///
/// Deliberately NOT guarded on `run_id`, unlike [`release_counted`] — the whole point
/// is to recover from a holder that will never release itself: a dead-lettered
/// run (the reaper cannot free it; see the TTL note above), or a `Ctrl-C`'d
/// `oxy airway run`, which otherwise leaves the pipeline unrunnable for the
/// rest of the TTL.
///
/// Returns rows removed, so a caller can tell "cleared it" from "there was
/// nothing to clear" rather than reporting success either way.
///
/// The risk this accepts is real and the caller must own it: if the holder is
/// genuinely still running, releasing lets a second run start alongside it —
/// exactly the overlap the lease prevents. Callers should show the holder and
/// require confirmation.
pub async fn force_release<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<u64, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM airway_pipeline_leases \
               WHERE workspace_id = $1 AND pipeline_name = $2",
            [workspace_id.into(), pipeline_name.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}
