//! Serving the Airway change check (`analyze`): where a preview's check stands
//! and what it found. Postgres only — the analyze row and its `agentic_runs`
//! outcome — so every function here is safe on any pod.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
};
use serde::Serialize;
use uuid::Uuid;

use super::analyze::{AnalyzeReport, PipelineReport, TransformReport};

/// A preview's check at a glance, on its list item.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    /// `pending` | `done` | `failed`.
    pub status: String,
    /// Changed pipelines whose verdict is `needs_reset`.
    pub needs_reset: u32,
    /// Changed pipelines whose verdict is `warning`.
    pub warnings: u32,
    /// Changed transforms (automations), regardless of `auto`/`manual` build.
    pub transforms: u32,
}

/// `GET /previews/checks?branch=` — the check of the preview's current revision.
#[derive(Serialize, Debug)]
pub struct ChecksResponse {
    pub branch: String,
    /// The staging revision checked; `None` while it is still compiling.
    pub revision_id: Option<String>,
    /// `pending` | `done` | `failed`.
    pub status: String,
    pub error: Option<String>,
    pub pipelines: Vec<PipelineReport>,
    /// The automations the branch changed: `auto` ones are built in the
    /// preview and compared with live, `manual` ones say why not.
    pub transforms: Vec<TransformReport>,
}

impl ChecksResponse {
    /// The preview's staging compile failed: nothing was, or will be, checked
    /// at this commit, so the check reads as failed with the compile's error.
    pub fn compile_failed(branch: &str, compile_error: Option<String>) -> Self {
        Self {
            branch: branch.to_string(),
            revision_id: None,
            status: "failed".to_string(),
            error: Some(format!(
                "the branch did not compile, so its pipelines were not checked: {}",
                compile_error.as_deref().unwrap_or("compile failed")
            )),
            pipelines: Vec::new(),
            transforms: Vec::new(),
        }
    }
}

/// Where one analyze run stands.
#[derive(Debug, PartialEq)]
pub struct CheckOutcome {
    pub status: &'static str,
    pub error: Option<String>,
    pub report: Option<AnalyzeReport>,
}

impl CheckOutcome {
    const PENDING: Self = Self {
        status: "pending",
        error: None,
        report: None,
    };

    fn failed(error: impl Into<String>) -> Self {
        Self {
            status: "failed",
            error: Some(error.into()),
            report: None,
        }
    }

    pub fn summary(&self) -> CheckSummary {
        let (needs_reset, warnings, transforms) = self
            .report
            .as_ref()
            .map_or((0, 0, 0), AnalyzeReport::counts);
        CheckSummary {
            status: self.status.to_string(),
            needs_reset,
            warnings,
            transforms,
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct OutcomeRow {
    state: String,
    task_status: Option<String>,
    task_metadata: Option<serde_json::Value>,
    error_message: Option<String>,
}

const OUTCOME_SQL: &str = "\
    SELECT r.state, a.task_status, a.task_metadata, a.error_message \
    FROM workspace_preview_runs r \
    LEFT JOIN agentic_runs a ON a.id = r.run_id \
    WHERE r.workspace_id = $1 AND r.revision_id = $2 AND r.kind = 'analyze' \
    ORDER BY r.created_at DESC \
    LIMIT 1";

/// The check of `revision_id`, or `None` when none was queued for it.
pub async fn outcome<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<Option<CheckOutcome>, DbErr> {
    let row = OutcomeRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        OUTCOME_SQL,
        [workspace_id.into(), revision_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(from_row))
}

/// The run's `task_status` is the authority: the executor's own row state can
/// lag a failure (a failed attempt never reaches `finished`).
fn from_row(row: OutcomeRow) -> CheckOutcome {
    match row.task_status.as_deref() {
        Some("done") => match row
            .task_metadata
            .map(serde_json::from_value::<AnalyzeReport>)
        {
            Some(Ok(report)) => CheckOutcome {
                status: "done",
                error: None,
                report: Some(report),
            },
            _ => CheckOutcome::failed("the check finished but its report could not be read"),
        },
        Some("failed" | "cancelled" | "timed_out") => CheckOutcome::failed(
            row.error_message
                .unwrap_or_else(|| "the check did not finish".to_string()),
        ),
        None if row.state == "finished" => {
            CheckOutcome::failed("the check's run record is no longer available")
        }
        _ => CheckOutcome::PENDING,
    }
}

/// The check of `branch`'s preview at its current revision. `revision_id` is
/// the preview's ready revision, `None` while it compiles; a revision with no
/// check queued yet reads as pending with no pipelines.
pub async fn for_preview(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
    revision_id: Option<Uuid>,
) -> Result<ChecksResponse, DbErr> {
    let outcome = match revision_id {
        Some(rev) => outcome(db, workspace_id, rev).await?,
        None => None,
    }
    .unwrap_or(CheckOutcome::PENDING);
    let (pipelines, transforms) = outcome
        .report
        .map(|r| (r.pipelines, r.transforms))
        .unwrap_or_default();
    Ok(ChecksResponse {
        branch: branch.to_string(),
        revision_id: revision_id.map(|id| id.to_string()),
        status: outcome.status.to_string(),
        error: outcome.error,
        pipelines,
        transforms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, task_status: Option<&str>, meta: Option<serde_json::Value>) -> OutcomeRow {
        OutcomeRow {
            state: state.into(),
            task_status: task_status.map(str::to_string),
            task_metadata: meta,
            error_message: Some("boom".into()),
        }
    }

    #[test]
    fn the_run_status_decides_and_a_queued_check_is_pending() {
        assert_eq!(
            from_row(row("queued", Some("running"), None)).status,
            "pending"
        );
        assert_eq!(
            from_row(row("running", Some("running"), None)).status,
            "pending"
        );
        let failed = from_row(row("running", Some("failed"), None));
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.error.as_deref(), Some("boom"));
        assert_eq!(from_row(row("finished", None, None)).status, "failed");
    }

    #[test]
    fn a_done_run_without_a_readable_report_is_failed_not_clean() {
        let o = from_row(row(
            "finished",
            Some("done"),
            Some(serde_json::json!({"x": 1})),
        ));
        assert_eq!(o.status, "failed");
        assert_eq!(o.summary().needs_reset, 0);
    }
}
