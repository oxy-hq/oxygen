//! Shared start helper for airway runs.
//!
//! Mirrors [`crate::automation_run::start_automation_run`]: seeds a fresh
//! `agentic_runs` row, populates the airway-specific extension row,
//! and enqueues a [`TaskSpec::Airway`] for the coordinator to claim.
//! HTTP, CLI, MCP, and eval all converge on this primitive.
//!
//! [`start_airway_run`] only seeds the DB; the runtime's existing
//! coordinator + worker pair drives the queued task to completion
//! through [`crate::executor::PipelineTaskExecutor::execute_airway`].

use std::sync::Arc;

use agentic_airway::AirwayPipelineSpec;
use agentic_airway::extension::run_extension;
use agentic_automation::WorkspaceContext;
use agentic_core::delegation::TaskSpec;
use agentic_core::hub_task::spawn_with_hub;
use agentic_core::transport::{CoordinatorTransport, WorkerTransport};
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::DurableTransport;
use agentic_runtime::worker::Worker;
use sea_orm::{DatabaseConnection, DbErr};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::executor::PipelineTaskExecutor;
use crate::platform::PlatformContext;

/// Inputs for [`start_airway_run`]. Doubles as the HTTP request body
/// for `POST /agentic-airway/runs`.
#[derive(Debug, Clone, Deserialize)]
pub struct StartAirwayRequest {
    /// Path to a `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Optional variables to render into the pipeline YAML at run
    /// time. Carried on the queue spec; not yet applied (templating
    /// is a follow-up).
    #[serde(default)]
    pub variables: Option<Value>,
    /// Conversation thread to associate this run with, if any.
    #[serde(default)]
    pub thread_id: Option<Uuid>,
    /// Explicit subset of resources (tables) to run, overriding the
    /// spec's `resources`. Caller-settable (HTTP/CLI) — used by "retry
    /// failed tables" to re-run only the streams that failed. Empty =
    /// run the whole spec.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Soft FK → `agentic_schedules.id`. Internal-only — only the scheduler
    /// fire path sets this; HTTP/CLI input cannot, so callers can't spoof
    /// which schedule a run "came from".
    #[serde(skip_deserializing, default)]
    pub schedule_id: Option<String>,
    /// How this run was triggered: `"scheduled"`, `"manual"`, `"backfill"`.
    /// Internal-only — stamped onto `agentic_runs.metadata.trigger`.
    #[serde(skip_deserializing, default)]
    pub trigger: Option<String>,
    /// The cron-scheduled time this run is replaying (UTC). Set by the
    /// backfill path; stamped onto `agentic_runs.metadata.logical_date`.
    #[serde(skip_deserializing, default)]
    pub logical_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Run id this run is a retry of. Set by `retry_run`; stamped onto
    /// `agentic_runs.metadata.retry_of`.
    #[serde(skip_deserializing, default)]
    pub retry_of: Option<String>,
    /// Bounded-backfill window `[from, to)` (RFC3339), set by the backfill
    /// endpoint. Threaded onto the queue spec and applied to the
    /// date-windowed sources (toast, quickbooks, sp_api) — the list lives in
    /// `executor::WINDOWED_BACKFILL_KINDS`. Internal-only — the public `/runs`
    /// body cannot set it (only `/backfill` does).
    #[serde(skip_deserializing, default)]
    pub backfill_from: Option<String>,
    #[serde(skip_deserializing, default)]
    pub backfill_to: Option<String>,
}

/// One row in the run-history list for a pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AirwayRunSummary {
    pub run_id: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
    /// Backfill window `[from, to)` (RFC3339) if this run was a backfill; `None`
    /// for a normal/incremental run. Lets the UI show which period a run covers
    /// instead of only its wall-clock timestamp.
    pub backfill_from: Option<String>,
    pub backfill_to: Option<String>,
}

#[derive(Debug, Error)]
pub enum AirwayRunError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("airway: {0}")]
    Airway(#[from] agentic_airway::AirwayError),
    #[error("database error: {0}")]
    Db(#[from] DbErr),
    #[error("io error reading airway spec: {0}")]
    Io(String),
    /// The pipeline's YAML could not be resolved **on this node** — the
    /// compile boundary could not be asked, or the revision has nothing
    /// compiled and this process holds no working copy.
    ///
    /// Distinct from [`InvalidInput`](Self::InvalidInput) because the caller
    /// did nothing wrong and retrying is the correct response: mid-deploy and
    /// not-yet-compiled are transient states, not bad requests. Callers should
    /// answer 503, not 400 (`agentic-http`'s airway route does).
    #[error("airway spec unavailable on this node: {0}")]
    Unavailable(String),
    /// A run this caller wanted to (re)drive is already in flight.
    ///
    /// No longer raised by `start_airway_run`: submit coalesces onto a queued
    /// run and the executor defers at claim, so contention never surfaces as
    /// an error. The remaining producer is the backfill re-drive path, where
    /// a chunk's PRIOR run is still queued or claimed by a worker.
    ///
    /// Carries the incumbent's `run_id` so callers can link to it instead of
    /// reporting a bare "busy" — the UI shows what is already running, and a
    /// scheduler can record *which* run it collapsed into.
    #[error("pipeline `{pipeline_name}` already has a run in flight ({run_id})")]
    AlreadyRunning {
        pipeline_name: String,
        run_id: String,
    },
}

/// Discover the tables a source exposes, for the pipeline-create UI.
///
/// Connects with the live credentials carried in `config` and returns
/// the tables (with columns) so the wizard can offer a picker instead
/// of making the user hand-type table names. Stateless: nothing is
/// persisted, and no DB/platform context is required.
pub async fn discover_airway_source_tables(
    kind: String,
    config: serde_json::Value,
) -> Result<Vec<agentic_airway::DiscoveredTable>, AirwayRunError> {
    let source = agentic_airway::config::SourceConfig { kind, config };
    Ok(agentic_airway::discover_source_tables(&source).await?)
}

/// Insert an `agentic_runs` row + `airway_run_extensions` row and
/// enqueue a [`TaskSpec::Airway`] for the coordinator to drive.
///
/// Returns the fresh `run_id`. The caller is responsible for any
/// runtime-state side effects (registering cancel watches, spawning
/// SSE subscribers); this function only touches the database.
/// `scope` records who will drive the seeded run: [`TaskScope::Scoped`] when
/// a co-located coordinator is spawned right after (every HTTP/CLI caller
/// today), [`TaskScope::Global`] when the Phase 2 scheduler seeds it for the
/// standalone/recovery loop to pick up.
pub async fn start_airway_run(
    db: &DatabaseConnection,
    workspace: &dyn WorkspaceContext,
    request: StartAirwayRequest,
    scope: crud::TaskScope,
    workspace_id: Uuid,
) -> Result<String, AirwayRunError> {
    if request.pipeline_ref.trim().is_empty() {
        return Err(AirwayRunError::InvalidInput(
            "pipeline_ref must not be empty".into(),
        ));
    }

    // Resolve + render + parse the spec up front so the user gets a
    // clear error at submit time rather than from a queued worker
    // failure later. The same `variables` ride the queue spec so the
    // worker renders an identical document at run time.
    // `load_pipeline_yaml` reads the compiled `airway_pipelines` row when the
    // host serves the compile boundary and only then the workspace FS, and
    // contains `pipeline_ref` (untrusted HTTP input) on both paths: reject
    // absolute/`..`/empty + canonical-containment. Errors quote only the ref,
    // never the resolved absolute path.
    let yaml = crate::pipeline_ref::load_pipeline_yaml(workspace, &request.pipeline_ref)
        .await
        .map_err(|e| match e {
            crate::pipeline_ref::PipelineRefError::Invalid(m) => AirwayRunError::InvalidInput(m),
            crate::pipeline_ref::PipelineRefError::Io(m) => AirwayRunError::Io(m),
            crate::pipeline_ref::PipelineRefError::Unavailable(m) => AirwayRunError::Unavailable(m),
        })?;
    let spec = AirwayPipelineSpec::from_yaml_with_vars(&yaml, request.variables.as_ref())?;

    let run_id = Uuid::new_v4().to_string();

    // SINGLE-FLIGHT IS ENFORCED AT CLAIM, NOT HERE.
    //
    // Submit used to take the lease and refuse a contended caller with a 409.
    // That protected only the callers that go through submit — an inline
    // `TaskSpec::Airway` step never did, so two workflows could run one
    // pipeline concurrently and the invariant was never actually enforced. It
    // also held the lease across queue time, so a run that failed before
    // executing blocked its pipeline for the full TTL.
    //
    // The executor now acquires at claim time and defers when contended, which
    // makes the executor the only door. Submit's remaining job is to not
    // create redundant work.
    //
    // COALESCE: if a run for this pipeline is already queued and has not
    // started, return it instead of enqueuing a second. Without this,
    // "queue instead of reject" turns ten clicks into ten identical runs —
    // the backlog problem that motivated rejecting in the first place.
    // Coalescing is what makes queueing safe; it is not optional polish.
    //
    // Deliberately NOT coalescing backfills: two backfill requests for
    // different windows are different work, and folding them would silently
    // drop one. Only plain runs (no window, matching variables) coalesce.
    if !spec.allow_concurrent_runs
        && request.backfill_from.is_none()
        && request.backfill_to.is_none()
        && let Some(existing) = find_coalescible_run(
            db,
            workspace_id,
            &spec.name,
            request.variables.as_ref(),
            &request.resources,
        )
        .await?
    {
        tracing::info!(
            pipeline = %spec.name,
            existing_run = %existing,
            "airway run coalesced onto an already-queued run"
        );
        return Ok(existing);
    }
    if spec.allow_concurrent_runs {
        tracing::warn!(
            pipeline = %spec.name,
            "airway single-flight DISABLED for this pipeline (allow_concurrent_runs: true)"
        );
    }

    // Lineage labels stamped at run-start so the dashboard can label
    // the Source / Destination cards even before `pipeline_plan` fires
    // (or for legacy runs that predated it). `source_kind` is the
    // connector kind from the YAML (e.g. `"postgres_cdc"` / `"stripe"`).
    // `destination_label` reads either the referenced database name
    // (most common — users write `destination: { database: foo, ... }`)
    // or the inline connector kind (test fixtures / pre-resolved specs).
    let source_kind = spec.source.kind.clone();
    let destination_label = match &spec.destination {
        agentic_airway::config::DestinationSpec::Reference(r) => r.database.clone(),
        agentic_airway::config::DestinationSpec::Inline(c) => c.kind.clone(),
    };

    // Resolved here, before any row exists, so a resolver failure is a true
    // no-op — no orphaned `agentic_runs` / `airway_run_extensions` row left
    // behind with no queue entry to ever execute it. The queued row then
    // records which policy admitted this run, which is what makes it
    // explainable after the config changes later — see the design doc's
    // "Resolution happens at enqueue". The cost is that a queued backlog
    // keeps the policy it was submitted under.
    // `AirwayRunError::Db` is `#[from] DbErr`, so `?` converts directly.
    let admission = crate::airway_config::resolve_admission(db, &source_kind, workspace_id).await?;

    let mut metadata = serde_json::json!({
        "pipeline_ref": request.pipeline_ref,
        "pipeline_name": spec.name,
        "concurrency": spec.concurrency,
        "source_kind": source_kind,
        "destination_label": destination_label,
        // `null` when omitted; the executor passes whatever lands here
        // through to the worker on resume.
        "variables": request.variables,
        // Subset of tables this run targeted (empty = whole spec). Persisted
        // so a retry of this run reproduces the same scope instead of
        // silently widening a "retry failed tables" run back to the full
        // pipeline — see `retry_airway`.
        "resources": request.resources.clone(),
        // Backfill window for observability in the run history (null for
        // normal runs). The effective window still rides the queue spec below.
        "backfill_from": request.backfill_from.clone(),
        "backfill_to": request.backfill_to.clone(),
    });
    crate::scheduler::stamp_trigger_metadata(
        &mut metadata,
        &request.trigger,
        &request.logical_date,
        &request.retry_of,
    );

    // Submit takes no lease — the executor acquires at claim — so there is
    // nothing to release if seeding fails, and no cleanup to funnel.
    seed_airway_run_rows(
        db,
        &run_id,
        &spec,
        &metadata_and_request(metadata, request),
        scope,
        workspace_id,
        &admission,
    )
    .await?;

    Ok(run_id)
}

/// The request fields `seed_airway_run_rows` needs, bundled so the seeding
/// helper stays under the argument limit and the caller keeps one owned value
/// to move.
struct SeedInputs {
    metadata: serde_json::Value,
    request: StartAirwayRequest,
}

fn metadata_and_request(metadata: serde_json::Value, request: StartAirwayRequest) -> SeedInputs {
    SeedInputs { metadata, request }
}

/// Insert the run row, the airway extension row, and the queue task.
///
/// Split out of [`start_airway_run`] purely so the single-flight lease has one
/// error path to unwind rather than four.
async fn seed_airway_run_rows(
    db: &DatabaseConnection,
    run_id: &str,
    spec: &AirwayPipelineSpec,
    inputs: &SeedInputs,
    scope: crud::TaskScope,
    workspace_id: Uuid,
    // Resolved once by the caller, before any row exists, and passed in rather
    // than re-resolved here: both writes below must record the same admission,
    // and a second resolution could read a config row edited in between.
    admission: &crate::airway_config::ResolvedAdmission,
) -> Result<(), AirwayRunError> {
    let run_id = run_id.to_string();
    let metadata = inputs.metadata.clone();
    let request = &inputs.request;

    let question = format!("airway: {}", spec.name);
    if let Some(schedule_id) = request.schedule_id.as_deref() {
        crud::insert_run_with_schedule(
            db,
            &run_id,
            &question,
            request.thread_id,
            agentic_airway::SOURCE_TYPE,
            Some(metadata),
            schedule_id,
            workspace_id,
        )
        .await?;
    } else {
        crud::insert_run(
            db,
            &run_id,
            &question,
            request.thread_id,
            agentic_airway::SOURCE_TYPE,
            Some(metadata),
            workspace_id,
        )
        .await?;
    }

    // Same `admission` binding used below for the queued spec — borrowed here
    // (not moved) so both writes carry the one resolution, never two.
    run_extension::insert_run_extension(
        db,
        &run_id,
        &spec,
        Some(&request.pipeline_ref),
        admission.contract_policy.as_deref(),
        admission.environment.as_deref(),
    )
    .await?;

    let task_spec = TaskSpec::Airway {
        pipeline_ref: request.pipeline_ref.clone(),
        variables: request.variables.clone(),
        resources: request.resources.clone(),
        backfill_from: request.backfill_from.clone(),
        backfill_to: request.backfill_to.clone(),
        contract_policy: admission.contract_policy.clone(),
        environment: admission.environment.clone(),
    };
    crud::enqueue_task(db, &run_id, &run_id, None, &task_spec, None, scope).await?;

    Ok(())
}

/// Release the single-flight lease held by `run_id`, if any.
///
/// Exists so transport-layer terminal paths can free the lease without
/// importing `agentic-airway` — `agentic-http` may only enter agentic through
/// this facade, so a direct domain import there would break the layering rule.
///
/// Idempotent: a `run_id`-scoped DELETE, so calling it on a run that already
/// released is a no-op rather than freeing a successor's lease.
pub async fn release_airway_lease(db: &DatabaseConnection, run_id: &str) {
    if let Err(e) = agentic_airway::extension::pipeline_lease::release_by_run(db, run_id).await {
        tracing::warn!(%run_id, error = %e,
            "failed to release the airway single-flight lease; it will lapse at expires_at");
    }
}

/// Run-scoped release for the operator CLI, reporting rows removed.
///
/// Guarded on `run_id` on purpose — see
/// `agentic_airway::extension::pipeline_lease::release_counted`. The unguarded
/// [`force_release_airway_lease`] stays available for `--force`.
pub async fn release_airway_lease_scoped(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    pipeline_name: &str,
    run_id: &str,
) -> Result<u64, sea_orm::DbErr> {
    agentic_airway::extension::pipeline_lease::release_counted(
        db,
        workspace_id,
        pipeline_name,
        run_id,
    )
    .await
}

/// Release the lease for `run_id` **only if its task was never claimed**.
///
/// The cancel handler terminalizes a run without `drive` ever returning, so it
/// must free the lease — but `request_cancel` is polled, so on a replica that
/// is NOT driving, the worker may still be mid-fold when the cancel lands.
/// Releasing there lets a scheduler tick or `POST /runs` start a second run
/// alongside one that is still writing: precisely the overlap the lease exists
/// to prevent, created by the cleanup for it.
///
/// `claimed_at.is_none()` is the safe subset — a queued-but-unclaimed run has
/// no worker to race. The cross-replica case is left to the worker's own
/// release at the tail of `drive` (and to the TTL if that worker dies), which
/// is later but never wrong.
///
/// The `Ok(None)` branch (no queue row) holds two different situations, and
/// only one has a worker to defer to. A row reaped from under a LIVE worker
/// does — that is why this branch does not release. A **reaper dead-letter**
/// does not: no row, no worker, `drive` never returns. Cancel used to clear
/// that lease and deliberately no longer does, because the two are
/// indistinguishable from here and releasing the wrong one admits a concurrent
/// run. The recourse for a dead-lettered run is
/// `oxy airway release-lease <pipeline> --workspace-id <id>`, or waiting out
/// the TTL — not this handler.
pub async fn release_airway_lease_if_unclaimed(db: &DatabaseConnection, run_id: &str) {
    match agentic_runtime::crud::get_queue_entry(db, run_id).await {
        Ok(Some(entry)) if entry.claimed_at.is_some() => {
            tracing::debug!(
                %run_id,
                "airway cancel: task is claimed; leaving the lease to the driving worker"
            );
        }
        // Never claimed — no worker to race, so releasing is safe.
        Ok(Some(_)) => release_airway_lease(db, run_id).await,
        // No queue row at all. Distinct from "never claimed": the task may have
        // been reaped or completed, and a worker could still be finishing its
        // fold with the row already gone. Folding this into the safe case was
        // the same over-eager release finding E fixed, one branch along.
        // Leave it to the worker's release, or the TTL.
        Ok(None) => tracing::debug!(
            %run_id,
            "airway cancel: no queue row; leaving the lease to the worker or the TTL"
        ),
        Err(e) => {
            // Don't guess: leaving it costs at most the TTL, releasing wrongly
            // costs a concurrent run.
            tracing::warn!(%run_id, error = %e,
                "airway cancel: could not read queue state; leaving the lease to lapse");
        }
    }
}

/// List every single-flight lease held in `workspace_id`.
///
/// Facade re-export so the CLI can answer "why won't this pipeline start?"
/// without importing the airway domain crate directly.
pub async fn list_airway_leases(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<agentic_airway::extension::pipeline_lease::Model>, DbErr> {
    agentic_airway::extension::pipeline_lease::list_for_workspace(db, workspace_id).await
}

/// Force-release a pipeline's lease. Returns rows removed.
///
/// See [`agentic_airway::extension::pipeline_lease::force_release`] for the risk
/// this accepts — callers must show the holder and confirm before calling.
pub async fn force_release_airway_lease(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<u64, DbErr> {
    agentic_airway::extension::pipeline_lease::force_release(db, workspace_id, pipeline_name).await
}

/// Spawn the coordinator + worker pair that drives a queued airway run
/// to completion.
///
/// Pairs with [`start_airway_run`]. Airway is a single atomic task —
/// no child delegation, no decision chain — so unlike
/// `spawn_automation_run_drive` this attaches no completion policy or
/// delegation resolver: the default coordinator terminates the run
/// when the root `TaskSpec::Airway` returns `Done` / `Failed`.
///
/// The transport is scoped to this run's id so the worker can't poach
/// a sibling run's queued root task (same race the automation drive
/// guards against).
pub fn spawn_airway_run_drive(
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    run_id: String,
    platform: Arc<dyn PlatformContext>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
) {
    let executor = Arc::new(PipelineTaskExecutor {
        platform,
        builder_bridges: None,
        schema_cache: None,
        builder_test_runner: None,
        builder_app_runner: None,
        db: db.clone(),
        state: Some(state.clone()),
        custom_executors: None,
    });

    let transport = DurableTransport::with_router(db.clone(), router, Some(run_id.clone()));

    let mut coordinator = Coordinator::new(
        db,
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
    );
    // `register_root` (not `submit_root`) — the queue row already
    // exists from `start_airway_run`'s `enqueue_task`.
    coordinator.register_root(run_id.clone(), 0);

    let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);

    // Forward `state.cancel(run_id)` into the transport so the worker's
    // in-flight `CancellationToken` fires and the queued row is marked
    // cancelled. The coordinator's `handle_cancelled` then propagates
    // to the run row.
    let transport_for_cancel = transport.clone();
    let cancel_task_id = run_id.clone();
    let cancel_forwarder = spawn_with_hub(async move {
        while cancel_rx.changed().await.is_ok() {
            if *cancel_rx.borrow() {
                let _ = transport_for_cancel.cancel_subtree(&cancel_task_id).await;
                break;
            }
        }
    });

    let worker_task = spawn_with_hub(async move {
        worker.run().await;
    });
    let cleanup_run_id = run_id.clone();
    let cleanup_state = state;
    spawn_with_hub(async move {
        let mut coord = coordinator;
        coord.run().await;
        // Close the SSE stream cleanly once the run terminates —
        // mirrors the automation drive's shutdown sequence.
        cleanup_state.notify(&cleanup_run_id);
        cancel_forwarder.abort();
        worker_task.abort();
        cleanup_state.deregister(&cleanup_run_id);
    });
}

/// Find a run of this pipeline that is already queued and has not started, so
/// a fresh request can ride on it instead of enqueuing duplicate work.
///
/// Keyed on `metadata->>'pipeline_name'` — the SAME key the single-flight
/// lease uses — not on `pipeline_ref`. Two refs can resolve to one pipeline
/// name, and coalescing on the ref would let them both through to contend at
/// claim time, which is the contention this is meant to avoid.
///
/// BEST EFFORT, not a guarantee: this is a read followed by an insert with no
/// unique constraint behind it, so two genuinely simultaneous submits can still
/// create two runs. That is acceptable under this design — they serialize at
/// claim — but it is not mutual exclusion.
///
/// "Has not started" is `queue_status = 'queued'`, which also covers a task
/// deferred while waiting for the lease. Coalescing onto a deferred run is
/// correct: it is the same pending work, and the caller wants it done once.
async fn find_coalescible_run(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    pipeline_name: &str,
    variables: Option<&Value>,
    requested_resources: &[String],
) -> Result<Option<String>, AirwayRunError> {
    use sea_orm::{FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        variables: Option<Value>,
        resources: Option<Value>,
    }

    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"
        SELECT r.id, r.metadata->'variables' AS variables,
               r.metadata->'resources' AS resources
        FROM agentic_runs r
        JOIN agentic_task_queue q
          ON q.run_id = r.id AND q.parent_task_id IS NULL
        WHERE r.source_type = $1
          AND r.parent_run_id IS NULL
          AND r.workspace_id = $2
          AND r.metadata->>'pipeline_name' = $3
          AND r.metadata->>'backfill_from' IS NULL
          AND q.queue_status = 'queued'
        ORDER BY r.created_at
        LIMIT 1
        "#,
        [
            agentic_airway::SOURCE_TYPE.into(),
            workspace_id.into(),
            pipeline_name.into(),
        ],
    );

    let Some(row) = Row::find_by_statement(stmt).one(db).await? else {
        return Ok(None);
    };
    // Different variables render a different document, so they are different
    // work. Compare rather than assume — a coalesce that ignored them would
    // silently serve one caller another caller's parameters.
    let existing_vars = row.variables.filter(|v| !v.is_null());
    if existing_vars.as_ref() != variables {
        return Ok(None);
    }
    // `resources` is the same class of parameter and must match too. A "retry
    // failed tables" run carries a RESTRICTED scope; folding a full-pipeline
    // submit onto it would silently skip the full load and report success.
    // `[]` and absent both mean "the whole spec", so normalise before
    // comparing.
    fn scope(v: Option<Value>) -> Vec<String> {
        v.and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default()
    }
    if scope(row.resources) != requested_resources {
        return Ok(None);
    }
    Ok(Some(row.id))
}

/// List recent airway runs for a `pipeline_ref` in `workspace_id`, newest
/// first, capped at `limit`. Backs the run-history dropdown.
///
/// Filters `agentic_runs` on `source_type = 'airway'`, the `workspace_id`
/// and the `metadata->>'pipeline_ref'` that `start_airway_run` stamps — no
/// join needed since the ref lives in the run row's metadata.
///
/// `workspace_id` is not optional. A ref is a workspace-relative path
/// (`pipelines/toast.airway.yml`), so the same ref names a different pipeline
/// in every workspace that has one; filtering on the ref alone handed each
/// workspace every other workspace's run history for that path. Pass the same
/// id `start_airway_run` stamps — `PlatformContext::workspace_id()`, which is
/// the nil UUID in local mode — so the two agree in every serve mode.
pub async fn list_airway_runs(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_ref: &str,
    limit: u64,
) -> Result<Vec<AirwayRunSummary>, AirwayRunError> {
    use sea_orm::{FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        task_status: Option<String>,
        created_at: chrono::DateTime<chrono::FixedOffset>,
        updated_at: chrono::DateTime<chrono::FixedOffset>,
        backfill_from: Option<String>,
        backfill_to: Option<String>,
    }

    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"
        SELECT id, task_status, created_at, updated_at,
               metadata->>'backfill_from' AS backfill_from,
               metadata->>'backfill_to'   AS backfill_to
        FROM agentic_runs
        WHERE source_type = $1
          AND parent_run_id IS NULL
          AND workspace_id = $2
          AND metadata->>'pipeline_ref' = $3
        ORDER BY created_at DESC
        LIMIT $4
        "#,
        [
            agentic_airway::SOURCE_TYPE.into(),
            workspace_id.into(),
            pipeline_ref.into(),
            (limit as i64).into(),
        ],
    );

    let rows = Row::find_by_statement(stmt).all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| AirwayRunSummary {
            run_id: r.id,
            status: r.task_status.unwrap_or_else(|| "unknown".into()),
            created_at: r.created_at,
            updated_at: r.updated_at,
            backfill_from: r.backfill_from,
            backfill_to: r.backfill_to,
        })
        .collect())
}

// Happy-path coverage lives in the testcontainers suite alongside the
// worker's integration tests — `start_airway_run` is mostly persistence
// (one runtime row + one extension row + one queue row) and the
// validation surface is a single empty-string check.

#[cfg(test)]
mod timeout_ordering_tests {
    /// An **absolute** gap rather than a ratio, deliberately. A ratio has to be
    /// re-justified every time either constant moves, and "at most half the
    /// lease" would rule out values well clear of it for no stated reason —
    /// which it did on the first draft of this test, rejecting the 4h the data
    /// supports purely because 4h is more than half of 6h. What the ordering
    /// actually needs is that the two events are far enough apart to be
    /// distinguishable in an incident, and an hour is comfortably that.
    const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

    /// The coordinator's delegation ceiling must expire BEFORE airway's
    /// single-flight lease.
    ///
    /// Strictly implied by the margin test below — kept for the distinct
    /// failure message, since "past the lease entirely" and "too close to it"
    /// are different mistakes to make and worth reading differently. Only one
    /// of the two can ever fail alone.
    ///
    /// Both bound the same stuck pipeline, and which one fires first decides
    /// what an operator sees. Coordinator first: the delegating task is failed
    /// with "delegation timed out" naming its children, and the airway lease is
    /// released on the way out. Lease first: the run simply looks slow for up to
    /// six hours, and the thing that eventually frees it is a TTL backstop that
    /// says nothing about which step hung.
    ///
    /// This is asserted here rather than in `agentic-runtime` because the
    /// runtime layer must not depend on a domain crate — `agentic-pipeline` is
    /// the lowest layer that can see both constants. Raising
    /// `DEFAULT_SUSPEND_TIMEOUT` past the lease TTL silently inverts the
    /// documented behaviour, which is exactly the kind of change a comment
    /// cannot stop.
    #[test]
    fn suspend_timeout_expires_before_the_airway_lease() {
        let suspend = agentic_runtime::orchestrator::coordinator::DEFAULT_SUSPEND_TIMEOUT;
        let lease = std::time::Duration::from_secs(
            agentic_airway::extension::pipeline_lease::LEASE_TTL_SECS as u64,
        );
        assert!(
            suspend < lease,
            "DEFAULT_SUSPEND_TIMEOUT ({suspend:?}) must stay under airway's \
             LEASE_TTL_SECS ({lease:?}) so a hung delegated pipeline surfaces as \
             a named timeout rather than waiting out the lease"
        );
    }

    /// …and with enough margin to be worth having, not one second under.
    ///
    /// Guards the useful half of the ordering: a value at 5h59m would satisfy
    /// the assertion above while leaving the two effectively simultaneous, so
    /// an operator could not tell which bound fired.
    #[test]
    fn suspend_timeout_leaves_real_margin_under_the_lease() {
        let suspend = agentic_runtime::orchestrator::coordinator::DEFAULT_SUSPEND_TIMEOUT;
        let lease = std::time::Duration::from_secs(
            agentic_airway::extension::pipeline_lease::LEASE_TTL_SECS as u64,
        );
        assert!(
            suspend + MIN_GAP <= lease,
            "DEFAULT_SUSPEND_TIMEOUT ({suspend:?}) must clear LEASE_TTL_SECS \
             ({lease:?}) by at least {MIN_GAP:?}; closer than that and the \
             coordinator's timeout stops being a distinguishable signal"
        );
    }
}
