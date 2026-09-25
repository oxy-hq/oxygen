//! Generic, resumable chunked backfill for airway pipelines.
//!
//! A backfill of a long `[start, end)` range is split into period-aligned
//! chunks (month / week / day). Each chunk is run as a bounded airway backfill
//! (`backfill_from`/`backfill_to`) and its outcome is recorded in a
//! `backfill_checkpoint` row, so a crashed or cancelled backfill **resumes** by
//! skipping chunks already `done`, and the same rows answer "what period is
//! missing?" (any expected chunk that isn't `done`).
//!
//! This module is pipeline-agnostic: the chunk enumeration here is the reusable
//! primitive; the orchestrator drives any `*.airway.yml` whose source honours a
//! `[backfill_from, backfill_to)` window (Toast, QuickBooks, …).

use std::sync::Arc;

use agentic_runtime::crud;
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::router::NoopTaskRouter;
use agentic_runtime::state::RuntimeState;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Months, TimeZone, Utc};
use entity::backfill_checkpoints::{
    ActiveModel as CpActive, Column as CpCol, Entity as Checkpoint, Model as CpModel,
};
use entity::backfill_ranges::{
    ActiveModel as RangeActive, Column as RangeCol, Entity as BackfillRange,
};
use futures::stream::StreamExt;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::airway_run::{
    AirwayRunError, StartAirwayRequest, spawn_airway_run_drive, start_airway_run,
};
use crate::platform::PlatformContext;
use crate::{AIRWAY_SOURCE_TYPE, TaskScope, WorkflowWorkspaceContext, airway_event_handler};

/// Granularity of a backfill chunk. Pick the coarsest that keeps a single chunk
/// a tractable unit of work + a useful resume/coverage granularity — month is a
/// good default (it also aligns with `year(business_date)`-style partitioning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkGranularity {
    Day,
    Week,
    Month,
}

impl ChunkGranularity {
    /// Parse from a CLI/config string. `None` for unknown.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "day" | "daily" => Some(Self::Day),
            "week" | "weekly" => Some(Self::Week),
            "month" | "monthly" => Some(Self::Month),
            _ => None,
        }
    }

    /// Canonical lowercase name, for persisting on a `backfill_ranges` row.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// Start of the period containing `t`, in UTC: month → 1st 00:00, week →
    /// Monday 00:00, day → 00:00.
    fn period_start(self, t: DateTime<Utc>) -> DateTime<Utc> {
        let d = t.date_naive();
        let aligned = match self {
            Self::Day => d,
            Self::Week => d - Duration::days(i64::from(d.weekday().num_days_from_monday())),
            Self::Month => d.with_day(1).expect("day 1 is always valid"),
        };
        Utc.from_utc_datetime(&aligned.and_hms_opt(0, 0, 0).expect("midnight is valid"))
    }

    /// The next period boundary at/after the period start of `t`.
    fn next_boundary(self, t: DateTime<Utc>) -> DateTime<Utc> {
        let ps = self.period_start(t);
        match self {
            Self::Day => ps + Duration::days(1),
            Self::Week => ps + Duration::weeks(1),
            Self::Month => ps + Months::new(1),
        }
    }
}

/// One half-open `[start, end)` backfill window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Split `[start, end)` into period-aligned half-open chunks. The first chunk
/// starts at `start` (not its period boundary) and the last ends at `end`, so
/// the chunks' union is exactly `[start, end)` with no gaps or overlaps. Empty
/// when `start >= end`. The interior boundaries are aligned (e.g. the 1st of each
/// month), which makes resume/coverage line up with calendar periods.
pub fn enumerate_chunks(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    granularity: ChunkGranularity,
) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let chunk_end = granularity.next_boundary(cursor).min(end);
        chunks.push(Chunk {
            start: cursor,
            end: chunk_end,
        });
        cursor = chunk_end;
    }
    chunks
}

// ── Chunked-backfill orchestration ─────────────────────────────────────────
//
// A backfill of a long `[from, to)` range is split into chunks (above); each
// chunk runs as a bounded airway backfill and its outcome is recorded in a
// `backfill_checkpoints` row, so a crashed/cancelled backfill resumes by
// skipping `done` chunks and the same rows answer "what period is missing?".
// Shared by the `oxy airway backfill` CLI and the HTTP
// `POST /agentic-airway/chunked-backfill` handler.

/// Hard ceiling on how long one chunk may run before we give up waiting and
/// mark it `failed`, so a wedged worker / lost driver lease can't hang the
/// driver forever — the chunk stays resumable instead.
const MAX_CHUNK_WAIT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

/// How long `get_run` may fail CONTINUOUSLY before a chunk gives up waiting.
///
/// Wall clock, not a poll count: a failing read returns after the pool's
/// `ACQUIRE_TIMEOUT` (30s), not after the 200ms poll interval, so a count
/// bounds nothing. 2 minutes rides out a blip or a managed-Postgres failover
/// (typically 30–120s) while still failing a real outage well inside
/// [`MAX_CHUNK_WAIT`]. Reset by any successful read, so intermittent errors
/// never accumulate to it.
const MAX_POLL_FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);

fn build_event_registry() -> EventRegistry {
    let mut registry = EventRegistry::new();
    registry.register(AIRWAY_SOURCE_TYPE, airway_event_handler());
    registry
}

fn is_terminal(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
    )
}

/// Outcome of one chunk run, derived from the run's **events** (not just its
/// runtime `task_status`): airway finishes a run with skipped resources as
/// `task_status = "done"` + a `LoadCompleted` event, so keying success on
/// `task_status` alone would silently checkpoint a partial load as fully done.
struct ChunkOutcome {
    run_id: String,
    /// `done` (clean) | `completed_with_errors` (loaded, ≥1 resource failed)
    /// | `failed`/`cancelled`/`timed_out` (not complete). Only `done` is treated
    /// as complete on resume; the rest re-run and show in coverage as missing.
    status: String,
    /// Total rows written across all tables (`LoadCompleted.rows_loaded`), or
    /// `None` if the run didn't load.
    row_count: Option<i64>,
    /// Failed-resource list or error message, surfaced into the checkpoint.
    detail: Option<String>,
}

/// Classify a terminated run by reading its event stream: a `pipeline_error`
/// (or a non-`done` runtime status) is a hard failure; any `resource_failed` /
/// `table_load_failed` makes it `completed_with_errors`; otherwise `done`. Also
/// sums `LoadCompleted.rows_loaded` for the checkpoint's `row_count`.
async fn classify_run_outcome(
    db: &DatabaseConnection,
    run_id: &str,
    task_status: &str,
    since_seq: i64,
) -> (String, Option<i64>, Option<String>) {
    let mut processor = build_event_registry().stream_processor(AIRWAY_SOURCE_TYPE);
    let mut failed_resources: Vec<String> = Vec::new();
    let mut hard_error: Option<String> = None;
    let mut rows_loaded: i64 = 0;
    let mut saw_load_completed = false;
    // `since_seq` isolates THIS attempt's events: on a reset-in-place re-drive the
    // prior failed attempt's events are still on the run, so reading from `-1`
    // would mis-classify a successful retry as failed.
    for row in crud::get_events_after(db, run_id, since_seq)
        .await
        .unwrap_or_default()
    {
        for (event_type, payload) in processor.process(&row.event_type, &row.payload) {
            match event_type.as_str() {
                // Both "skipped a table, run still finishes done" events: a
                // ResourceFailed (extract path) AND a TableLoadFailed (streaming
                // load path). Either makes the chunk completed_with_errors — both
                // carry a `table` field, so the same extraction works.
                "resource_failed" | "table_load_failed" => failed_resources.push(
                    payload
                        .get("table")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                ),
                "pipeline_error" => {
                    hard_error = Some(
                        payload
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                    )
                }
                "load_completed" => {
                    saw_load_completed = true;
                    if let Some(rl) = payload.get("rows_loaded").and_then(Value::as_object) {
                        rows_loaded += rl.values().filter_map(Value::as_u64).sum::<u64>() as i64;
                    }
                }
                _ => {}
            }
        }
    }
    let row_count = saw_load_completed.then_some(rows_loaded);
    if task_status != "done" {
        // cancelled / timed_out / failed — surface the pipeline_error if we have it.
        return (task_status.to_string(), row_count, hard_error);
    }
    if let Some(err) = hard_error {
        return ("failed".to_string(), row_count, Some(err));
    }
    if !failed_resources.is_empty() {
        return (
            "completed_with_errors".to_string(),
            row_count,
            Some(format!("resources failed: {}", failed_resources.join(", "))),
        );
    }
    ("done".to_string(), row_count, None)
}

/// The highest `agentic_run_events.seq` currently recorded for `run_id`, or `-1`
/// if none. Used as the classify watermark when re-driving a run in place so
/// only the new attempt's events are read.
async fn latest_event_seq(db: &DatabaseConnection, run_id: &str) -> i64 {
    crud::get_events_after(db, run_id, -1)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.seq)
        .max()
        .unwrap_or(-1)
}

/// Run one bounded `[from, to)` backfill of `pipeline_ref`, block until it
/// terminates (or `MAX_CHUNK_WAIT` elapses), and classify the outcome. Uses a
/// fresh `RuntimeState` + `NoopTaskRouter` per chunk so it's self-contained
/// (no shared orchestrator state required).
///
/// `existing_run_id`: when set (a chunk retry — the checkpoint already carries a
/// run_id), the run is re-driven **in place** — its queued task is revived, its
/// status flipped back to `running`, and `retry_count` bumped — keeping a stable
/// run_id (and, once the cursor lands, resuming instead of re-extracting).
/// Otherwise a fresh run is seeded.
/// Re-take the single-flight lease for a chunk's reset-in-place re-drive.
///
/// Returns `false` when another run holds it, so the caller seeds a fresh run
/// instead. That seed now simply enqueues: submit no longer refuses a contended
/// caller, and the contention is resolved at claim time like any other.
async fn reacquire_chunk_lease(
    db: &DatabaseConnection,
    platform: &Arc<dyn PlatformContext>,
    pipeline_ref: &str,
    run_id: &str,
) -> Result<bool, AirwayRunError> {
    use agentic_airway::extension::pipeline_lease::{
        LEASE_TTL_SECS, LeaseAcquisition, try_acquire,
    };
    // The chunk driver knows the ref, not the spec's `name`. The lease is keyed
    // by pipeline NAME, so read it off the run's own metadata — the same place
    // `start_airway_run` stamped it when the chunk was first seeded.
    let Some(name) = crud::get_run(db, run_id)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.metadata)
        .and_then(|m| {
            m.get("pipeline_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
    else {
        // Fail CLOSED. Returning `true` here reported "lease acquired" while
        // taking none, so the chunk re-drove unguarded — the single-flight hole
        // this function exists to close, reached by a metadata gap rather than
        // by contention. `false` sends the caller down the seed-fresh path,
        // where `start_airway_run` re-derives the name from the spec and takes
        // the lease properly.
        tracing::warn!(
            run_id,
            pipeline_ref,
            "backfill retry: no pipeline_name on the run; seeding fresh so the \
             lease is taken through the normal path"
        );
        return Ok(false);
    };
    match try_acquire(db, platform.workspace_id(), &name, run_id, LEASE_TTL_SECS).await? {
        LeaseAcquisition::Acquired => Ok(true),
        LeaseAcquisition::Held { run_id: holder, .. } => {
            tracing::info!(pipeline = %name, held_by = %holder,
                "backfill retry: lease held by another run; will seed fresh");
            Ok(false)
        }
    }
}

/// Outcome of a chunk's reset-in-place attempt. `StillLive` exists so the
/// "a worker holds this run" bail can return without releasing — routing it
/// through `Err` would hit the release-on-error arm and free that worker's
/// lease, which is the overlap this guard prevents.
enum Reuse {
    Reused((String, i64)),
    /// Task row gone, or the lease was never taken — safe to release + reseed.
    Reaped,
    /// Task is queued/claimed: a worker is driving it. Do not release.
    StillLive,
}

#[allow(clippy::too_many_arguments)]
async fn run_airway_window(
    db: &DatabaseConnection,
    platform: &Arc<dyn PlatformContext>,
    pipeline_ref: &str,
    variables: Option<Value>,
    resources: Vec<String>,
    backfill_from: String,
    backfill_to: String,
    existing_run_id: Option<&str>,
    // The chunk's checkpoint, so a freshly seeded run's id is persisted where
    // it is FIRST KNOWN rather than when the outcome returns. The caller only
    // learns the id when this function comes back, which can be up to
    // `MAX_CHUNK_WAIT` (2h) later — a deploy, pod restart or CLI `Ctrl-C` in
    // that window left a run that exists and holds the lease with
    // `cp.run_id = None`, and the next pass re-ingested the window. Recording
    // at seed time makes "seeded but unrecorded" one statement wide instead of
    // two hours. Same move as `RunIdWrite`: remove the state, not the paths
    // that reach it.
    cp: &CpModel,
) -> Result<ChunkOutcome, AirwayRunError> {
    // Reset-in-place re-drive of the chunk's prior run — but only if its queued
    // task still exists. `reset_task_to_queued` returns 0 rows when the task was
    // reaped/purged (e.g. resuming an old backfill) or is still live; re-driving
    // then would hang (the spawned worker has nothing to claim until
    // `MAX_CHUNK_WAIT`), so fall back to a fresh run instead.
    let reused = match existing_run_id {
        Some(rid) => {
            // `reset_task_to_queued` revives the task in place; 0 rows = it was
            // reaped, so there's nothing to re-drive → seed a fresh run below.
            // Re-take the single-flight lease before reviving the task. It was
            // deleted when the prior attempt terminalized, so a reset-in-place
            // re-drive would otherwise run unguarded — the same gap as the
            // `retry_airway` path. Keyed to the SAME run_id so the worker's
            // existing `release_by_run` still frees exactly this lease.
            //
            // On conflict, fall through to `None` (seed a fresh run below)
            // rather than erroring: the caller already treats a reap the same
            // way, and a chunk that can't reuse its run is not a chunk failure.
            let lease_ok = match reacquire_chunk_lease(db, platform, pipeline_ref, rid).await {
                Ok(ok) => ok,
                Err(e) => {
                    tracing::warn!(run_id = %rid, error = %e,
                        "backfill retry: lease re-acquire failed; seeding a fresh run");
                    false
                }
            };
            // Whether the reset actually revived the task. Split out from the
            // `&&` because the acquired-but-not-reused combination is a LEAK:
            // `lease_ok == true` with `revived == false` (the task was reaped)
            // seeds a fresh run below whose `start_airway_run` takes its own
            // lease, stranding this one under `rid` for the full 6h TTL — and
            // that fresh seed then fails against the very lease we just took,
            // so the chunk can never progress. Self-sustaining stall.
            // The `?` here is itself an exit under the lease — releasing only on
            // the acquired-but-not-revived path left a DB error leaking it.
            //
            // Same shape as `retry_airway`: one block with plain `?` inside and
            // a single release on the way out. `Some` = reused in place; `None`
            // = seed fresh. Every non-reuse exit — reaped, DB error, lease held
            // — releases exactly once, at the match below. Releasing per call
            // site is what left `reset_run_for_retry` leaking through two
            // rounds of fixes.
            let guarded = async {
                if !lease_ok {
                    return Ok::<Reuse, AirwayRunError>(Reuse::Reaped);
                }
                if crud::reset_task_to_queued(db, rid).await? == 0 {
                    // 0 rows is TWO states: the row is gone (reaped), or it is
                    // queued/claimed and a worker is still driving this run.
                    // This function's own doc says both out loud; treating them
                    // alike releases the lease and seeds fresh ALONGSIDE a live
                    // worker — two concurrent runs of one pipeline, via this
                    // guard's own path.
                    //
                    // Reachable on any chunk that hit MAX_CHUNK_WAIT: the
                    // timeout arm reports `failed` while the spawned drive keeps
                    // running (deliberately, so a wedged worker can't hang the
                    // driver), and the next Resume pass arrives here with the
                    // task still `claimed`.
                    // Fails CLOSED on a read error: guessing "reaped" wrongly
                    // releases a running worker's lease and starts a second
                    // run; guessing "live" wrongly costs one deferred chunk.
                    let live = match crud::get_queue_entry(db, rid).await {
                        Ok(entry) => entry.is_some(),
                        Err(e) => {
                            tracing::warn!(run_id = %rid, error = %e,
                                "backfill retry: queue-state read failed; assuming live");
                            true
                        }
                    };
                    if live {
                        tracing::warn!(
                            run_id = %rid,
                            "backfill retry: prior run is still queued/claimed — a worker \
                             is driving it; deferring this chunk instead of reseeding"
                        );
                        return Ok(Reuse::StillLive);
                    }
                    tracing::warn!(
                        run_id = %rid,
                        "backfill retry: prior run's task was reaped; seeding a fresh run"
                    );
                    return Ok(Reuse::Reaped);
                }
                // Drop the prior attempt's events + clear its terminal error so
                // the re-run shows clean; `since = -1` (no old events left).
                crud::delete_events_from_seq(db, rid, 0).await.ok();
                crud::reset_run_for_retry(db, rid).await?;
                if let Err(e) =
                    agentic_airway::extension::run_extension::increment_retry_count(db, rid).await
                {
                    tracing::warn!(run_id = %rid, error = %e, "backfill retry: retry_count bump failed");
                }
                Ok(Reuse::Reused((rid.to_string(), latest_event_seq(db, rid).await)))
            }
            .await;

            match guarded {
                Ok(Reuse::Reused(pair)) => Some(pair),
                Ok(Reuse::Reaped) => {
                    if lease_ok {
                        crate::airway_run::release_airway_lease(db, rid).await;
                    }
                    None
                }
                // A worker holds this run. Return WITHOUT releasing — the lease
                // is that worker's. Surfacing `AlreadyRunning` defers the chunk
                // (it stays `pending` for the next pass) rather than failing it.
                Ok(Reuse::StillLive) => {
                    return Err(AirwayRunError::AlreadyRunning {
                        pipeline_name: pipeline_ref.to_string(),
                        run_id: rid.to_string(),
                    });
                }
                Err(e) => {
                    if lease_ok {
                        crate::airway_run::release_airway_lease(db, rid).await;
                    }
                    return Err(e);
                }
            }
        }
        None => None,
    };

    let (run_id, since) = match reused {
        Some(pair) => pair,
        // First attempt, or a reaped re-drive — seed a fresh run.
        None => {
            let request = StartAirwayRequest {
                pipeline_ref: pipeline_ref.to_string(),
                variables,
                thread_id: None,
                // The range's scope, not an empty list. A backfill run is
                // run-scoped, so an unscoped chunk makes every SNAPSHOT
                // resource believe it has never run and pull its ordinary
                // daily snapshot — on every chunk, for a period Amazon serves
                // no historical form of.
                resources,
                schedule_id: None,
                trigger: Some("backfill".to_string()),
                logical_date: None,
                retry_of: None,
                backfill_from: Some(backfill_from),
                backfill_to: Some(backfill_to),
            };
            let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
            let workspace_id = platform.workspace_id();
            let rid = start_airway_run(
                db,
                workspace.as_ref(),
                request,
                TaskScope::Scoped,
                workspace_id,
            )
            .await?;
            // Persist immediately — before the drive is even spawned.
            // Best-effort: failing to record must not abandon a run that now
            // exists, and the outcome path records it again on return.
            if let Err(e) = checkpoint_record_run_id(db, cp, &rid).await {
                tracing::warn!(run_id = %rid, error = %e,
                    "backfill: could not record the seeded run on its checkpoint");
            }
            (rid, -1)
        }
    };

    let state = Arc::new(RuntimeState::new());
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.register(&run_id, mpsc::channel::<String>(1).0, cancel_tx);
    spawn_airway_run_drive(
        db.clone(),
        Arc::clone(&state),
        run_id.clone(),
        Arc::clone(platform),
        cancel_rx,
        Arc::new(NoopTaskRouter),
    );

    let deadline = tokio::time::Instant::now() + MAX_CHUNK_WAIT;
    // `None` while reads are succeeding; set on the first of a run of failures
    // so the tolerance is measured in wall clock, not iterations.
    let mut first_poll_failure: Option<tokio::time::Instant> = None;
    // `None` = nothing logged for the CURRENT failure run. Not a bare `u64`:
    // the first failure has `elapsed` ≈ 0, so its bucket is 0 — identical to a
    // zero initial value, which made the opening failure of a chunk silent and
    // absorbed every sub-30s blip without a trace. Reset alongside
    // `first_poll_failure` so every run logs its own first failure.
    let mut last_logged_failure_bucket: Option<u64> = None;
    loop {
        // Checked BEFORE the read, so it bounds the failure path too. It used
        // to live only in the `match observed` arm below, which a failing poll
        // never reaches — leaving the chunk (and, at concurrency 1, the range)
        // bounded solely by the retry budget.
        if tokio::time::Instant::now() >= deadline {
            // One last read before declaring failure: the run may have reached
            // a terminal status since the poll above, and checkpointing a
            // SUCCEEDED run `failed` would re-ingest a window already loaded.
            // Cheap — one query, only on the timeout path.
            if let Ok(Some(run)) = crud::get_run(db, &run_id).await
                && is_terminal(run.task_status.as_deref())
            {
                let task_status = run.task_status.unwrap_or_else(|| "done".into());
                let (status, row_count, detail) =
                    classify_run_outcome(db, &run_id, &task_status, since).await;
                tracing::info!(%run_id, %status,
                    "backfill: run reached a terminal status as the chunk deadline expired");
                return Ok(ChunkOutcome {
                    run_id,
                    status,
                    row_count,
                    detail,
                });
            }
            return Ok(ChunkOutcome {
                run_id,
                status: "failed".into(),
                row_count: None,
                detail: Some(format!(
                    "timed out after {}s waiting for a terminal status",
                    MAX_CHUNK_WAIT.as_secs()
                )),
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // Tolerant of a BLIP, not of an outage. This `?` used to be the one
        // fallible exit between seeding a run and returning its id, so a
        // transient `DbErr` aborted the wait while the spawned drive kept
        // going. But retrying unboundedly to the 2h deadline was worse than
        // what it replaced: a sustained outage stalled the chunk — and with
        // concurrency pinned to 1, the whole range — for two hours, where it
        // previously failed in 200ms and re-ran next pass. The `"failed"`
        // outcome could not even be persisted, since `checkpoint_set` needs the
        // same database.
        //
        // An ELAPSED WINDOW, reset by any successful read, separates the two:
        // blips are absorbed, a dead database gives up promptly.
        //
        // Deliberately not a poll count. A failing `get_run` does not return in
        // 200ms — it returns after the pool's ACQUIRE_TIMEOUT (30s), so N polls
        // is anywhere from 0.2N to 30N seconds and no count can bound wall
        // clock. A previous revision capped at 300 polls and claimed ~60s; the
        // real worst case was ~2.5h, past MAX_CHUNK_WAIT, which is the one
        // property the bound exists to provide.
        let observed = match crud::get_run(db, &run_id).await {
            Ok(v) => {
                first_poll_failure = None;
                last_logged_failure_bucket = None;
                v
            }
            Err(e) => {
                let since = *first_poll_failure.get_or_insert_with(tokio::time::Instant::now);
                let elapsed = since.elapsed();
                // First failure, then every ~30s of continuous failure.
                let bucket = elapsed.as_secs() / 30;
                if last_logged_failure_bucket != Some(bucket) {
                    last_logged_failure_bucket = Some(bucket);
                    tracing::warn!(
                        %run_id, error = %e, elapsed_secs = elapsed.as_secs(),
                        "backfill: run poll failed; retrying"
                    );
                }
                if elapsed >= MAX_POLL_FAILURE_WINDOW {
                    return Ok(ChunkOutcome {
                        run_id,
                        status: "failed".into(),
                        row_count: None,
                        detail: Some(format!(
                            "run poll failed continuously for {}s: {e}",
                            elapsed.as_secs()
                        )),
                    });
                }
                continue;
            }
        };
        match observed {
            Some(run) if is_terminal(run.task_status.as_deref()) => {
                let task_status = run.task_status.unwrap_or_else(|| "done".into());
                let (status, row_count, detail) =
                    classify_run_outcome(db, &run_id, &task_status, since).await;
                return Ok(ChunkOutcome {
                    run_id,
                    status,
                    row_count,
                    detail,
                });
            }
            // A vanished run row is an anomaly, not a success — fail it so it
            // retries on the next pass instead of being checkpointed `done`.
            None => {
                return Ok(ChunkOutcome {
                    run_id,
                    status: "failed".into(),
                    row_count: None,
                    detail: Some("run row vanished before reaching a terminal status".into()),
                });
            }
            _ => {}
        }
    }
}

/// Find-or-create the checkpoint row for a chunk within a range (the
/// resume/coverage key).
///
/// Non-atomic find-then-insert: fine for a single sequential driver, but two
/// concurrent drivers of the same range would race the unique
/// `(backfill_range_id, period_start, period_end)` index into a violation
/// rather than no-op'ing. Make it an `ON CONFLICT DO NOTHING` insert if this
/// ever runs from multiple drivers at once.
async fn checkpoint_upsert_pending(
    db: &DatabaseConnection,
    backfill_range_id: Uuid,
    workspace_id: Uuid,
    pipeline_ref: &str,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
) -> Result<CpModel, DbErr> {
    if let Some(existing) = Checkpoint::find()
        .filter(CpCol::BackfillRangeId.eq(backfill_range_id))
        .filter(CpCol::PeriodStart.eq(start))
        .filter(CpCol::PeriodEnd.eq(end))
        .one(db)
        .await?
    {
        return Ok(existing);
    }
    let now = Utc::now().fixed_offset();
    CpActive {
        id: Set(Uuid::new_v4()),
        workspace_id: Set(workspace_id),
        backfill_range_id: Set(backfill_range_id),
        pipeline_ref: Set(pipeline_ref.to_string()),
        period_start: Set(start),
        period_end: Set(end),
        status: Set("pending".to_string()),
        run_id: Set(None),
        row_count: Set(None),
        attempts: Set(0),
        error: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
}

/// How a checkpoint write should treat the `run_id` column.
///
/// Exists because the parameter used to be a bare `Option<String>`, where
/// `None` meant ERASE — and no caller ever meant that. Three of the five call
/// sites passed `None` to mean "I have nothing to say about run_id", silently
/// NULLing the link to a live run; two rounds of review each caught one of
/// them. Making "leave it alone" spellable is what stops the next caller
/// reintroducing it.
enum RunIdWrite {
    /// Leave the stored `run_id` untouched.
    Keep,
    /// Overwrite it, including with `None` when the link is genuinely gone.
    Set(Option<String>),
}

/// Update a checkpoint's status, `error` and `row_count` together.
///
/// All three are written on EVERY call — `error: None` and `row_count: None`
/// mean "clear it", not "leave it". That suits all current callers: each marks
/// a state transition where the previous attempt's error and row count are
/// stale by definition. It does NOT suit a caller that wants to touch one
/// column; that caller wants [`checkpoint_record_run_id`], or a `Keep`/`Set`
/// treatment for the field in question the way [`RunIdWrite`] gives `run_id`.
///
/// `run_id` is the exception precisely because it once behaved this way and
/// was wrong for it — see [`RunIdWrite`]. Reviewers have flagged the remaining
/// three as the same hazard three times; they are left in-band deliberately,
/// because no caller today means anything else, and an enum per column with a
/// single variant in use is harder to read than this note. Add the enum when a
/// caller needs it, not before.
async fn checkpoint_set(
    db: &DatabaseConnection,
    cp: &CpModel,
    status: &str,
    run_id: RunIdWrite,
    error: Option<String>,
    row_count: Option<i64>,
    bump_attempts: bool,
) -> Result<(), DbErr> {
    let mut active: CpActive = cp.clone().into();
    active.status = Set(status.to_string());
    // `Keep` leaves the column out of the update entirely — an unset
    // ActiveValue is not written, which is the whole point.
    if let RunIdWrite::Set(v) = run_id {
        active.run_id = Set(v);
    }
    active.error = Set(error);
    active.row_count = Set(row_count);
    active.updated_at = Set(Utc::now().fixed_offset());
    if bump_attempts {
        active.attempts = Set(cp.attempts + 1);
    }
    active.update(db).await?;
    Ok(())
}

/// Record the run a chunk was just seeded with, touching **only** `run_id`.
///
/// Deliberately not `checkpoint_set`: that helper restates `status`, `error`
/// and `row_count` on every call, and the only model available here is the one
/// read before the caller wrote `"running"`. Passing `&cp.status` back would
/// revert the row to its pre-attempt status — `pending` on the normal path,
/// `failed` on a re-run — for the entire time the chunk runs, contradicting
/// the `pending` vs `running` split this module documents below. Restating a
/// stale `error` alongside it would be incoherent in the same way.
///
/// This write means one thing, so it writes one column.
async fn checkpoint_record_run_id(
    db: &DatabaseConnection,
    cp: &CpModel,
    run_id: &str,
) -> Result<(), DbErr> {
    let mut active: CpActive = cp.clone().into();
    active.run_id = Set(Some(run_id.to_string()));
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(db).await?;
    Ok(())
}

/// The disposition of one chunk, for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkDisposition {
    /// Already `done` on a prior pass — skipped.
    Resumed,
    /// Ran clean.
    Done,
    /// Loaded but ≥1 resource failed (`completed_with_errors`).
    Degraded,
    /// Hard failure / timeout / cancelled.
    Failed,
    /// Not attempted: the pipeline's single-flight lease was held by another
    /// run. Distinct from `Failed` on purpose — nothing went wrong, the chunk
    /// simply has to wait, and it stays `pending` so the next pass takes it.
    /// Folding this into `Failed` would put red chunks in coverage for a
    /// pipeline behaving exactly as designed.
    Deferred,
}

/// One progress tick, emitted per chunk as a chunked backfill proceeds so a
/// caller (the CLI) can surface it. `note` carries the failure detail.
pub struct ChunkProgress {
    pub label: String,
    pub disposition: ChunkDisposition,
    pub note: Option<String>,
}

/// Tally of a chunked-backfill pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BackfillSummary {
    pub total: usize,
    pub done: usize,
    pub resumed: usize,
    pub degraded: usize,
    pub failed: usize,
    /// Chunks not attempted because the pipeline's single-flight lease was
    /// held. Counted separately from `failed` so a caller can tell "needs
    /// investigation" from "run it again in a minute".
    pub deferred: usize,
}

/// Run one already-checkpointed chunk to completion: skip if `done`, else mark
/// `running`, drive the bounded window, and record the outcome. Returns the
/// progress tick for the driver to fold into the summary. Each chunk touches
/// only its own `backfill_checkpoints` row (distinct `(range, period)`), so this
/// is safe to run concurrently with other chunks of the same range.
#[allow(clippy::too_many_arguments)]
async fn run_one_chunk(
    db: DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    backfill_range_id: Uuid,
    pipeline_ref: String,
    variables: Option<Value>,
    resources: Vec<String>,
    chunk: Chunk,
) -> Result<ChunkProgress, AirwayRunError> {
    let label = format!("{} → {}", chunk.start.date_naive(), chunk.end.date_naive());
    let (ps, pe) = (chunk.start.fixed_offset(), chunk.end.fixed_offset());
    let workspace_id = platform.workspace_id();
    // The row was pre-created by the driver; find-or-create is idempotent.
    let cp = checkpoint_upsert_pending(&db, backfill_range_id, workspace_id, &pipeline_ref, ps, pe)
        .await?;
    if cp.status == "done" {
        return Ok(ChunkProgress {
            label,
            disposition: ChunkDisposition::Resumed,
            note: None,
        });
    }
    checkpoint_set(&db, &cp, "running", RunIdWrite::Keep, None, None, true).await?;
    // Re-drive the chunk's prior run in place if it already has one (a retry);
    // otherwise seed a fresh run. Keeps the checkpoint's run_id stable.
    let (disposition, note) = match run_airway_window(
        &db,
        &platform,
        &pipeline_ref,
        variables,
        resources,
        chunk.start.to_rfc3339(),
        chunk.end.to_rfc3339(),
        cp.run_id.as_deref(),
        &cp,
    )
    .await
    {
        Ok(o) if o.status == "done" => {
            checkpoint_set(
                &db,
                &cp,
                "done",
                RunIdWrite::Set(Some(o.run_id)),
                None,
                o.row_count,
                false,
            )
            .await?;
            (ChunkDisposition::Done, None)
        }
        Ok(o) => {
            // completed_with_errors / failed / cancelled / timed_out — NOT
            // `done`, so it re-runs next pass and shows in coverage.
            let note = o.detail.clone().unwrap_or_else(|| o.status.clone());
            let disposition = if o.status == "completed_with_errors" {
                ChunkDisposition::Degraded
            } else {
                ChunkDisposition::Failed
            };
            checkpoint_set(
                &db,
                &cp,
                &o.status,
                RunIdWrite::Set(Some(o.run_id)),
                o.detail,
                o.row_count,
                false,
            )
            .await?;
            (disposition, Some(note))
        }
        // A chunk whose PRIOR run is still being driven is not a failure.
        //
        // Lease contention no longer reaches here: submit coalesces and the
        // executor defers at claim, so `start_airway_run` cannot return
        // `AlreadyRunning` any more. The one remaining source is
        // `Reuse::StillLive` — a re-drive of a chunk whose earlier run is
        // still queued or claimed by a worker. Recording that as `failed`
        // would put a red chunk in coverage for a pipeline working exactly as
        // designed, and an operator reading coverage could not tell it from a
        // real load error. Left `pending` instead, so the next pass picks it
        // up — the same treatment a never-started chunk gets, which is what
        // this is.
        Err(AirwayRunError::AlreadyRunning {
            pipeline_name,
            run_id: holder,
        }) => {
            // Two distinct causes reach here, and the note says which:
            //   * this chunk's OWN prior run is still queued/claimed (StillLive)
            //   * a different run holds the pipeline's lease
            // Only the first names this chunk's run id.
            let mine = cp.run_id.as_deref() == Some(holder.as_str());
            let note = if mine {
                format!("deferred: this chunk's run {holder} is still active; stays pending")
            } else {
                format!("deferred: `{pipeline_name}` had another run in flight ({holder})")
            };
            // PRESERVE the run_id. `checkpoint_set` assigns it unconditionally,
            // so passing `None` erased the link to the still-live run — and the
            // next pass would then seed a FRESH run instead of recognising its
            // own, which is the reseed-alongside-a-live-worker this deferral
            // exists to prevent.
            checkpoint_set(
                &db,
                &cp,
                "pending",
                RunIdWrite::Keep,
                Some(note.clone()),
                None,
                false,
            )
            .await?;
            (ChunkDisposition::Deferred, Some(note))
        }
        // Same reasoning as `AlreadyRunning` directly above, for a different
        // cause: this node could not resolve the pipeline's YAML — a
        // compile-boundary blip, or a revision still compiling. Backfill drives
        // on the same stateless fleet, so this is reachable by exactly the
        // deploy the compile boundary exists for. Recording it `failed` would
        // leave a permanently red chunk that `rollup_range_status` folds into a
        // failed range, demanding an operator re-run for a node condition that
        // clears itself in seconds.
        // Disposition decided by `chunk_action_for_error` — see there for why
        // `Unavailable` is not a red chunk.
        Err(e) => {
            let action = chunk_action_for_error(&e);
            checkpoint_set(
                &db,
                &cp,
                action.status,
                // `Keep` for every arm here, but NOT for the reason the
                // `AlreadyRunning` arm above gives. There, `Keep` preserves
                // the link to a run that is still live. Here nothing was
                // seeded — `start_airway_run` fails before
                // `checkpoint_record_run_id` — so `Keep` instead leaves a PRIOR
                // attempt's id in place for the reuse branch to recognise.
                RunIdWrite::Keep,
                Some(action.note.clone()),
                None,
                false,
            )
            .await?;
            (action.disposition, Some(action.note))
        }
    };
    Ok(ChunkProgress {
        label,
        disposition,
        note,
    })
}

/// How a chunk records a failed submit: the checkpoint status, the coverage
/// disposition, and the note an operator reads.
struct ChunkFailureAction {
    status: &'static str,
    disposition: ChunkDisposition,
    note: String,
}

/// Decide a failed submit's disposition.
///
/// Free function so the choice is assertable without a database — this is
/// coverage-visible state, and the difference between `pending`/`Deferred` and
/// `failed`/`Failed` is the difference between a chunk the next pass picks up
/// and one an operator has to re-run by hand.
fn chunk_action_for_error(e: &AirwayRunError) -> ChunkFailureAction {
    match e {
        // The node could not resolve the pipeline's YAML: a compile-boundary
        // blip, or a revision still compiling. Backfill drives on the same
        // stateless fleet, so this is reachable by exactly the deploy the
        // compile boundary exists for. Recording it `failed` would leave a
        // permanently red chunk that `rollup_range_status` folds into a failed
        // range, demanding an operator re-run for a condition that clears
        // itself in seconds.
        AirwayRunError::Unavailable(m) => ChunkFailureAction {
            status: "pending",
            disposition: ChunkDisposition::Deferred,
            note: format!("deferred: {m}"),
        },
        e => ChunkFailureAction {
            status: "failed",
            disposition: ChunkDisposition::Failed,
            note: e.to_string(),
        },
    }
}

/// The resources a range was scoped to, as every reader of the column must
/// read it.
///
/// NULL means UNSCOPED — every resource — because that is what every range
/// created before the column existed carries, and those ranges really did run
/// everything.
///
/// A stored value that does not decode as an array of names widens too, and
/// that direction is deliberate. Narrowing on a misparse would silently skip
/// resources an operator asked for and report the range `done`; widening at
/// worst spends what the pre-column behaviour already spent, and shows up as
/// extra runs rather than as missing data.
fn range_scope(stored: &Option<Value>) -> Vec<String> {
    normalize_scope(
        &stored
            .as_ref()
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
            .unwrap_or_default(),
    )
}

/// A scope is a SET of resource names, so it is compared as one.
///
/// Sorted and deduped, because the alternative is an identity that depends on
/// argument order: `--resources shipments,ledger_detail` and
/// `--resources ledger_detail,shipments` name the same backfill, and without
/// this the second would miss the first's range and re-run every chunk — the
/// same wasted replay `find_or_create_backfill_range` exists to avoid. The
/// runtime does not care either: `resources` is a filter over the pipeline's
/// resource map, not an execution order.
///
/// Applied on the way IN as well as on the way out, so a row written by this
/// version is already canonical and one written by an older one still compares
/// correctly.
fn normalize_scope(resources: &[String]) -> Vec<String> {
    let mut out: Vec<String> = resources
        .iter()
        // Trimmed, and blanks dropped. `--resources "shipments, ledger_detail"`
        // splits on the comma without trimming, so the second name arrives as
        // " ledger_detail", matches nothing airway declares, and the chunk
        // fetches one resource while still rolling up `done` — silent missing
        // data, the direction `range_scope`'s widening rule exists to avoid.
        // Worse once persisted: the stray space rides the range into every
        // resume. Done at this seam so the CLI and the HTTP handler cannot
        // disagree about it.
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Record a user-initiated backfill of `[from, to)` as a `backfill_ranges` row
/// and return its id. Chunks created under this id are owned by exactly this
/// range (per-run) — overlapping backfills are distinct ranges, never merged.
#[allow(clippy::too_many_arguments)]
pub async fn create_backfill_range(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    granularity: ChunkGranularity,
    concurrency: i32,
    created_by: Option<Uuid>,
    resources: &[String],
) -> Result<Uuid, DbErr> {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    // Normalise FIRST, then decide emptiness — the guard used to test the raw
    // slice, which trimming made wrong: `--resources " "` is non-empty going in
    // and empty once trimmed, so the row stored `[]`, the one literal this
    // column is meant never to hold. Nothing misbehaves today because
    // `range_scope` maps `[]` and NULL to the same value, but the stored form
    // would stop matching its own invariant, and a reader that does not go
    // through `range_scope` — a SQL filter, an admin query, the ranges gantt —
    // would see two spellings of unscoped.
    let scope = normalize_scope(resources);
    let stored_scope = (!scope.is_empty()).then(|| serde_json::json!(scope));
    RangeActive {
        id: Set(id),
        workspace_id: Set(workspace_id),
        pipeline_ref: Set(pipeline_ref.to_string()),
        requested_from: Set(from.fixed_offset()),
        requested_to: Set(to.fixed_offset()),
        granularity: Set(granularity.as_str().to_string()),
        concurrency: Set(concurrency),
        resources: Set(stored_scope),
        created_by: Set(created_by),
        status: Set("running".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await?;
    Ok(id)
}

/// Find an existing range that exactly matches this window (same workspace,
/// pipeline, `[from, to)`, granularity) or create one. Used by the CLI so that
/// re-running `oxy airway backfill` with the same window RESUMES that range
/// (drives only its not-`done` chunks) instead of spawning a duplicate range
/// that re-runs everything. The HTTP path deliberately creates a fresh range per
/// request — each user-initiated backfill is its own entry in the ranges gantt,
/// and a range's own "Resume" is the explicit resume path there.
#[allow(clippy::too_many_arguments)]
pub async fn find_or_create_backfill_range(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    granularity: ChunkGranularity,
    concurrency: i32,
    created_by: Option<Uuid>,
    resources: &[String],
) -> Result<Uuid, DbErr> {
    // The SCOPE is part of the identity, not just the window: re-running
    // `oxy airway backfill` for the same dates under a different `--resources`
    // is a different backfill, and resuming the old range would drive the old
    // scope's chunks while reporting the new one.
    //
    // It is matched while CHOOSING the row rather than after, and that is not a
    // refactor. Taking the newest range for the window and then comparing makes
    // only the most recent one a resume candidate, so alternating scopes never
    // resumes: scope A creates a range, unscoped creates another, then A again
    // finds the unscoped one newest, mismatches, and re-runs every chunk from
    // scratch — re-spending exactly the report jobs this scope exists to save.
    // The scope is not a SQL filter because NULL and `[]` both mean "every
    // resource"; `range_scope` is what reconciles them, so the comparison
    // belongs in Rust.
    let existing = BackfillRange::find()
        .filter(RangeCol::WorkspaceId.eq(workspace_id))
        .filter(RangeCol::PipelineRef.eq(pipeline_ref))
        .filter(RangeCol::RequestedFrom.eq(from.fixed_offset()))
        .filter(RangeCol::RequestedTo.eq(to.fixed_offset()))
        .filter(RangeCol::Granularity.eq(granularity.as_str()))
        .order_by_desc(RangeCol::CreatedAt)
        .all(db)
        .await?
        .into_iter()
        .find(|r| range_scope(&r.resources) == normalize_scope(resources));
    if let Some(existing) = existing {
        return Ok(existing.id);
    }
    create_backfill_range(
        db,
        workspace_id,
        pipeline_ref,
        from,
        to,
        granularity,
        concurrency,
        created_by,
        resources,
    )
    .await
}

/// Drive a backfill range to completion: split its `[requested_from,
/// requested_to)` window into `granularity` chunks and run each as a bounded
/// airway backfill, checkpointing every outcome under `range_id`. Resumable —
/// chunks already `done` are skipped, so re-driving retries only failed/partial
/// chunks. No cross-range merge: this range owns exactly the chunks its window
/// enumerates, and re-loading a period another range already covered is safe
/// (airhouse is merge-on-read). The range `status` is rolled up from its chunks
/// when the pass finishes.
///
/// Chunks run **one at a time**, whatever the range's stored `concurrency`
/// says — the value is clamped at the driver (see the pin below), and a higher
/// stored value logs a warning rather than taking effect. `on_progress` fires
/// once per chunk in completion order (the CLI prints; the HTTP driver passes a
/// no-op).
///
/// This doc previously recommended ≈4 as a "sweet spot", on the reasoning that
/// concurrency trades parallel extract for DuckLake commit-retries. That
/// reasoning was incomplete: chunks of one pipeline share a single
/// `<schema>_raw.<table>` staging buffer, and the fold's watermark spans the WHOLE
/// buffer — so one chunk's fold drains another's partially-loaded rows
/// mid-flight. Per-chunk cursor isolation (run-scoped store) covers the cursor
/// and nothing else.
pub async fn drive_backfill_range(
    db: &DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    range_id: Uuid,
    variables: Option<Value>,
    on_progress: impl FnMut(ChunkProgress),
) -> Result<BackfillSummary, AirwayRunError> {
    let range = BackfillRange::find_by_id(range_id)
        .one(db)
        .await?
        .ok_or_else(|| {
            AirwayRunError::InvalidInput(format!("backfill range {range_id} not found"))
        })?;
    let from = range.requested_from.with_timezone(&Utc);
    let to = range.requested_to.with_timezone(&Utc);
    let granularity =
        ChunkGranularity::parse(&range.granularity).unwrap_or(ChunkGranularity::Month);
    // Clamped to 1, not merely defaulted: `range.concurrency` is read from a
    // stored row, so existing ranges (and the HTTP path) would otherwise still
    // fan out. Concurrent chunks of one pipeline share a single `<schema>_raw.<table>`
    // buffer whose fold watermark spans the WHOLE buffer, so one chunk's fold
    // drains another's partially-loaded rows; and concurrent folds of one table
    // are the exact shape of the duplicate rows measured on pokehouse. Per-chunk
    // cursor isolation (run-scoped store) covers the cursor and nothing else.
    let concurrency = 1usize;
    // Say so rather than silently ignoring the operator's setting — a backfill
    // that quietly runs 4x slower than asked is its own support ticket.
    if range.concurrency > 1 {
        tracing::warn!(
            requested = range.concurrency,
            "backfill chunk concurrency is pinned to 1; the requested value is ignored \
             (chunks share one raw buffer and their folds would interleave)"
        );
    }
    // NULL (or unparseable) means unscoped — every resource — which is what
    // ranges created before the column existed meant, and what the chunk
    // driver did unconditionally before it could be asked otherwise.
    let resources = range_scope(&range.resources);
    let chunks = enumerate_chunks(from, to, granularity);
    // Pre-create every chunk as `pending` so coverage shows the plan (0/N) at once.
    for chunk in &chunks {
        checkpoint_upsert_pending(
            db,
            range_id,
            range.workspace_id,
            &range.pipeline_ref,
            chunk.start.fixed_offset(),
            chunk.end.fixed_offset(),
        )
        .await?;
    }
    let summary = run_chunks(
        db,
        platform,
        range_id,
        &range.pipeline_ref,
        variables,
        &resources,
        chunks,
        concurrency,
        on_progress,
    )
    .await?;
    rollup_range_status(db, range_id).await?;
    Ok(summary)
}

/// Drive a fixed set of one range's chunks concurrently (≤ `concurrency` at
/// once), folding each outcome into a `BackfillSummary`. Shared by
/// `drive_backfill_range` (the window) and `resume_backfill_range` (the missing
/// chunks).
#[allow(clippy::too_many_arguments)]
async fn run_chunks(
    db: &DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    backfill_range_id: Uuid,
    pipeline_ref: &str,
    variables: Option<Value>,
    resources: &[String],
    chunks: Vec<Chunk>,
    concurrency: usize,
    mut on_progress: impl FnMut(ChunkProgress),
) -> Result<BackfillSummary, AirwayRunError> {
    let mut summary = BackfillSummary {
        total: chunks.len(),
        ..Default::default()
    };
    // Each chunk future owns its inputs (cheap clones — `DatabaseConnection`
    // and `Arc` are ref-counted) so the buffered set is `'static + Send` and can
    // be driven from a detached `tokio::spawn`. Iterate `Chunk` by value
    // (it's `Copy`): a `&Chunk`-taking closure returning a future trips a
    // higher-ranked-lifetime `FnOnce is not general enough` error.
    let mut running = futures::stream::iter(chunks.into_iter().map(|chunk| {
        run_one_chunk(
            db.clone(),
            platform.clone(),
            backfill_range_id,
            pipeline_ref.to_string(),
            variables.clone(),
            resources.to_vec(),
            chunk,
        )
    }))
    .buffer_unordered(concurrency.max(1));
    while let Some(progress) = running.next().await.transpose()? {
        match progress.disposition {
            ChunkDisposition::Resumed => summary.resumed += 1,
            ChunkDisposition::Done => summary.done += 1,
            ChunkDisposition::Degraded => summary.degraded += 1,
            ChunkDisposition::Failed => summary.failed += 1,
            ChunkDisposition::Deferred => summary.deferred += 1,
        }
        on_progress(progress);
    }
    Ok(summary)
}

/// Recompute a range's rollup `status` from its chunks and persist it if
/// changed: `running` while a chunk is claimed, `pending` when work remains but
/// nothing is driving it (a chunk deferred by lease contention), else `done`
/// (all done), `failed` (any hard failure), or `degraded` (some
/// `completed_with_errors`, the rest done).
async fn rollup_range_status(db: &DatabaseConnection, range_id: Uuid) -> Result<(), DbErr> {
    let statuses: Vec<String> = Checkpoint::find()
        .filter(CpCol::BackfillRangeId.eq(range_id))
        .all(db)
        .await?
        .into_iter()
        .map(|r| r.status)
        .collect();
    // `pending` and `running` are NOT the same rollup. A chunk deferred by lease
    // contention is left `pending` with nobody driving it, and folding that into
    // "running" made the range read as live indefinitely — a stalled backfill
    // presented as an in-flight one, which is the state an operator is least
    // likely to investigate. `running` now means a chunk is genuinely claimed;
    // `pending` means work remains and nothing is doing it.
    let rolled = if statuses.is_empty() {
        "done"
    } else if statuses.iter().any(|s| s == "running") {
        "running"
    } else if statuses.iter().any(|s| s == "pending") {
        "pending"
    } else if statuses.iter().all(|s| s == "done") {
        "done"
    } else if statuses
        .iter()
        .any(|s| s == "failed" || s == "timed_out" || s == "cancelled")
    {
        "failed"
    } else {
        "degraded"
    };
    if let Some(range) = BackfillRange::find_by_id(range_id).one(db).await?
        && range.status != rolled
    {
        let mut active: RangeActive = range.into();
        active.status = Set(rolled.to_string());
        active.updated_at = Set(Utc::now().fixed_offset());
        active.update(db).await?;
    }
    Ok(())
}

/// Resume a backfill range by re-running exactly its not-`done` chunks, read
/// straight from the checkpoints owned by `range_id` — so only the actual
/// missing periods run (never a re-derived window), independent of the original
/// granularity. `run_one_chunk` skips any that flipped to `done` since the read,
/// so it's race-safe. Rolls the range status up afterward.
pub async fn resume_backfill_range(
    db: &DatabaseConnection,
    platform: Arc<dyn PlatformContext>,
    range_id: Uuid,
    variables: Option<Value>,
    on_progress: impl FnMut(ChunkProgress),
) -> Result<BackfillSummary, AirwayRunError> {
    // Gate on the caller's workspace so a range_id from another tenant can't be
    // driven cross-tenant.
    let range = BackfillRange::find_by_id(range_id)
        .filter(RangeCol::WorkspaceId.eq(platform.workspace_id()))
        .one(db)
        .await?
        .ok_or_else(|| {
            AirwayRunError::InvalidInput(format!("backfill range {range_id} not found"))
        })?;
    // Clamped to 1, not merely defaulted: `range.concurrency` is read from a
    // stored row, so existing ranges (and the HTTP path) would otherwise still
    // fan out. Concurrent chunks of one pipeline share a single `<schema>_raw.<table>`
    // buffer whose fold watermark spans the WHOLE buffer, so one chunk's fold
    // drains another's partially-loaded rows; and concurrent folds of one table
    // are the exact shape of the duplicate rows measured on pokehouse. Per-chunk
    // cursor isolation (run-scoped store) covers the cursor and nothing else.
    let concurrency = 1usize;
    // Say so rather than silently ignoring the operator's setting — a backfill
    // that quietly runs 4x slower than asked is its own support ticket.
    if range.concurrency > 1 {
        tracing::warn!(
            requested = range.concurrency,
            "backfill chunk concurrency is pinned to 1; the requested value is ignored \
             (chunks share one raw buffer and their folds would interleave)"
        );
    }
    // Same scope the range was created with — a resume that widened it would
    // be the unattended run spending report jobs the operator had excluded.
    let resources = range_scope(&range.resources);
    let chunks: Vec<Chunk> = Checkpoint::find()
        .filter(CpCol::BackfillRangeId.eq(range_id))
        .filter(CpCol::Status.ne("done"))
        .order_by_asc(CpCol::PeriodStart)
        .all(db)
        .await?
        .into_iter()
        .map(|r| Chunk {
            start: r.period_start.with_timezone(&Utc),
            end: r.period_end.with_timezone(&Utc),
        })
        .collect();
    let summary = run_chunks(
        db,
        platform,
        range_id,
        &range.pipeline_ref,
        variables,
        &resources,
        chunks,
        concurrency,
        on_progress,
    )
    .await?;
    rollup_range_status(db, range_id).await?;
    Ok(summary)
}

/// One chunk's coverage row (serialized for the HTTP coverage endpoint and the
/// CLI `--json` output).
#[derive(Debug, Clone, Serialize)]
pub struct CoverageChunk {
    pub period_start: DateTime<FixedOffset>,
    pub period_end: DateTime<FixedOffset>,
    pub status: String,
    pub run_id: Option<String>,
    pub row_count: Option<i64>,
    pub attempts: i32,
    pub error: Option<String>,
}

/// Rollup of coverage: how many chunks are `done`, the loaded *envelope*
/// (min/max over `done` chunks — NOT necessarily gap-free), and how many chunks
/// are still missing.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageSummary {
    pub total: usize,
    pub done: usize,
    /// Earliest `period_start` / latest `period_end` across `done` chunks, or
    /// `null` when nothing is done yet. This is an envelope: an interior
    /// not-`done` chunk does not narrow it — check `missing` / per-chunk status.
    pub loaded_from: Option<DateTime<FixedOffset>>,
    pub loaded_to: Option<DateTime<FixedOffset>>,
    pub missing: usize,
}

/// Full coverage report — every checkpoint chunk plus the rollup. Scoped either
/// to a single range (`range_id = Some`, the drill-in view) or to a whole
/// pipeline across all its ranges (`range_id = None`, the CLI/overall view).
#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub pipeline_ref: String,
    /// The range this report covers, or `None` for a pipeline-wide aggregate.
    pub range_id: Option<Uuid>,
    pub chunks: Vec<CoverageChunk>,
    pub summary: CoverageSummary,
}

/// Roll a set of checkpoint rows up into a `CoverageReport`.
fn build_coverage(
    pipeline_ref: String,
    range_id: Option<Uuid>,
    rows: Vec<CpModel>,
) -> CoverageReport {
    let total = rows.len();
    let done = rows.iter().filter(|r| r.status == "done").count();
    let loaded_from = rows
        .iter()
        .filter(|r| r.status == "done")
        .map(|r| r.period_start)
        .min();
    let loaded_to = rows
        .iter()
        .filter(|r| r.status == "done")
        .map(|r| r.period_end)
        .max();
    let chunks = rows
        .into_iter()
        .map(|r| CoverageChunk {
            period_start: r.period_start,
            period_end: r.period_end,
            status: r.status,
            run_id: r.run_id,
            row_count: r.row_count,
            attempts: r.attempts,
            error: r.error,
        })
        .collect();
    CoverageReport {
        pipeline_ref,
        range_id,
        chunks,
        summary: CoverageSummary {
            total,
            done,
            loaded_from,
            loaded_to,
            missing: total - done,
        },
    }
}

/// Pipeline-wide coverage across *all* of `pipeline_ref`'s ranges (ordered by
/// period). Answers "what period is missing?" — any chunk that isn't `done`.
/// Note: with per-run ranges a period backfilled by two ranges appears as two
/// chunk rows, so this aggregate counts chunk *rows*, not distinct periods.
pub async fn load_coverage(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
) -> Result<CoverageReport, DbErr> {
    let rows = Checkpoint::find()
        .filter(CpCol::WorkspaceId.eq(workspace_id))
        .filter(CpCol::PipelineRef.eq(pipeline_ref))
        .order_by_asc(CpCol::PeriodStart)
        .all(db)
        .await?;
    Ok(build_coverage(pipeline_ref.to_string(), None, rows))
}

/// Coverage for a single backfill range (the UI drill-in): the range's own
/// chunk rows, ordered by period. Gated on `workspace_id` so a `range_id` from
/// another tenant returns an empty report rather than leaking its chunks.
pub async fn load_range_coverage(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    range_id: Uuid,
) -> Result<CoverageReport, DbErr> {
    let Some(range) = BackfillRange::find_by_id(range_id)
        .filter(RangeCol::WorkspaceId.eq(workspace_id))
        .one(db)
        .await?
    else {
        return Ok(build_coverage(String::new(), Some(range_id), Vec::new()));
    };
    let rows = Checkpoint::find()
        .filter(CpCol::BackfillRangeId.eq(range_id))
        .order_by_asc(CpCol::PeriodStart)
        .all(db)
        .await?;
    Ok(build_coverage(range.pipeline_ref, Some(range_id), rows))
}

/// One backfill range plus its chunk tally, for the ranges list / gantt.
#[derive(Debug, Clone, Serialize)]
pub struct BackfillRangeInfo {
    pub id: Uuid,
    pub pipeline_ref: String,
    pub requested_from: DateTime<FixedOffset>,
    pub requested_to: DateTime<FixedOffset>,
    pub granularity: String,
    pub concurrency: i32,
    pub created_by: Option<Uuid>,
    pub status: String,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    /// Chunk tally for this range.
    pub total: usize,
    pub done: usize,
    pub missing: usize,
}

/// List a pipeline's backfill ranges (newest first) with each range's chunk
/// tally — the source for the ranges gantt. One checkpoint scan, grouped by
/// range in memory.
pub async fn list_backfill_ranges(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
) -> Result<Vec<BackfillRangeInfo>, DbErr> {
    let ranges = BackfillRange::find()
        .filter(RangeCol::WorkspaceId.eq(workspace_id))
        .filter(RangeCol::PipelineRef.eq(pipeline_ref))
        .order_by_desc(RangeCol::CreatedAt)
        .all(db)
        .await?;
    let checkpoints = Checkpoint::find()
        .filter(CpCol::WorkspaceId.eq(workspace_id))
        .filter(CpCol::PipelineRef.eq(pipeline_ref))
        .all(db)
        .await?;
    let mut tally: std::collections::HashMap<Uuid, (usize, usize)> =
        std::collections::HashMap::new();
    for c in &checkpoints {
        let e = tally.entry(c.backfill_range_id).or_insert((0, 0));
        e.0 += 1;
        if c.status == "done" {
            e.1 += 1;
        }
    }
    Ok(ranges
        .into_iter()
        .map(|r| {
            let (total, done) = tally.get(&r.id).copied().unwrap_or((0, 0));
            BackfillRangeInfo {
                id: r.id,
                pipeline_ref: r.pipeline_ref,
                requested_from: r.requested_from,
                requested_to: r.requested_to,
                granularity: r.granularity,
                concurrency: r.concurrency,
                created_by: r.created_by,
                status: r.status,
                created_at: r.created_at,
                updated_at: r.updated_at,
                total,
                done,
                missing: total - done,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// NULL is not "no resources", it is "all of them" — the behaviour every
    /// range predating the column already had.
    #[test]
    fn an_absent_scope_means_every_resource() {
        assert_eq!(range_scope(&None), Vec::<String>::new());
    }

    /// A name carrying the whitespace of a comma-separated flag matches
    /// nothing airway declares, and the chunk would still report `done`.
    #[test]
    fn a_scope_is_trimmed_and_blanks_dropped() {
        let stored = Some(serde_json::json!(["shipments", " ledger_detail", "", "  "]));
        assert_eq!(range_scope(&stored), vec!["ledger_detail", "shipments"]);
    }

    /// A scope of nothing but blanks is UNSCOPED, which is what makes
    /// `create_backfill_range` store NULL for it rather than `[]`. The guard
    /// there normalises before testing emptiness for exactly this input.
    #[test]
    fn a_scope_of_only_blanks_is_unscoped() {
        let only_blanks = [" ".to_string(), "".to_string(), "\t".to_string()];
        assert!(normalize_scope(&only_blanks).is_empty());
    }

    /// Round-trips as a SET — the names survive, canonically ordered. It does
    /// not preserve the order they were written in, which is what makes two
    /// spellings of the same scope resume each other.
    #[test]
    fn a_stored_scope_round_trips_canonically() {
        let stored = Some(serde_json::json!(["shipments", "ledger_detail"]));
        assert_eq!(range_scope(&stored), vec!["ledger_detail", "shipments"]);
    }

    /// Widen rather than narrow. Narrowing on a value nobody can parse would
    /// skip resources the operator asked for and still roll the range up as
    /// `done`; widening costs report jobs, which is visible.
    #[test]
    fn an_unreadable_scope_widens_rather_than_narrows() {
        assert_eq!(
            range_scope(&Some(serde_json::json!({"oops": 1}))),
            Vec::<String>::new()
        );
        assert_eq!(
            range_scope(&Some(serde_json::json!([1, 2]))),
            Vec::<String>::new()
        );
    }

    /// Order is not identity. Without this, `--resources a,b` and
    /// `--resources b,a` are different backfills and the second re-runs every
    /// chunk the first already landed.
    #[test]
    fn a_scope_is_a_set_not_a_sequence() {
        let one = Some(serde_json::json!(["shipments", "ledger_detail"]));
        let other = Some(serde_json::json!(["ledger_detail", "shipments"]));
        assert_eq!(range_scope(&one), range_scope(&other));
        assert_eq!(
            range_scope(&Some(serde_json::json!(["a", "a", "b"]))),
            vec!["a", "b"]
        );
    }

    #[test]
    fn month_chunks_are_calendar_aligned_and_clamped() {
        let chunks = enumerate_chunks(
            ts("2024-01-15T00:00:00Z"),
            ts("2024-03-10T00:00:00Z"),
            ChunkGranularity::Month,
        );
        assert_eq!(
            chunks,
            vec![
                Chunk {
                    start: ts("2024-01-15T00:00:00Z"),
                    end: ts("2024-02-01T00:00:00Z")
                },
                Chunk {
                    start: ts("2024-02-01T00:00:00Z"),
                    end: ts("2024-03-01T00:00:00Z")
                },
                Chunk {
                    start: ts("2024-03-01T00:00:00Z"),
                    end: ts("2024-03-10T00:00:00Z")
                },
            ]
        );
    }

    #[test]
    fn chunks_tile_the_range_with_no_gaps_or_overlaps() {
        let start = ts("2023-11-20T00:00:00Z");
        let end = ts("2024-02-03T00:00:00Z");
        for g in [
            ChunkGranularity::Day,
            ChunkGranularity::Week,
            ChunkGranularity::Month,
        ] {
            let chunks = enumerate_chunks(start, end, g);
            assert_eq!(
                chunks.first().unwrap().start,
                start,
                "{g:?} starts at start"
            );
            assert_eq!(chunks.last().unwrap().end, end, "{g:?} ends at end");
            for w in chunks.windows(2) {
                assert_eq!(w[0].end, w[1].start, "{g:?} contiguous, no gap/overlap");
            }
        }
    }

    #[test]
    fn empty_or_inverted_range_yields_no_chunks() {
        let t = ts("2024-01-01T00:00:00Z");
        assert!(enumerate_chunks(t, t, ChunkGranularity::Month).is_empty());
        assert!(enumerate_chunks(t, t - Duration::days(1), ChunkGranularity::Day).is_empty());
    }

    #[test]
    fn week_aligns_to_monday() {
        // 2024-01-03 is a Wednesday → first interior boundary is Mon 2024-01-08.
        let chunks = enumerate_chunks(
            ts("2024-01-03T00:00:00Z"),
            ts("2024-01-20T00:00:00Z"),
            ChunkGranularity::Week,
        );
        assert_eq!(chunks[0].end, ts("2024-01-08T00:00:00Z"));
        assert_eq!(chunks[1].end, ts("2024-01-15T00:00:00Z"));
    }

    #[test]
    fn granularity_parse() {
        assert_eq!(
            ChunkGranularity::parse("Monthly"),
            Some(ChunkGranularity::Month)
        );
        assert_eq!(ChunkGranularity::parse("day"), Some(ChunkGranularity::Day));
        assert_eq!(ChunkGranularity::parse("fortnight"), None);
    }
}

#[cfg(test)]
mod chunk_failure_action_tests {
    use super::{AirwayRunError, ChunkDisposition, chunk_action_for_error};

    /// A blip must not leave coverage red. `rollup_range_status` folds a
    /// `failed` chunk into a failed range, so recording one here costs an
    /// operator a manual re-run for a node condition that clears itself.
    #[test]
    fn unavailable_leaves_the_chunk_pending_and_deferred() {
        let a = chunk_action_for_error(&AirwayRunError::Unavailable("compiling".into()));
        assert_eq!(a.status, "pending");
        assert_eq!(a.disposition, ChunkDisposition::Deferred);
        assert!(a.note.starts_with("deferred: "), "note was {:?}", a.note);
    }

    /// And the converse, or the deferral would hide real load errors as
    /// perpetually-pending chunks that never surface to anyone.
    #[test]
    fn any_other_error_still_fails_the_chunk() {
        let a = chunk_action_for_error(&AirwayRunError::InvalidInput("bad ref".into()));
        assert_eq!(a.status, "failed");
        assert_eq!(a.disposition, ChunkDisposition::Failed);
    }
}
