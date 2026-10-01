//! Procedure dry runs: staff run a procedure of a previewed branch; it runs on
//! the worker fleet against the branch's staging revision, reads and agent
//! steps run for real, managed-Airhouse writes land in the preview's own
//! schemas (phase 2b), and every other write is held and reported as "would
//! have written". A change check queues the same kind of run for each
//! pure-Airhouse transform the branch changed (`transform_build`), which the
//! same queue starts; its compare with live is `previews::compare`. An Airway
//! sample (`airway_sample`, phase 2b S11) is a bounded window of a branch's
//! pipeline into the preview's own schemas, queued on the same queue and run
//! by `previews::sample`.
//!
//! Only rows are written here — nothing runs in a handler:
//!
//! 1. [`submit`] records a `workspace_preview_runs` row (`queued`) and calls
//!    [`advance`].
//! 2. [`advance`] starts the oldest queued run of a workspace when none is
//!    running, and seeds its `agentic_runs` row and root task
//!    (`agentic_pipeline::automation_run::seed_preview_automation_run`, or
//!    `previews::sample::seed` for a sample) in the same transaction. Preview work serialises per workspace; the
//!    `one_running` partial unique index settles a race.
//! 3. The global-run driver claims the task and drives it with the preview
//!    platform (`previews::runtime::PreviewRunResolver`).
//! 4. [`sweep`], on the driver's periodic tick, retires runs past the
//!    wall-clock ceiling ([`retire_overdue`], `OXY_PREVIEW_RUN_MAX_MINUTES`),
//!    finishes rows whose run is terminal ([`mark_finished`]) and advances
//!    each workspace's queue.
//!
//! Gated by `OXY_PREVIEW_RUNS`: every runs route answers `404
//! preview_runs_disabled` without it, and the sweep starts nothing. Enable it
//! only once every serve, ide and worker replica runs this release.

mod ceiling;
mod lifecycle;
mod notes;
mod report;
mod submit;
#[cfg(test)]
mod tests;

pub use ceiling::{MAX_MINUTES_ENV, max_minutes, retire_overdue};
pub use lifecycle::{TERMINAL_RUN_STATUSES, cancel_queued, cancel_queued_at};

pub(crate) use report::outcome_of;
pub use report::{
    CopyNote, HeldNote, Redirect, RedirectNote, RunDetail, RunStep, RunSummary, get, list,
};
pub use submit::{SubmitRun, Submitted, submit};

use axum::http::StatusCode;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use agentic_airway::preview::SampleRefusal;

/// The flag. Truthy (`1`/`true`/`yes`/`on`) turns preview runs on.
pub const RUNS_FLAG_ENV: &str = "OXY_PREVIEW_RUNS";

pub fn runs_enabled() -> bool {
    std::env::var(RUNS_FLAG_ENV).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// Why a runs request was refused, in the contract's codes.
#[derive(Debug, thiserror::Error)]
pub enum RunRequestError {
    #[error("workspace preview runs are not enabled on this deployment")]
    Disabled,
    #[error("there is no preview of branch {0}")]
    PreviewNotFound(String),
    #[error("the preview of {0} has no ready staging revision yet")]
    NotReady(String),
    #[error("{0} is not an automation in the previewed revision")]
    RefNotInRevision(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("there is no preview run {0}")]
    RunNotFound(String),
    /// An Airway sample the rules refuse (`agentic_airway::preview`).
    #[error("{0}")]
    Sample(SampleRefusal),
    #[error("{0}")]
    Internal(String),
}

impl RunRequestError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Disabled => "preview_runs_disabled",
            Self::PreviewNotFound(_) => "preview_not_found",
            Self::NotReady(_) => "preview_not_ready",
            Self::RefNotInRevision(_) => "ref_not_in_revision",
            Self::BadRequest(_) => "bad_request",
            Self::RunNotFound(_) => "run_not_found",
            Self::Sample(refusal) => refusal.code(),
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Disabled
            | Self::PreviewNotFound(_)
            | Self::RefNotInRevision(_)
            | Self::RunNotFound(_) => StatusCode::NOT_FOUND,
            Self::NotReady(_) => StatusCode::CONFLICT,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Sample(refusal) => match refusal.code() {
                "sample_refused" | "sample_unsupported" => StatusCode::UNPROCESSABLE_ENTITY,
                "sandbox_required" => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            },
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<DbErr> for RunRequestError {
    fn from(e: DbErr) -> Self {
        Self::Internal(format!("database error: {e}"))
    }
}

/// The P3 queue step: the oldest queued run of the workspace — a procedure
/// or Airway sample staff started, or a transform build a change check queued
/// — becomes `running` when nothing preview-owned is running there.
const ADVANCE_SQL: &str = "\
    UPDATE workspace_preview_runs SET state = 'running', started_at = now() \
    WHERE run_id = ( \
      SELECT q.run_id FROM workspace_preview_runs q \
      WHERE q.workspace_id = $1 AND q.state = 'queued' \
        AND q.kind IN ('procedure','transform_build','airway_sample') \
        AND NOT EXISTS (SELECT 1 FROM workspace_preview_runs r \
                        WHERE r.workspace_id = q.workspace_id AND r.state = 'running' \
                          AND r.kind IN ('procedure','transform_build','airway_sample')) \
      ORDER BY q.created_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
    RETURNING run_id, kind, branch, preview_key, target_ref, options";

/// Start the next queued run of `workspace_id`, if the workspace has none
/// running: mark it `running` and seed its run, in one transaction. `Some` is
/// the run started. Losing the race to another caller is `Ok(None)` — the
/// `one_running` index refused the second `running` row.
pub async fn advance(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Option<String>, DbErr> {
    let txn = db.begin().await?;
    let won = match txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            ADVANCE_SQL,
            [workspace_id.into()],
        ))
        .await
    {
        Ok(row) => row,
        Err(e) if is_unique_violation(&e) => {
            txn.rollback().await?;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let Some(row) = won else {
        txn.rollback().await?;
        return Ok(None);
    };
    let (run_id, kind, key): (String, String, String) = (
        row.try_get("", "run_id")?,
        row.try_get("", "kind")?,
        row.try_get("", "preview_key")?,
    );
    if let Err(e) = seed_started(&txn, workspace_id, &row).await {
        txn.rollback().await?;
        return Err(DbErr::Custom(format!("seeding preview run {run_id}: {e}")));
    }
    // Run start: the key's schemas outlive the run, and a drop claimed but
    // not yet run lets them go (`registry::touch_key`); what the preview
    // already holds is noted on the run, for a build's compare.
    super::registry::touch_key(&txn, workspace_id, &key, super::registry::schema_ttl()).await?;
    lifecycle::note_held_before(&txn, &run_id, workspace_id, &key).await?;
    txn.commit().await?;
    tracing::info!(target: "preview", %workspace_id, %run_id, %kind, "preview run started");
    Ok(Some(run_id))
}

/// The started run's `agentic_runs` row and root task, by kind: an Airway
/// sample (`previews::sample::seed`) or an automation (a procedure or a
/// transform build).
async fn seed_started(
    txn: &sea_orm::DatabaseTransaction,
    workspace_id: Uuid,
    row: &sea_orm::QueryResult,
) -> Result<(), String> {
    let get = |col: &str| row.try_get::<String>("", col).map_err(|e| e.to_string());
    let (run_id, kind, branch, key) = (
        get("run_id")?,
        get("kind")?,
        get("branch")?,
        get("preview_key")?,
    );
    let options: Value = row.try_get("", "options").map_err(|e| e.to_string())?;
    let target_ref: Option<String> = row.try_get("", "target_ref").map_err(|e| e.to_string())?;
    let target = target_ref.as_deref().unwrap_or_default();
    if kind == super::sample::RUN_KIND {
        let sample = super::sample::SampleSeed {
            run_id: &run_id,
            workspace_id,
            branch: &branch,
            preview_key: &key,
            target_ref: target,
            options: &options,
        };
        return super::sample::seed(txn, &sample)
            .await
            .map_err(|e| e.to_string());
    }
    let seed = agentic_pipeline::automation_run::PreviewAutomationSeed {
        run_id: &run_id,
        target_ref: target,
        variables: options.get("variables").filter(|v| !v.is_null()).cloned(),
        workspace_id,
        preview_key: &key,
        branch: &branch,
    };
    agentic_pipeline::automation_run::seed_preview_automation_run(txn, seed)
        .await
        .map_err(|e| e.to_string())
}

fn is_unique_violation(e: &DbErr) -> bool {
    matches!(
        e.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    )
}

/// Finish every running preview run whose `agentic_runs` row is terminal
/// ([`TERMINAL_RUN_STATUSES`], the one list the TTL sweep also reads), and
/// restart each one's schema clock (run finish, `registry::touch_key`).
/// Returns the workspaces that freed a slot.
pub async fn mark_finished<C: ConnectionTrait>(db: &C) -> Result<Vec<Uuid>, DbErr> {
    let sql = format!(
        "UPDATE workspace_preview_runs p SET state = 'finished', finished_at = now() \
         FROM agentic_runs r \
         WHERE r.id = p.run_id AND p.state = 'running' \
           AND p.kind IN ('procedure','transform_build','airway_sample') \
           AND r.task_status IN {TERMINAL_RUN_STATUSES} \
         RETURNING p.workspace_id, p.preview_key"
    );
    let rows = db
        .query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await?;
    let ttl = super::registry::schema_ttl();
    let mut workspaces = Vec::with_capacity(rows.len());
    for row in &rows {
        let workspace_id: Uuid = row.try_get("", "workspace_id")?;
        let key: String = row.try_get("", "preview_key")?;
        super::registry::touch_key(db, workspace_id, &key, ttl).await?;
        workspaces.push(workspace_id);
    }
    Ok(workspaces)
}

/// The periodic step: finish what is done, then start what is queued. Errors
/// are logged; the next tick retries.
pub async fn sweep(db: &DatabaseConnection) {
    // First, so an overdue run frees its workspace's slot this same tick.
    if let Err(e) = retire_overdue(db, max_minutes()).await {
        tracing::warn!(target: "preview", error = %e, "could not retire overdue preview runs");
    }
    match mark_finished(db).await {
        Ok(done) if !done.is_empty() => {
            tracing::info!(target: "preview", finished = done.len(), "preview runs finished");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(target: "preview", error = %e, "could not finish preview runs"),
    }
    // A transform build that just finished gets its compare this tick.
    if let Err(e) = super::compare::enqueue_compares(db).await {
        tracing::warn!(target: "preview", error = %e, "could not queue preview compares");
    }
    if !runs_enabled() {
        return;
    }
    let queued = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT workspace_id FROM workspace_preview_runs \
             WHERE state = 'queued' AND kind IN ('procedure','transform_build','airway_sample')",
        ))
        .await;
    let workspaces = match queued {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(target: "preview", error = %e, "could not list queued preview runs");
            return;
        }
    };
    for row in workspaces {
        let Ok(workspace_id) = row.try_get::<Uuid>("", "workspace_id") else {
            continue;
        };
        if let Err(e) = advance(db, workspace_id).await {
            tracing::warn!(target: "preview", %workspace_id, error = %e, "could not start a queued preview run");
        }
    }
}
