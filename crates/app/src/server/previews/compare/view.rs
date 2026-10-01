//! The compare a run report shows: for a `transform_build`, the compare
//! queued under it; for a `compare`, itself. Postgres only.

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use serde::Serialize;
use uuid::Uuid;

use super::{CompareReport, TableCompare};
use crate::server::previews::runs::outcome_of;

/// `GET /previews/runs/{run_id}` → `compare`.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct CompareView {
    pub run_id: String,
    /// `queued` | `running` | `finished`.
    pub state: String,
    /// `succeeded` | `failed` | `cancelled`, once finished.
    pub outcome: Option<String>,
    pub error: Option<String>,
    /// `OXY_PREVIEW_DIFF_MAX_ROWS` when it ran.
    pub diff_max_rows: Option<u64>,
    /// What else a difference can mean (fixed sentences); empty until done.
    pub caveats: Vec<String>,
    /// Counts per table the build wrote; empty until the compare is done.
    pub tables: Vec<TableCompare>,
}

const COMPARE_SQL: &str = "\
    SELECT c.run_id, c.state, a.task_status, a.error_message, a.task_metadata \
    FROM workspace_preview_runs c LEFT JOIN agentic_runs a ON a.id = c.run_id \
    WHERE c.workspace_id = $1 AND c.kind = 'compare' \
      AND (c.parent_run_id = $2 OR c.run_id = $2) \
    ORDER BY c.created_at DESC LIMIT 1";

/// The compare of run `run_id` (a build's, or the compare itself), if any.
pub async fn for_run<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    run_id: &str,
) -> Result<Option<CompareView>, DbErr> {
    let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            COMPARE_SQL,
            [workspace_id.into(), run_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let task_status: Option<String> = row.try_get("", "task_status")?;
    let outcome = outcome_of(task_status.as_deref());
    let report = row
        .try_get::<Option<serde_json::Value>>("", "task_metadata")?
        .filter(|_| outcome == Some("succeeded"))
        .and_then(|m| serde_json::from_value::<CompareReport>(m).ok());
    let state: String = row.try_get("", "state")?;
    Ok(Some(CompareView {
        run_id: row.try_get("", "run_id")?,
        state: if outcome.is_some() {
            "finished".into()
        } else {
            state
        },
        outcome: outcome.map(str::to_string),
        error: row.try_get("", "error_message")?,
        diff_max_rows: report.as_ref().map(|r| r.diff_max_rows),
        caveats: report
            .as_ref()
            .map(|r| r.caveats.clone())
            .unwrap_or_default(),
        tables: report.map(|r| r.tables).unwrap_or_default(),
    }))
}
