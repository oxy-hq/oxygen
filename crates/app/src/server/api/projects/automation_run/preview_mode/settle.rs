//! Closing a custom app's preview-run poll rows from the finished run: the
//! half of [`super`] that the preview sweep drives.

use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde_json::Value as JsonValue;
use tracing::warn;
use uuid::Uuid;

use crate::server::api::projects::automation_run::settle;
use crate::server::previews::runs::TERMINAL_RUN_STATUSES;

/// Close every open poll row whose preview run has finished, from its
/// `agentic_runs` row: `done` with the steps' results as outputs, `failed`
/// with the run's error, or `cancelled`. Returns how many rows this call
/// closed; a row someone else closed first is not counted.
pub async fn settle_custom_app_runs(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let sql = format!(
        "SELECT p.run_id, r.task_status, r.error_message, s.results \
         FROM customer_app_automation_runs a \
         JOIN workspace_preview_runs p ON p.run_id = a.id::text AND p.state = 'finished' \
         JOIN agentic_runs r ON r.id = p.run_id \
         LEFT JOIN agentic_workflow_state s ON s.run_id = p.run_id \
         WHERE a.status = 'running' AND r.task_status IN {TERMINAL_RUN_STATUSES}"
    );
    let rows = db
        .query_all_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await?;
    let mut closed = 0;
    for row in rows {
        match settle_row(db, &row).await {
            Ok(true) => closed += 1,
            Ok(false) => {}
            // One bad row must not hold back the rest of the batch.
            Err(e) => warn!(error = %e, "could not settle a custom-app preview run; skipping it"),
        }
    }
    Ok(closed)
}

/// Close the poll row `row` (a finished run) names. `Ok(false)` when it was
/// not ours to close: unparsable id, or already closed by someone else.
async fn settle_row(db: &DatabaseConnection, row: &sea_orm::QueryResult) -> Result<bool, DbErr> {
    let run_id: String = row.try_get("", "run_id")?;
    let Ok(run_id) = Uuid::parse_str(&run_id) else {
        return Ok(false);
    };
    let status: String = row.try_get("", "task_status")?;
    let message: Option<String> = row.try_get("", "error_message")?;
    let results: Option<JsonValue> = row.try_get("", "results")?;
    let terminal = terminal_for(&status, message, results);
    settle::close_running(db, run_id, terminal, settle::Guard::Running).await
}

/// The poll row's terminal state for a finished run's `task_status`.
fn terminal_for(
    task_status: &str,
    message: Option<String>,
    results: Option<JsonValue>,
) -> ProcRunActiveModel {
    match task_status {
        "done" => settle::done(&outputs_of(results)),
        "cancelled" => settle::cancelled(),
        "timed_out" => settle::failed_with(
            "automation_run_timeout",
            message.unwrap_or_else(|| "the preview run timed out".to_string()),
        ),
        _ => {
            // The run's own words, with the code the production path would
            // have classified them under (`Inline` prefixes its text).
            let message = message.unwrap_or_else(|| "automation failed".to_string());
            let error =
                agentic_pipeline::automation_run::AutomationRunError::Inline(message.clone());
            let (code, _) =
                crate::server::api::projects::automation_run::automation_error_to_code(&error);
            settle::failed_with(code, message)
        }
    }
}

/// A finished run's per-step results as the bundle's `outputs` map.
fn outputs_of(results: Option<JsonValue>) -> std::collections::HashMap<String, JsonValue> {
    match results {
        Some(JsonValue::Object(map)) => map.into_iter().collect(),
        _ => Default::default(),
    }
}
