//! What staff read back: `GET /previews/runs?branch=` and
//! `GET /previews/runs/{run_id}`.
//!
//! State comes from the registry row, the outcome from the run's
//! `agentic_runs.task_status`, and the steps, holds and redirects from the
//! automation's `agentic_workflow_state` — a step's result carries its
//! `preview` note wherever it ran ([`super::notes`]).

use chrono::SecondsFormat;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::RunRequestError;
pub use super::notes::{CopyNote, HeldNote, Redirect, RedirectNote};
use super::notes::{count_held, first_held, redirects};
use crate::server::previews::compare::{self, CompareView};

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub branch: String,
    /// `procedure` | `transform_build` | `compare` | `airway_sample`.
    pub kind: String,
    pub target_ref: Option<String>,
    /// The change check a `transform_build` was queued by; the build a
    /// `compare` compares.
    pub parent_run_id: Option<String>,
    pub revision_id: String,
    /// `queued` | `running` | `finished`.
    pub state: String,
    /// `succeeded` | `failed` | `cancelled`, once finished.
    pub outcome: Option<String>,
    pub held_count: usize,
    pub requested_by: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct RunDetail {
    #[serde(flatten)]
    pub summary: RunSummary,
    pub agentic_run_id: Option<String>,
    pub error: Option<String>,
    pub steps: Vec<RunStep>,
    /// A `transform_build`'s compare with live (or a `compare` run's own).
    pub compare: Option<CompareView>,
    /// An `airway_sample`'s ask and result (`previews::sample::view`).
    pub sample: Option<Value>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct RunStep {
    pub name: String,
    /// `execute_sql` | `http_request` | `airway` | `agent` | `other`.
    pub kind: String,
    /// `succeeded` | `failed` | `held` | `running` | `pending`.
    pub status: String,
    pub held: Option<HeldNote>,
    /// What the step wrote and read in the preview's own schemas instead of
    /// live (managed Airhouse, phase 2b).
    pub redirected: Option<RedirectNote>,
}

const SELECT: &str = "\
    SELECT p.run_id, p.branch, p.kind, p.target_ref, p.parent_run_id, p.revision_id, p.state, \
           p.requested_by, p.created_at, p.started_at, p.finished_at, \
           p.options, r.id AS agentic_run_id, r.task_status, r.error_message, \
           r.updated_at AS run_updated_at, r.task_metadata, \
           s.results, s.workflow_config, s.current_step \
    FROM workspace_preview_runs p \
    LEFT JOIN agentic_runs r ON r.id = p.run_id \
    LEFT JOIN agentic_workflow_state s ON s.run_id = p.run_id \
    WHERE p.workspace_id = $1 AND p.kind <> 'analyze' ";

/// A branch's runs, newest first, at most 50.
pub async fn list(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
) -> Result<Vec<RunSummary>, RunRequestError> {
    super::super::service::validate_branch_name(branch)
        .map_err(|e| RunRequestError::BadRequest(e.to_string()))?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("{SELECT} AND p.branch = $2 ORDER BY p.created_at DESC LIMIT 50"),
            [workspace_id.into(), branch.into()],
        ))
        .await?;
    rows.iter().map(|r| Ok(Row::read(r)?.summary())).collect()
}

/// One run, with its steps.
pub async fn get(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    run_id: &str,
) -> Result<RunDetail, RunRequestError> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!("{SELECT} AND p.run_id = $2"),
            [workspace_id.into(), run_id.into()],
        ))
        .await?
        .ok_or_else(|| RunRequestError::RunNotFound(run_id.to_string()))?;
    let row = Row::read(&row)?;
    let compare = match row.kind.as_str() {
        "transform_build" | "compare" => compare::for_run(db, workspace_id, run_id).await?,
        _ => None,
    };
    let sample = (row.kind == crate::server::previews::sample::RUN_KIND)
        .then(|| crate::server::previews::sample::view(&row.options, row.task_metadata.as_ref()));
    Ok(RunDetail {
        summary: row.summary(),
        agentic_run_id: row.agentic_run_id.clone(),
        error: row.error_message.clone(),
        steps: steps(&row),
        compare,
        sample,
    })
}

type Ts = chrono::DateTime<chrono::FixedOffset>;

struct Row {
    run_id: String,
    branch: String,
    kind: String,
    target_ref: Option<String>,
    parent_run_id: Option<String>,
    revision_id: Uuid,
    state: String,
    requested_by: Option<Uuid>,
    options: Value,
    task_metadata: Option<Value>,
    created_at: Ts,
    started_at: Option<Ts>,
    finished_at: Option<Ts>,
    agentic_run_id: Option<String>,
    task_status: Option<String>,
    error_message: Option<String>,
    run_updated_at: Option<Ts>,
    results: Option<Value>,
    workflow: Option<Value>,
    current_step: Option<i32>,
}

impl Row {
    fn read(r: &QueryResult) -> Result<Self, sea_orm::DbErr> {
        Ok(Self {
            run_id: r.try_get("", "run_id")?,
            branch: r.try_get("", "branch")?,
            kind: r.try_get("", "kind")?,
            target_ref: r.try_get("", "target_ref")?,
            parent_run_id: r.try_get("", "parent_run_id")?,
            revision_id: r.try_get("", "revision_id")?,
            state: r.try_get("", "state")?,
            requested_by: r.try_get("", "requested_by")?,
            options: r.try_get("", "options")?,
            task_metadata: r.try_get("", "task_metadata")?,
            created_at: r.try_get("", "created_at")?,
            started_at: r.try_get("", "started_at")?,
            finished_at: r.try_get("", "finished_at")?,
            agentic_run_id: r.try_get("", "agentic_run_id")?,
            task_status: r.try_get("", "task_status")?,
            error_message: r.try_get("", "error_message")?,
            run_updated_at: r.try_get("", "run_updated_at")?,
            results: r.try_get("", "results")?,
            workflow: r.try_get("", "workflow_config")?,
            current_step: r.try_get("", "current_step")?,
        })
    }

    fn outcome(&self) -> Option<&'static str> {
        outcome_of(self.task_status.as_deref())
    }

    /// A run whose `agentic_runs` row is terminal reads `finished` even before
    /// the sweep has written it, so the viewer does not lag the run.
    fn summary(&self) -> RunSummary {
        let terminal = self.outcome().is_some();
        let finished_at = self
            .finished_at
            .or(if terminal { self.run_updated_at } else { None });
        RunSummary {
            run_id: self.run_id.clone(),
            branch: self.branch.clone(),
            kind: self.kind.clone(),
            target_ref: self.target_ref.clone(),
            parent_run_id: self.parent_run_id.clone(),
            revision_id: self.revision_id.to_string(),
            state: if terminal {
                "finished".into()
            } else {
                self.state.clone()
            },
            outcome: self.outcome().map(str::to_string),
            held_count: self.results.as_ref().map(count_held).unwrap_or(0),
            requested_by: self.requested_by.map(|u| u.to_string()),
            created_at: iso_utc(&self.created_at),
            started_at: self.started_at.as_ref().map(iso_utc),
            finished_at: finished_at.as_ref().map(iso_utc),
        }
    }
}

pub(crate) fn outcome_of(task_status: Option<&str>) -> Option<&'static str> {
    match task_status? {
        "done" => Some("succeeded"),
        "failed" | "timed_out" => Some("failed"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}

fn iso_utc(t: &Ts) -> String {
    t.with_timezone(&chrono::Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The top-level steps of the automation, in order, each with its status and
/// the first hold found in its result.
fn steps(row: &Row) -> Vec<RunStep> {
    let Some(tasks) = row
        .workflow
        .as_ref()
        .and_then(|w| w.get("tasks"))
        .and_then(Value::as_array)
    else {
        return vec![];
    };
    let empty = serde_json::Map::new();
    let results = row
        .results
        .as_ref()
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let current = row.current_step.unwrap_or(0).max(0) as usize;
    tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let name = task
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let held = results.get(&name).and_then(first_held);
            let redirected = results.get(&name).and_then(redirects);
            let status = step_status(
                results.contains_key(&name),
                held.is_some(),
                i,
                current,
                row.outcome(),
            );
            RunStep {
                kind: step_kind(task.get("type").and_then(Value::as_str)).to_string(),
                name,
                status: status.to_string(),
                held,
                redirected,
            }
        })
        .collect()
}

pub(super) fn step_kind(task_type: Option<&str>) -> &'static str {
    match task_type {
        Some("execute_sql") => "execute_sql",
        Some("http_request") => "http_request",
        Some("airway") => "airway",
        Some("agent") => "agent",
        _ => "other",
    }
}

/// A failed step keeps its (error) result, so the run's outcome and position
/// decide it: the step the run stopped on failed; one it never reached is
/// pending.
pub(super) fn step_status(
    has_result: bool,
    held: bool,
    index: usize,
    current: usize,
    outcome: Option<&str>,
) -> &'static str {
    let stopped_here = index == current && matches!(outcome, Some("failed" | "cancelled"));
    match (has_result, held) {
        _ if stopped_here => "failed",
        (true, true) => "held",
        (true, false) => "succeeded",
        (false, _) if index == current && outcome.is_none() => "running",
        (false, _) => "pending",
    }
}
