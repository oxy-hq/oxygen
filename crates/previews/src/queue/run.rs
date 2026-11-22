//! The preview queue's one step: start the oldest queued run of a workspace
//! when none is running, and seed what it runs (see `previews::runs` for the
//! whole cycle). Rows only — the run itself is driven on the worker fleet.
//!
//! Below the line the previews lift draws: the custom-app automation run's
//! preview mode starts its run through [`advance`], from `oxy-api-custom-apps`.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

/// The flag. Truthy (`1`/`true`/`yes`/`on`) turns preview runs on.
pub const RUNS_FLAG_ENV: &str = "OXY_PREVIEW_RUNS";

pub fn runs_enabled() -> bool {
    std::env::var(RUNS_FLAG_ENV).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
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
    crate::registry::touch_key(&txn, workspace_id, &key, crate::registry::schema_ttl()).await?;
    super::lifecycle::note_held_before(&txn, &run_id, workspace_id, &key).await?;
    txn.commit().await?;
    tracing::info!(target: "preview", %workspace_id, %run_id, %kind, "preview run started");
    Ok(Some(run_id))
}

/// The started run's `agentic_runs` row and root task, by kind: an Airway
/// sample (`super::sample_seed::seed`) or an automation (a procedure or a
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
    if kind == super::sample_seed::RUN_KIND {
        let sample = super::sample_seed::SampleSeed {
            run_id: &run_id,
            workspace_id,
            branch: &branch,
            preview_key: &key,
            target_ref: target,
            options: &options,
        };
        return super::sample_seed::seed(txn, &sample)
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
