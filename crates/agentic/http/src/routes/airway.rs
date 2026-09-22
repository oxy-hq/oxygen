//! Airway run lifecycle handlers.
//!
//! Airway is queue-driven like automation: `POST /runs` seeds an
//! `agentic_runs` row + `airway_run_extensions` row and enqueues a
//! **`Global`** `TaskSpec::Airway`, which a driver process — normally a
//! worker-fleet pod, not this one — claims and drives to completion. The
//! handler itself does no driving; see `start_and_drive`.
//!
//! SSE just streams whatever lands in `agentic_run_events` — the registry
//! routes by `source_type = "airway"`, so the shared `stream_events` handler
//! needs no airway awareness. It does need a timer rather than the in-process
//! notifier, though, now that the writer is usually another pod: see
//! `REMOTE_DRIVER_POLL_INTERVAL` in `routes/run.rs`.
//!
//! ## Routes
//!
//! | Method | Path                              | Purpose |
//! |--------|-----------------------------------|---------|
//! | POST   | `/agentic-airway/runs`            | Start a run |
//! | GET    | `/agentic-airway/runs/:id/events` | SSE stream (shared handler) |
//! | POST   | `/agentic-airway/runs/:id/cancel` | Cancel a running pipeline |
//!
//! ## Two driving models live in this file — check which one you are in
//!
//! Everything routed through [`start_and_drive`] (`/runs` and the
//! single-window `/backfill`) enqueues `TaskScope::Global` and drives
//! nothing locally. The **chunked** backfill (`/chunked-backfill`, and
//! `/backfill-ranges/:id/resume`) still direct-drives its chunks in-process
//! via `drive_backfill_range` / `resume_backfill_range`, so those requests
//! still execute on the node that accepted them and still charge its memory.
//!
//! That asymmetry is deliberate, not an oversight: moving the chunk driver is
//! the other half of the reverted option (b) (`7dc7148ed`) and is a
//! materially larger change than the scope flip here. Do not "make it
//! consistent" by adding a direct-drive back to `start_and_drive`.

use std::sync::Arc;

use agentic_runtime::hub_task::spawn_with_hub;
use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::{mpsc, watch};

use agentic_pipeline::WorkflowWorkspaceContext;
use agentic_pipeline::airway_run::{
    AirwayRunError, StartAirwayRequest, list_airway_runs, start_airway_run,
};
use agentic_pipeline::backfill::{
    ChunkGranularity, create_backfill_range, drive_backfill_range, enumerate_chunks,
    list_backfill_ranges, load_range_coverage, resume_backfill_range,
};
use agentic_pipeline::platform::PlatformContext;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use super::run_scope::ensure_run_access;
use crate::state::AgenticState;

#[derive(Serialize)]
pub struct CreateAirwayRunResponse {
    pub run_id: String,
}

#[derive(Deserialize)]
pub struct AirwayRunIdPath {
    id: String,
}

#[derive(Deserialize)]
pub struct ListRunsQuery {
    pub pipeline_ref: String,
    /// Hard cap to keep responses bounded; the dropdown only shows the
    /// most-recent N. Defaults to 50, clamped to 200.
    #[serde(default)]
    pub limit: Option<u64>,
}

// ── GET /agentic-airway/runs?pipeline_ref=... ──────────────────────────────

/// Workspace-scoped: `pipeline_ref` is a workspace-relative path, so it is
/// only unique together with the workspace. `platform.workspace_id()` is the
/// id `start_and_drive` stamps on the run, so the filter matches it in every
/// serve mode (the nil UUID in local mode included).
pub async fn list_runs_for_pipeline(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    axum::extract::Query(q): axum::extract::Query<ListRunsQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).min(200);
    match list_airway_runs(&state.db, platform.workspace_id(), &q.pipeline_ref, limit).await {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_runs_for_pipeline failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("list runs: {e}")).into_response()
        }
    }
}

// ── POST /agentic-airway/runs ──────────────────────────────────────────────

/// `Retry-After` for every airway 503, in seconds.
///
/// Derived from the executor's defer cadence rather than restated: the two
/// answer the same question ("when is it worth asking again?") for the same
/// condition, and three hand-written `"5"`s across two routes and another crate
/// is three places for one number to drift. That cadence has already moved once
/// for reasons a route author would not see.
fn airway_unavailable_retry_after() -> String {
    agentic_pipeline::executor::AIRWAY_UNAVAILABLE_RETRY_SECS.to_string()
}

pub async fn create_airway_run(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<StartAirwayRequest>,
) -> Response {
    // Write-side authz: a caller-supplied `thread_id` is persisted on
    // the run and surfaces in that thread's feed. Without this check
    // any authed user could attach a run to someone else's thread
    // (read-side is already guarded by `ensure_run_access`).
    if let Some(tid) = body.thread_id {
        match state.thread_owner.thread_owner(tid).await {
            Ok(None) => return (StatusCode::NOT_FOUND, "thread not found").into_response(),
            Ok(Some(Some(owner_id))) if owner_id != user.id => {
                return (StatusCode::FORBIDDEN, "access denied").into_response();
            }
            Ok(_) => {}
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}"))
                    .into_response();
            }
        }
    }

    start_and_drive(state, platform, body).await
}

/// Shared tail for the airway start handlers: seed the run, map
/// `start_airway_run` errors to status codes, then register the cancel/answer
/// channels and return. The queued `TaskSpec::Airway` is claimed and driven by
/// a *driver process* — not by this handler. Both `create_airway_run` and
/// `backfill_airway` build their `StartAirwayRequest` and delegate here.
///
/// **The run is enqueued `Global`, and that is the whole point of this
/// function's shape.** It used to be `Scoped` plus an out-of-band
/// `spawn_airway_run_drive` on this very node, which meant a memory-heavy
/// pipeline executed inside whichever pod served the submit — and airway
/// submit routes are `IdeOnly`, so that was always the IDE singleton, the pod
/// least able to afford it. `Global` hands the run to the durable queue, where
/// the worker fleet claims it (`internal-docs/worker-fleet.md`). It also makes
/// the run crash-recoverable for free: a dead claim is requeued by the reaper
/// and resumed by another worker, where the old direct-drive stranded it at
/// `running` forever.
///
/// This ordering is load-bearing and was gotten wrong once. Going `Global`
/// before `oxy worker` could drive runs is what had to be reverted in
/// `54355b198` — nothing claimed the task and every run hung. It is safe now
/// only because the worker actually drives (Phase 1, #3014).
///
/// The `oxy airway run` CLI deliberately keeps its direct-drive: it is a
/// one-shot with no fleet behind it, so its own spawn *is* its worker and
/// going `Global` there would hang exactly as the revert describes.
async fn start_and_drive(
    state: Arc<AgenticState>,
    platform: Arc<dyn PlatformContext>,
    request: StartAirwayRequest,
) -> Response {
    // `PlatformContext: WorkflowWorkspaceContext`, so this coercion is free —
    // `start_airway_run` only needs the workspace surface. `workspace_id`
    // routes the row back to its workspace, which is what lets an
    // out-of-process driver rebuild the right `PlatformContext` for the run;
    // with the direct-drive gone it is the only thing that does.
    //
    // Read before the move rather than cloning the Arc: the clone was only
    // there because `spawn_airway_run_drive` consumed `platform`.
    let workspace_id = platform.workspace_id();
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform;
    let run_id = match start_airway_run(
        &state.db,
        workspace.as_ref(),
        request,
        agentic_pipeline::TaskScope::Global,
        workspace_id,
    )
    .await
    {
        Ok(id) => id,
        Err(AirwayRunError::InvalidInput(msg)) | Err(AirwayRunError::Io(msg)) => {
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
        // 503 + Retry-After, not 400: the caller's ref may be perfectly good
        // and this node simply could not resolve it — a compile-boundary blip,
        // or a revision mid-compile. Answering 400 tells a client to fix a
        // request that is not broken, and tells a retrying scheduler to stop.
        Err(AirwayRunError::Unavailable(msg)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(
                    axum::http::header::RETRY_AFTER,
                    airway_unavailable_retry_after(),
                )],
                msg,
            )
                .into_response();
        }
        Err(AirwayRunError::Airway(e)) => {
            // Spec parse / validation failure — caller's input problem.
            return (StatusCode::BAD_REQUEST, format!("airway spec: {e}")).into_response();
        }
        // 409, not 500: nothing is broken — this pipeline is already running.
        // The active run id rides the body so the UI can link to it rather
        // than telling the user to go hunt for it.
        Err(AirwayRunError::AlreadyRunning {
            pipeline_name,
            run_id,
        }) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "pipeline_already_running",
                    "pipeline_name": pipeline_name,
                    "run_id": run_id,
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(%e, "airway run start failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("start: {e}")).into_response();
        }
    };

    // Register cancel + answer channels so the Stop button works. Airway never
    // consumes answers (no HITL), but `register` wants the pair; the answer_rx
    // is simply dropped.
    //
    // The `cancel_rx` is dropped too, and that is not a leak of intent: the
    // driver is in another process, so an in-memory watch channel cannot reach
    // it. `cancel_airway_run` writes the durable cancel flag
    // (`crud::request_cancel`) which the out-of-process driver polls; the
    // in-memory `cancel_tx` stays registered only so `state.cancel(..)` can
    // still answer "was this run live here?" on the node that accepted it.
    //
    // Registering the notifier also keeps this node's SSE stream on its fast
    // path when the driver happens to be local (`OXY_ROLE=all`). When it is
    // NOT local, nothing in this process ever rings that notifier — see
    // `stream_events`, which is why it polls on a timer rather than parking on
    // the notifier alone.
    let (answer_tx, _answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, _cancel_rx) = watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);

    Json(CreateAirwayRunResponse { run_id }).into_response()
}

// ── POST /agentic-airway/backfill ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct BackfillAirwayRequest {
    /// Path to a `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Inclusive lower bound (RFC3339). The window is half-open `[from, to)`.
    pub from: chrono::DateTime<chrono::Utc>,
    /// Exclusive upper bound (RFC3339).
    pub to: chrono::DateTime<chrono::Utc>,
    /// Optional subset of resources to backfill. Empty = whole spec; the
    /// non-date-windowed resources just ignore the window.
    #[serde(default)]
    pub resources: Vec<String>,
}

/// Start a bounded date-window backfill. Pins `[from, to)` onto the
/// date-windowed source (toast, quickbooks, sp_api) and drives a normal run; a
/// live pipeline's incremental state is unaffected either way — quickbooks
/// freezes its cursor, toast and sp_api advance one in a run-scoped store so
/// the backfill can resume. Other source kinds are rejected by the executor.
pub async fn backfill_airway(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Json(body): Json<BackfillAirwayRequest>,
) -> Response {
    if body.from >= body.to {
        return (
            StatusCode::BAD_REQUEST,
            "backfill `from` must be strictly before `to`",
        )
            .into_response();
    }

    let request = StartAirwayRequest {
        pipeline_ref: body.pipeline_ref,
        variables: None,
        thread_id: None,
        resources: body.resources,
        schedule_id: None,
        trigger: Some("backfill".to_string()),
        logical_date: None,
        retry_of: None,
        backfill_from: Some(body.from.to_rfc3339()),
        backfill_to: Some(body.to.to_rfc3339()),
    };

    start_and_drive(state, platform, request).await
}

// ── POST /agentic-airway/chunked-backfill ──────────────────────────────────

#[derive(Deserialize)]
pub struct ChunkedBackfillRequest {
    /// Path to a `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Inclusive lower bound (RFC3339). The window is half-open `[from, to)`.
    pub from: chrono::DateTime<chrono::Utc>,
    /// Exclusive upper bound (RFC3339).
    pub to: chrono::DateTime<chrono::Utc>,
    /// Chunk size: `month` | `week` | `day`.
    pub granularity: String,
    /// Resources to replay. Empty (or omitted) means every resource the
    /// pipeline declares.
    ///
    /// Worth setting on any source with SNAPSHOT resources. A backfill run is
    /// run-scoped, so a snapshot sees empty state, concludes it has never run,
    /// and pulls its ordinary daily snapshot on EVERY chunk — for a period the
    /// upstream serves no historical form of. On a 13-chunk sp_api backfill
    /// that is ~39 report jobs spent against a daily quota shared with the
    /// vendor's own UI, which then refuses the windowed reports that were the
    /// point. The scope is stored on the range, so `/resume-backfill` keeps it.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Accepted for compatibility and IGNORED: chunks of one pipeline run one
    /// at a time. They share a single `<table>_raw` staging buffer whose fold
    /// watermark spans the whole buffer, so a parallel chunk's fold drains
    /// another's partially-loaded rows. Defaults to 1; the driver clamps
    /// regardless, and a higher value only logs a warning.
    #[serde(default)]
    pub concurrency: Option<usize>,
}

#[derive(Serialize)]
pub struct ChunkedBackfillResponse {
    /// The backfill range created for this window. Poll
    /// `/coverage?range_id=…`, or list via `/backfill-ranges`, for progress.
    pub range_id: Uuid,
    /// Number of chunks the window was split into (also the number of
    /// checkpoint rows the driver will drive).
    pub chunk_count: usize,
}

/// Start a chunked backfill: create a `backfill_ranges` row for `[from, to)`,
/// split it into `granularity` chunks, and drive each as a bounded window,
/// checkpointing each outcome under the range.
///
/// Returns immediately with the range id + chunk count — the actual drive runs
/// detached (it can take far longer than a request). Each POST creates a NEW
/// range (a distinct entry in the ranges gantt); to resume a range's failed or
/// interrupted chunks — including recovering a mid-drive process restart — POST
/// `/resume-backfill { range_id }`, which re-drives only the range's not-`done`
/// chunks. Progress is read via `GET /coverage?range_id`.
pub async fn chunked_backfill(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<ChunkedBackfillRequest>,
) -> Response {
    if body.from >= body.to {
        return (
            StatusCode::BAD_REQUEST,
            "backfill `from` must be strictly before `to`",
        )
            .into_response();
    }
    let granularity = match ChunkGranularity::parse(&body.granularity) {
        Some(g) => g,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid granularity `{}` (expected month|week|day)",
                    body.granularity
                ),
            )
                .into_response();
        }
    };
    let workspace_id = platform.workspace_id();
    // 1, not 4: chunks of one pipeline are serialized (they share a single
    // `<table>_raw` staging buffer whose fold watermark spans the whole buffer).
    // The driver clamps regardless, so a persisted 4 only bought an
    // ignored-value warning on every drive of every HTTP-created range.
    let concurrency = body.concurrency.unwrap_or(1).clamp(1, 16);
    // No merge: this range owns exactly the chunks its window enumerates.
    let chunk_count = enumerate_chunks(body.from, body.to, granularity).len();

    // Record the range up front (captures the initiating user), then drive it
    // detached — the drive runs the chunks (up to `concurrency` at once) well
    // beyond a request's lifetime. Checkpoints make it resumable; losing this
    // task (restart) just means a Resume of the range continues from its
    // not-`done` chunks. Variables are None — the HTTP path renders the spec
    // from persisted config, like the single-window backfill.
    let range_id = match create_backfill_range(
        &state.db,
        workspace_id,
        &body.pipeline_ref,
        body.from,
        body.to,
        granularity,
        concurrency as i32,
        Some(user.id),
        &body.resources,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(%e, "chunked backfill: create range failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("backfill: {e}")).into_response();
        }
    };

    let db = state.db.clone();
    let pref = body.pipeline_ref.clone();
    spawn_with_hub(async move {
        if let Err(e) = drive_backfill_range(&db, platform, range_id, None, |_| {}).await {
            tracing::error!(%e, pipeline_ref = %pref, %range_id, "chunked backfill driver failed");
        }
    });

    Json(ChunkedBackfillResponse {
        range_id,
        chunk_count,
    })
    .into_response()
}

// ── POST /agentic-airway/resume-backfill ───────────────────────────────────

#[derive(Deserialize)]
pub struct ResumeBackfillRequest {
    /// The backfill range to resume.
    pub range_id: Uuid,
}

/// Resume a backfill range: re-run exactly its not-`done` chunks (read straight
/// from the range's checkpoints, at the range's stored concurrency). Returns the
/// count it will re-run; the drive is detached like `chunked_backfill`, and
/// progress is read via `GET /coverage?range_id=…`.
pub async fn airway_resume(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Json(body): Json<ResumeBackfillRequest>,
) -> Response {
    let range_id = body.range_id;
    // Count the missing chunks up front (for the response) — the same not-`done`
    // set the detached resume will drive. Workspace-scoped read.
    let missing = match load_range_coverage(&state.db, platform.workspace_id(), range_id).await {
        Ok(report) => report.summary.missing,
        Err(e) => {
            tracing::error!(%e, "airway resume: coverage read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("resume: {e}")).into_response();
        }
    };
    let db = state.db.clone();
    // Detach, same deferral as `chunked_backfill` (KNOWN / DEFERRED, per
    // oxy-task-spec-default): this drive is a bare `tokio::spawn`, not a durable
    // `TaskSpec`, so a mid-drive restart drops the in-flight resume. Checkpoints
    // make that safe — another Resume just continues from the still-not-`done`
    // chunks. `resume_backfill_range` re-checks the range's workspace.
    spawn_with_hub(async move {
        if let Err(e) = resume_backfill_range(&db, platform, range_id, None, |_| {}).await {
            tracing::error!(%e, %range_id, "airway resume driver failed");
        }
    });

    Json(ChunkedBackfillResponse {
        range_id,
        chunk_count: missing,
    })
    .into_response()
}

// ── GET /agentic-airway/backfill-ranges?pipeline_ref=... ───────────────────

#[derive(Deserialize)]
pub struct BackfillRangesQuery {
    pub pipeline_ref: String,
}

/// List a pipeline's backfill ranges (newest first) with each range's chunk
/// tally — the source for the ranges gantt. Read-only, workspace-scoped.
pub async fn airway_backfill_ranges(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    axum::extract::Query(q): axum::extract::Query<BackfillRangesQuery>,
) -> Response {
    match list_backfill_ranges(&state.db, platform.workspace_id(), &q.pipeline_ref).await {
        Ok(ranges) => Json(ranges).into_response(),
        Err(e) => {
            tracing::error!(%e, "airway backfill ranges failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("ranges: {e}")).into_response()
        }
    }
}

// ── GET /agentic-airway/coverage?range_id=... ──────────────────────────────

#[derive(Deserialize)]
pub struct CoverageQuery {
    pub range_id: Uuid,
}

/// Coverage for a single backfill range: every checkpoint chunk plus a rollup
/// (done/total, loaded envelope, missing count). Read-only, workspace-scoped —
/// drives the UI's per-range coverage grid.
pub async fn airway_coverage(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    axum::extract::Query(q): axum::extract::Query<CoverageQuery>,
) -> Response {
    match load_range_coverage(&state.db, platform.workspace_id(), q.range_id).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => {
            tracing::error!(%e, "airway coverage failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("coverage: {e}")).into_response()
        }
    }
}

// ── POST /agentic-airway/runs/:id/cancel ───────────────────────────────────

pub async fn cancel_airway_run(
    Path(AirwayRunIdPath { id: run_id }): Path<AirwayRunIdPath>,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    if let Err(resp) = ensure_run_access(&state, &user.id, &run_id, platform.workspace_id()).await {
        return resp;
    }
    // Durable cross-process cancel signal (see cancel_automation_run).
    agentic_runtime::crud::request_cancel(&state.db, &run_id)
        .await
        .ok();
    if !state.cancel(&run_id) {
        // No live cancel channel — which now means one of THREE things, and
        // only the first two want the defensive write below.
        //
        // (a) The run finished and its channels were deregistered. Handled by
        //     the `already_terminal` guard: must not rewrite a `done` run.
        // (b) No queue row at all — nothing will ever drive this. The only
        //     case the defensive write still reaches.
        //
        //     It used to say "a stuck queue row whose driver died", and that
        //     is now wrong: a dead driver's row reads `claimed` until the
        //     reaper requeues it, and `claimed` is in the live set below, so
        //     `driver_may_hold_it` is true and the write is skipped. That is
        //     the right outcome — the reaper owns dead claims and will requeue
        //     the row for someone to finish or fail properly — but the comment
        //     described the opposite, which in this file is worse than saying
        //     nothing.
        // (c) **The run is being driven in another process.** Since Phase 2 an
        //     interactive submit is enqueued `Global` and `start_and_drive`
        //     drops the `cancel_rx` — there is no local driver to receive on
        //     it — so `watch::Sender::send` fails for want of receivers and
        //     `state.cancel` reads `false` even though a worker is executing
        //     the pipeline right now. A scheduler-seeded run reaches the same
        //     place by never having been registered on this pod at all, which
        //     means (c) predates Phase 2 and is not only a new-code concern.
        //
        // Writing `failed` in (c) races a live driver: the user is shown
        // "cancelled by user" as a FAILURE while the pipeline keeps running,
        // and the driver's own terminal write lands afterwards on top of it.
        // Nothing is cancelled by that write — the durable `request_cancel`
        // above is what actually stops the run, and the driver turns it into a
        // proper `cancelled`.
        //
        // A live queue entry is what separates (c) from (b): `queued` or
        // `claimed` means a driver holds the row or is about to take it.
        // Errs toward NOT writing — a failed lookup is treated as "something
        // may be driving this", because the durable cancel flag has already
        // been written and is sufficient on its own, while a wrong defensive
        // write is not recoverable.
        let driver_may_hold_it =
            match agentic_runtime::crud::get_queue_entry(&state.db, &run_id).await {
                Ok(Some(entry)) => {
                    matches!(entry.queue_status.as_str(), "queued" | "claimed")
                }
                // No queue row at all, so nothing is going to drive it: the
                // defensive write is exactly right here.
                Ok(None) => false,
                // Unknown. Decline to write: the durable cancel flag is
                // already in, and it is what actually stops a live run, so
                // the cost is at most a slower cancel. A wrong `failed` on a
                // running pipeline is not recoverable.
                Err(e) => {
                    tracing::warn!(
                        %run_id, error = %e,
                        "airway cancel: queue lookup failed; skipping the \
                         defensive fail-write rather than risk racing a live \
                         driver"
                    );
                    true
                }
            };
        let already_terminal = match agentic_runtime::crud::get_run(&state.db, &run_id).await {
            Ok(Some(run)) => matches!(
                run.task_status.as_deref(),
                Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
            ),
            Ok(None) => true,
            Err(e) => {
                tracing::warn!(%run_id, error = %e, "airway cancel: status lookup failed");
                true
            }
        };
        if !already_terminal
            && !driver_may_hold_it
            && let Err(e) =
                agentic_runtime::crud::update_run_failed(&state.db, &run_id, "cancelled by user")
                    .await
        {
            tracing::warn!(%run_id, error = %e, "airway cancel: defensive DB update failed");
        }
        // Free the lease for a queued-but-unclaimed run, whose `drive` never
        // ran and so never releases. Gated on unclaimed deliberately: cancel is
        // polled, so on a replica that isn't driving, the worker may still be
        // mid-fold — releasing there would admit a second run alongside one
        // still writing. That case is left to the worker's own release.
        agentic_pipeline::airway_run::release_airway_lease_if_unclaimed(&state.db, &run_id).await;
        // Wake any local SSE subscriber to re-read immediately rather than
        // wait out its poll interval.
        state.notify(&run_id);
        // Clear the channel maps only when this handler has actually settled
        // the run. `deregister` drops all three `state.register` created
        // (`notifiers`, `answer_txs`, `cancel_txs`) — the drive that used to
        // do this is gone, and the remote driver's own `deregister` runs in
        // its process, not ours.
        //
        // Gated because a worker may still be driving: `stream_events` no
        // longer treats a missing notifier as "the run is over" (it asks the
        // run row), so dropping it early is not the hang it once was — but it
        // would still discard the `cancel_tx` for a live run and cost this pod
        // the ability to answer "was this run live here?". Left registered,
        // the stream delivers the driver's real `cancelled` when it lands and
        // reaps the maps itself.
        if !driver_may_hold_it {
            state.deregister(&run_id);
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── GET /agentic-airway/files ─────────────────────
//
// Mirrors `/agentic-workflows/files`: lists `.airway.yml` pipeline
// files as { path, path_b64 } so the Schedules UI target picker can be
// populated for airway schedules.

#[derive(Serialize)]
pub struct AirwayFile {
    /// Workspace-relative path, usable as a schedule `target_ref`.
    pub path: String,
    /// URL-safe base64 of `path` (parity with the automation files shape).
    pub path_b64: String,
}

pub async fn list_airway_files(
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
) -> Response {
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    let paths = match workspace.list_airway_files().await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list files: {e}"),
            )
                .into_response();
        }
    };
    let root = workspace
        .workspace_path()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let files: Vec<AirwayFile> = paths
        .into_iter()
        .map(|abs| {
            let rel = abs
                .strip_prefix(&root)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let path_b64 = URL_SAFE_NO_PAD.encode(rel.as_bytes());
            AirwayFile {
                path: rel,
                path_b64,
            }
        })
        .collect();
    Json(files).into_response()
}

// ── POST /agentic-airway/sources/discover ──────────────────────────────────
//
// Connect to a SQL source with the live credentials supplied at wizard
// time and return its tables (with columns) so the New Pipeline UI can
// offer a table picker instead of hand-typed table names. Stateless —
// nothing is persisted. Authed: a caller can already author pipelines
// that connect anywhere, so this grants no new privilege; the auth gate
// is kept because the handler makes an outbound connection to a
// caller-specified host.
//
// KNOWN / DEFERRED (intentional, not a review gap): this dials a
// caller-controlled host/port on the API thread and surfaces the
// connector error verbatim. It only matters on multi-tier deployments
// where the API tier's network reach differs from the worker tier.
// Hardening (reject RFC1918 / link-local / 169.254.169.254 metadata IPs
// + sanitise the error) is tracked separately, not done here.

#[derive(Deserialize)]
pub struct DiscoverSourceRequest {
    /// Source kind — only introspectable kinds (`clickhouse`) are wired.
    pub kind: String,
    /// Live connector credentials (e.g. host/port/database/username/
    /// password/secure for ClickHouse). Not persisted.
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Serialize)]
pub struct DiscoverSourceResponse {
    pub tables: Vec<agentic_pipeline::DiscoveredTable>,
}

pub async fn discover_source_tables(
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Json(req): Json<DiscoverSourceRequest>,
) -> Response {
    match agentic_pipeline::airway_run::discover_airway_source_tables(req.kind, req.config).await {
        Ok(tables) => Json(DiscoverSourceResponse { tables }).into_response(),
        Err(AirwayRunError::Airway(e)) => {
            // Bad credentials / unreachable host / unsupported kind —
            // the caller's input, surfaced verbatim so the wizard can show it.
            (StatusCode::BAD_GATEWAY, format!("discovery failed: {e}")).into_response()
        }
        Err(e) => {
            tracing::warn!(%e, "discover_source_tables failed");
            (StatusCode::BAD_REQUEST, format!("discovery: {e}")).into_response()
        }
    }
}

// ── POST /agentic-airway/reset-schema ──────────────────────────────────────
//
// Drop a pipeline's destination tables and clear its stored
// `airway_pipeline_state` row (schema + incremental cursors) so a later run
// re-infers a fresh schema. Airhouse destinations only. Returns the dropped
// table names. Authed: this destroys ingested data, so it must not be open.

#[derive(Deserialize)]
pub struct ResetSchemaRequest {
    /// Path to a `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
}

#[derive(Serialize)]
pub struct ResetSchemaResponse {
    /// Tables dropped at the destination. Empty when the pipeline had never
    /// provisioned a schema (state is still cleared).
    pub dropped_tables: Vec<String>,
}

pub async fn reset_airway_schema(
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Json(req): Json<ResetSchemaRequest>,
) -> Response {
    // Reset only needs the platform (workspace + secret resolution) and the db;
    // `bare` single-sources the builder/automation knobs as `None`.
    let executor =
        agentic_pipeline::executor::PipelineTaskExecutor::bare(platform, state.db.clone());
    match executor.reset_airway_schema(&req.pipeline_ref).await {
        Ok(dropped_tables) => Json(ResetSchemaResponse { dropped_tables }).into_response(),
        Err(e) => {
            use agentic_pipeline::executor::ResetSchemaError;
            // Caller mistakes (bad ref / non-airhouse dest) → 400; a failed
            // destination drop or state delete is server-side → 500.
            // Status AND headers from one match, so the error→response mapping
            // is stated once. Re-testing the mapped status afterwards to attach
            // `Retry-After` split it across two places in one function.
            let (status, retry_after) = match &e {
                ResetSchemaError::BadRequest(_) => (StatusCode::BAD_REQUEST, None),
                ResetSchemaError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, None),
                // Matches the start path's answer for the same condition: the
                // request was fine, this node could not serve it yet — and half
                // the reason 503 beats 400 here is telling the caller *when*.
                ResetSchemaError::Unavailable(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Some(airway_unavailable_retry_after()),
                ),
            };
            tracing::warn!(
                error = %e,
                pipeline_ref = %req.pipeline_ref,
                status = status.as_u16(),
                "reset_airway_schema failed"
            );
            // `Retry-After` on the 503, matching the start path. Half the reason
            // 503 beats 400 for this condition is telling a retrying client
            // *when*; a 503 without it leaves that to the client's guess, and
            // the two airway routes would answer the same condition differently.
            match retry_after {
                Some(secs) => (
                    status,
                    [(axum::http::header::RETRY_AFTER, secs)],
                    e.to_string(),
                )
                    .into_response(),
                None => (status, e.to_string()).into_response(),
            }
        }
    }
}
