//! The held preview runs' per-pass step, run by the global driver on every
//! pass (`drivers::recovery`): retire runs past the wall-clock ceiling
//! ([`super::ceiling`]), finish rows whose run is terminal, settle a custom
//! app's preview-mode automation run, queue the compare under each finished
//! transform build ([`super::compare`]) and start each workspace's next queued
//! run ([`super::run::advance`]).
//!
//! Row writes and queue inserts only, over the registry's tables, so it stays
//! below the previews surface with the rest of the queue: the driver and
//! `oxy-api-custom-apps`' tests of the preview-mode automation run call it.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::ceiling::{max_minutes, retire_overdue};
use super::lifecycle::TERMINAL_RUN_STATUSES;
use super::run::{advance, runs_enabled};

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
    let ttl = crate::registry::schema_ttl();
    let mut workspaces = Vec::with_capacity(rows.len());
    for row in &rows {
        let workspace_id: Uuid = row.try_get("", "workspace_id")?;
        let key: String = row.try_get("", "preview_key")?;
        crate::registry::touch_key(db, workspace_id, &key, ttl).await?;
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
    // A custom app's automation run that ran as a preview: its poll row reads
    // the outcome now that the run is finished.
    if let Err(e) = super::preview_settle::settle_custom_app_runs(db).await {
        tracing::warn!(target: "preview", error = %e, "could not settle custom-app preview runs");
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
