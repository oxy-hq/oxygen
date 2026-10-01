//! P2: each auto-build transform the change check found gets a queued
//! `transform_build` run — a procedure run of the staging revision, Airway
//! held — under the check (`parent_run_id`), once. The per-workspace queue
//! (`runs::advance`) starts it; `maintenance::enqueue_compares` compares it
//! with live once it is done.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde_json::json;
use uuid::Uuid;

use super::transforms::TransformReport;
use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, Writers};
use crate::server::previews::runs;

/// With runs off, an auto transform is reported, not queued.
const RUNS_OFF: &str = "not built: workspace preview runs are off on this deployment \
                        (OXY_PREVIEW_RUNS)";

/// Where a build's writes would be held, nothing is built: it would compare
/// nothing.
pub const BUILDS_SKIPPED: &str = "builds skipped: Airhouse can't scope preview writers";

/// Queue a build for every `auto` transform of `check` that has none yet,
/// and fill in each one's `build_run_id` — unless runs are off, or this
/// deployment's Airhouse cannot take a preview's writes, which each `auto`
/// transform's `reason` then says. The check's row is locked while its builds
/// are read and written, so a re-run check never queues one twice.
pub async fn queue_builds(
    db: &DatabaseConnection,
    airhouse: &dyn PreviewAirhousePorts,
    check: &entity::workspace_preview_runs::Model,
    transforms: &mut [TransformReport],
) -> Result<(), DbErr> {
    if !transforms.iter().any(|t| t.build == "auto") {
        return Ok(());
    }
    if let Some(why) = not_built(airhouse).await {
        for t in transforms.iter_mut().filter(|t| t.build == "auto") {
            t.reason = Some(why.clone());
        }
        return Ok(());
    }
    let txn = db.begin().await?;
    lock(&txn, &check.run_id).await?;
    for t in transforms.iter_mut().filter(|t| t.build == "auto") {
        let run_id = match existing(&txn, &check.run_id, &t.file_path).await? {
            Some(run_id) => run_id,
            None => insert(&txn, check, &t.file_path).await?,
        };
        t.build_run_id = Some(run_id);
    }
    txn.commit().await?;
    runs::advance(db, check.workspace_id).await?;
    Ok(())
}

/// Why no build is queued at all, if none is.
async fn not_built(airhouse: &dyn PreviewAirhousePorts) -> Option<String> {
    if !runs::runs_enabled() {
        return Some(RUNS_OFF.into());
    }
    match airhouse.deployment_writers().await {
        Ok(Writers::Confined) => None,
        Ok(Writers::Unavailable(why)) => Some(format!("{BUILDS_SKIPPED} ({why})")),
        Err(e) => {
            tracing::warn!(target: "preview", error = %e,
                "could not ask Airhouse whether it scopes preview writers; builds skipped");
            Some(format!("{BUILDS_SKIPPED} (it could not be asked)"))
        }
    }
}

async fn lock<C: ConnectionTrait>(db: &C, check_run_id: &str) -> Result<(), DbErr> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT run_id FROM workspace_preview_runs WHERE run_id = $1 FOR UPDATE",
        [check_run_id.into()],
    ))
    .await
    .map(|_| ())
}

async fn existing<C: ConnectionTrait>(
    db: &C,
    check_run_id: &str,
    target_ref: &str,
) -> Result<Option<String>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT run_id FROM workspace_preview_runs \
             WHERE parent_run_id = $1 AND kind = 'transform_build' AND target_ref = $2",
            [check_run_id.into(), target_ref.into()],
        ))
        .await?;
    row.map(|r| r.try_get("", "run_id")).transpose()
}

async fn insert<C: ConnectionTrait>(
    db: &C,
    check: &entity::workspace_preview_runs::Model,
    target_ref: &str,
) -> Result<String, DbErr> {
    let run_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_preview_runs \
             (run_id, workspace_id, branch, preview_key, revision_id, kind, target_ref, \
              parent_run_id, options, state) \
         VALUES ($1, $2, $3, $4, $5, 'transform_build', $6, $7, $8, 'queued')",
        [
            run_id.clone().into(),
            check.workspace_id.into(),
            check.branch.clone().into(),
            check.preview_key.clone().into(),
            check.revision_id.into(),
            target_ref.into(),
            check.run_id.clone().into(),
            json!({ "variables": null, "read_live_only": false }).into(),
        ],
    ))
    .await?;
    tracing::info!(target: "preview", workspace_id = %check.workspace_id, %run_id, target_ref,
        check = %check.run_id, "preview transform build queued");
    Ok(run_id)
}
