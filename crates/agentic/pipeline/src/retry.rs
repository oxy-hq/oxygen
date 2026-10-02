//! Retry path for a terminal-failed run.
//!
//! - **airway** runs retry **in place**: the run's existing queued task is
//!   revived and the run flipped back to `running` under the SAME `run_id`, so
//!   the coordinator re-drives it and the worker resumes from its persisted
//!   cursor (`airway_run_extensions.resume_state`). Only if the task row was
//!   reaped does it fall back to clone-and-reseed.
//! - **workflow** (automation) runs still **clone-and-reseed**: a brand-new run
//!   is seeded with the same target/variables/`schedule_id`, tagged
//!   `trigger="retry"` + `metadata.retry_of=<original>` so the dashboard links
//!   them.
//!
//! Only `workflow` and `airway` source types are retryable in v1 — analytics
//! and builder runs need per-domain reconstruction that doesn't exist yet (the
//! original interactive context is lost the moment the original session ends).

use sea_orm::{DatabaseConnection, DbErr};

use agentic_runtime::crud::{TaskScope, get_run_in_workspace};
use agentic_runtime::entity::run;

use crate::airway_run::{StartAirwayRequest, start_airway_run};
use crate::automation_run::{StartAutomationRequest, start_automation_run};

#[derive(Debug)]
pub enum RetryError {
    /// The originating run id doesn't resolve.
    NotFound,
    /// The original run isn't in a state we can retry from, or its source
    /// type lacks a reconstruction path.
    NotRetryable(String),
    /// Reconstructing succeeded but seeding the new run failed.
    SeedFailed(String),
    /// A single-flight refusal: the pipeline already has a run in flight, or
    /// this run's task is still queued/claimed by a worker. Caller state, NOT a
    /// server fault — mapped to 409, matching the contract the HTTP start path
    /// already follows. Routing these through `SeedFailed` made a refused retry
    /// present as a 500, and the cross-replica-cancel path makes that the
    /// COMMON outcome, so it would page.
    Conflict(String),
    Db(DbErr),
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryError::NotFound => write!(f, "run not found"),
            RetryError::NotRetryable(m) => write!(f, "{m}"),
            RetryError::SeedFailed(m) => write!(f, "seed failed: {m}"),
            // No "failed:" prefix — this is a refusal, not a failure.
            RetryError::Conflict(m) => write!(f, "{m}"),
            RetryError::Db(e) => write!(f, "db error: {e}"),
        }
    }
}

impl From<DbErr> for RetryError {
    fn from(e: DbErr) -> Self {
        RetryError::Db(e)
    }
}

/// Clone `run_id` into a fresh run. Returns the new `run_id`.
///
/// **Workspace-scoped**: a foreign run id is reported as `NotFound`
/// rather than `NotRetryable`, so the surface mirrors `get_schedule`'s
/// cross-workspace handling — the existence of a run in another tenant
/// isn't probeable. A check run staff queued in a non-production app
/// environment is `NotFound` the same way (`get_run_in_workspace`).
///
/// `workspace` is needed for the airway path (it resolves the pipeline file
/// off the workspace filesystem); automation retries don't actually read it
/// but the signature stays uniform with `run_schedule_now`.
pub async fn retry_run(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    run_id: &str,
) -> Result<String, RetryError> {
    let original = get_run_in_workspace(db, workspace_id, run_id)
        .await?
        .ok_or(RetryError::NotFound)?;

    if !is_terminal_failed(&original) {
        return Err(RetryError::NotRetryable(format!(
            "run is {:?}; only failed / cancelled / timed_out runs can be retried",
            original.task_status.as_deref().unwrap_or("(none)")
        )));
    }

    let source = original.source_type.as_deref().unwrap_or("");
    match source {
        "workflow" => retry_automation(db, &original).await,
        "airway" => retry_airway(db, workspace, &original).await,
        other => Err(RetryError::NotRetryable(format!(
            "retry not supported for source_type {other:?}"
        ))),
    }
}

fn is_terminal_failed(r: &run::Model) -> bool {
    matches!(
        r.task_status.as_deref(),
        Some("failed") | Some("cancelled") | Some("timed_out")
    )
}

async fn retry_automation(
    db: &DatabaseConnection,
    original: &run::Model,
) -> Result<String, RetryError> {
    let metadata = original.metadata.as_ref();
    let workflow_ref = metadata
        .and_then(|m| m.get("workflow_ref"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RetryError::NotRetryable("workflow_ref missing on original run".into()))?
        .to_string();
    let variables = metadata
        .and_then(|m| m.get("variables"))
        .filter(|v| !v.is_null())
        .cloned();

    let req = StartAutomationRequest {
        workflow_ref,
        variables,
        retry_from_run_id: None,
        cache_enabled: false,
        invalidate_steps: None,
        invalidate_iterations: None,
        thread_id: None,
        schedule_id: original.schedule_id.clone(),
        trigger: Some("retry".to_string()),
        logical_date: None,
        retry_of: Some(original.id.clone()),
    };
    start_automation_run(db, req, TaskScope::Global, original.workspace_id)
        .await
        .map_err(|e| RetryError::SeedFailed(e.to_string()))
}

/// Outcome of the reset-in-place attempt. `StillLive` exists so the "a worker
/// holds this run" bail can return without releasing — routing it through
/// `Err` would hit the release-on-error arm and free that worker's lease.
enum Reset {
    /// Reset-in-place succeeded; re-drive this run id.
    Reused(String),
    /// Task row is gone — safe to release and clone-reseed.
    Reaped,
    /// Task is queued/claimed: a worker is driving it. Do not release.
    StillLive,
}

async fn retry_airway(
    db: &DatabaseConnection,
    workspace: &dyn crate::WorkflowWorkspaceContext,
    original: &run::Model,
) -> Result<String, RetryError> {
    // ── Reset-in-place (the normal path) ─────────────────────────────────────
    // Revive the run's existing queued task (keeping its stored spec) and flip
    // the run back to `running`, so the coordinator re-drives the SAME run_id —
    // no new run row. The per-run cursor (`airway_run_extensions.resume_state`)
    // lets the worker resume where it left off. `0` rows means the task was
    // reaped (or is still live — the guard skips live tasks), so we fall through
    // to a clone-reseed.
    let run_id = original.id.clone();

    // Re-take the single-flight lease BEFORE reviving the task. The lease for
    // this run_id was deleted when the failed attempt terminalized, so without
    // this the retried load runs completely unguarded — and "a load failed, is
    // retried, and the next cron slot fires mid-retry" is the most plausible
    // real-world route to the very overlap this lease exists to prevent.
    //
    // Re-acquiring under the SAME run_id is deliberate: it keeps the lease's
    // identity aligned with the run the worker will release on, so the existing
    // `release_by_run` at the end of `drive` still frees exactly this lease.
    let pipeline_name = original
        .metadata
        .as_ref()
        .and_then(|m| m.get("pipeline_name"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if let Some(pipeline_name) = pipeline_name {
        use agentic_airway::extension::pipeline_lease::{
            LEASE_TTL_SECS, LeaseAcquisition, try_acquire,
        };
        match try_acquire(
            db,
            original.workspace_id,
            &pipeline_name,
            &run_id,
            LEASE_TTL_SECS,
        )
        .await
        .map_err(|e| RetryError::SeedFailed(e.to_string()))?
        {
            LeaseAcquisition::Acquired => {}
            LeaseAcquisition::Held { run_id: holder, .. } => {
                return Err(RetryError::Conflict(format!(
                    "pipeline `{pipeline_name}` already has a run in flight ({holder}); \
                     retry once it finishes"
                )));
            }
        }
    }

    // Everything from here to the reap fallback runs under the lease, so every
    // exit must release it. One block with plain `?` inside and a single
    // release-on-`Err`, rather than a release at each call site: a `?` is an
    // invisible early return, and two prior rounds of hand-rolled per-site
    // releases each shipped with one more site missed. This shape cannot be
    // partially applied.
    //
    // THREE outcomes, not two. The previous round bailed on "still live" by
    // returning `Err`, which routes through the release-on-`Err` arm below —
    // so the code correctly refused to reseed and then deleted the LIVE run's
    // lease on the way out, re-opening the overlap one step later. A bail that
    // must not release cannot be an `Err` here.
    let guarded = async {
        let reset = agentic_runtime::crud::reset_task_to_queued(db, &run_id).await?;
        if reset == 0 {
            // 0 rows means `queue_status NOT IN ('queued','claimed')` matched
            // nothing — which is TWO states, not one: the row is gone (reaped),
            // or it is queued/claimed and a worker is or will be driving this
            // exact run. Treating them alike releases the lease and clone-
            // reseeds alongside a live worker: two concurrent runs of one
            // pipeline, reached through this guard's own re-acquire path.
            //
            // Reachable without anything exotic — a cross-replica cancel marks
            // the run failed while the worker keeps folding, `is_terminal_failed`
            // then passes, and the retry lands here on a still-claimed row.
            // Fails CLOSED: a DB error here is treated as "live", because the
            // cost of guessing wrong is releasing a running worker's lease and
            // starting a second run, while the cost of a false positive is one
            // refused retry.
            let live = match agentic_runtime::crud::get_queue_entry(db, &run_id).await {
                Ok(entry) => entry.is_some(),
                Err(e) => {
                    tracing::warn!(%run_id, error = %e,
                        "airway retry: queue-state read failed; assuming the run is live");
                    true
                }
            };
            return Ok::<Reset, RetryError>(if live {
                Reset::StillLive
            } else {
                Reset::Reaped
            });
        }
        // Drop the failed attempt's events + clear its terminal error so the run
        // shows a clean re-run, not a stale failure, then flip it to running.
        if let Err(e) = agentic_runtime::crud::delete_events_from_seq(db, &run_id, 0).await {
            tracing::warn!(%run_id, error = %e, "airway retry: clearing prior events failed");
        }
        agentic_runtime::crud::reset_run_for_retry(db, &run_id).await?;
        // A backfill chunk's task is `scope_owned` (driven under its range's
        // scope); the global coordinator's `claim_task` only picks up
        // `scope_owned = false`, so a reset-in-place retry would never be
        // re-driven. Make it global so the coordinator re-drives it. No-op for a
        // normal (already-global) run.
        agentic_runtime::crud::mark_task_global(db, &run_id).await?;
        // Best-effort — the retry succeeding matters more than the counter.
        if let Err(e) =
            agentic_airway::extension::run_extension::increment_retry_count(db, &run_id).await
        {
            tracing::warn!(%run_id, error = %e, "airway retry: retry_count bump failed");
        }
        Ok(Reset::Reused(run_id.clone()))
    }
    .await;

    match guarded {
        Ok(Reset::Reused(id)) => return Ok(id),
        // Reaped — fall through; the lease is released just below.
        Ok(Reset::Reaped) => {}
        // A worker holds this run. Return WITHOUT releasing: the lease is that
        // worker's, and freeing it is exactly the overlap being prevented.
        Ok(Reset::StillLive) => {
            return Err(RetryError::Conflict(format!(
                "run {run_id} is still queued or claimed by a worker; \
                 cancel it and wait for the worker to finish before retrying"
            )));
        }
        Err(e) => {
            crate::airway_run::release_airway_lease(db, &run_id).await;
            return Err(e);
        }
    }

    // ── Reap fallback: the task row is gone → clone-and-reseed a fresh run. ───
    //
    // Release the lease taken above FIRST. It is held under the original
    // `run_id`, but this path abandons that run and seeds a new one — whose
    // `start_airway_run` takes its own lease. Without this the old lease is
    // stranded under a run id nothing will ever release, and the pipeline
    // stalls for the full 6h TTL: a silent block reachable from one Retry
    // click, and `start_airway_run` below would immediately fail against the
    // very lease this function just took.
    crate::airway_run::release_airway_lease(db, &run_id).await;

    let metadata = original.metadata.as_ref();
    let pipeline_ref = metadata
        .and_then(|m| m.get("pipeline_ref"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RetryError::NotRetryable("pipeline_ref missing on original run".into()))?
        .to_string();
    let variables = metadata
        .and_then(|m| m.get("variables"))
        .filter(|v| !v.is_null())
        .cloned();
    // Reproduce the original run's table scope. Absent on pre-existing runs
    // (treated as a full-spec run); empty for a run that already targeted the
    // whole spec.
    let resources = metadata
        .and_then(|m| m.get("resources"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let req = StartAirwayRequest {
        pipeline_ref,
        variables,
        thread_id: None,
        resources,
        schedule_id: original.schedule_id.clone(),
        trigger: Some("retry".to_string()),
        logical_date: None,
        retry_of: Some(original.id.clone()),
        // A retry re-runs as a normal incremental; the original backfill
        // window is not re-applied (the source's cursor was frozen during the
        // backfill, so the live position is unchanged). Re-backfill explicitly
        // via /backfill if a bounded replay is wanted again.
        backfill_from: None,
        backfill_to: None,
    };
    start_airway_run(db, workspace, req, TaskScope::Global, original.workspace_id)
        .await
        .map_err(|e| RetryError::SeedFailed(e.to_string()))
}
