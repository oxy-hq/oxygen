//! One function-job run, read back: its status, its answer and the
//! `function_log` lines it persisted (`GET …/function-runs/{run_id}`).
//!
//! Two guards run before anything is returned:
//!
//! 1. **The run is this app's.** Both seeds set `question = "fn:<app_id>/<name>"`,
//!    so the app-scoped path cannot read another app's run, or any run that is
//!    not a function's.
//! 2. **A run queued outside production is read only with reach.** It carries
//!    the staging or sandbox build's answer, error and log lines — a
//!    non-production row like any other — so a publish token, and a caller
//!    who may not open the app's non-production environments, are answered
//!    not-found, exactly as for a run id that does not exist: holding an id
//!    confirms nothing, not even which environment it ran in. A production
//!    run asks no such question.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use entity::apps;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;
use uuid::Uuid;

use super::environment_scope::{self, Caller, RouteError};

#[derive(Debug, Serialize)]
pub struct FunctionRunDetail {
    pub run_id: String,
    /// Effective status: `queued` (enqueued, not yet claimed by a worker) |
    /// `running` (a worker is executing it) | `done` | `failed` | `cancelled` |
    /// `timed_out`.
    pub status: Option<String>,
    /// `scheduled` | `manual`, from the run metadata.
    pub trigger: Option<String>,
    /// The app environment the run was queued for: `production` unless the
    /// trigger named another (`?environment=`), from the run metadata.
    pub environment: String,
    /// The `app_function_invocations` row the run wrote — the id to read its
    /// held writes and log lines back by. `None` until the run finishes, and
    /// for a run that was refused before it wrote one.
    pub invocation_id: Option<Uuid>,
    /// The function's return body on success.
    pub answer: Option<String>,
    pub error: Option<String>,
    pub logs: Vec<FunctionRunLogLine>,
}

#[derive(Debug, Serialize)]
pub struct FunctionRunLogLine {
    /// The event sequence number — a stable, unique id per run (a React key).
    pub seq: i64,
    pub level: String,
    pub message: String,
}

/// Whether a run is an `app_function` run seeded for app `id`. Both the scheduled
/// and manual seeds set `question = "fn:<app_id>/<name>"`, and `<app_id>` is a
/// fixed-length UUID followed by `/`, so the prefix can't collide across apps.
/// The security guard for the app-scoped run-detail endpoint.
fn run_belongs_to_app(source_type: Option<&str>, question: &str, id: Uuid) -> bool {
    source_type == Some("app_function") && question.starts_with(&format!("fn:{id}/"))
}

/// The run's *effective* execution state for the UI. `insert_run` stamps
/// `task_status="running"` at enqueue time — before any worker claims the task —
/// so a run that is only *queued* would otherwise read as "running" (a silent
/// spinner if no worker is draining the queue). Consult the queue: a still-
/// `queued` task reports `queued` (waiting for a worker), a `dead` task (retries
/// exhausted) reports `failed`; a terminal run status always wins.
fn effective_run_status(run_status: Option<&str>, queue_status: Option<&str>) -> String {
    if let Some(s) = run_status
        && matches!(s, "done" | "failed" | "cancelled" | "timed_out")
    {
        return s.to_string();
    }
    match queue_status {
        Some("queued") => "queued".to_string(),
        Some("dead") => "failed".to_string(),
        _ => run_status.unwrap_or("running").to_string(),
    }
}

/// The app environment a run was queued for, from its metadata. A run queued
/// before the field was stamped, and every production run, names none.
fn run_environment(metadata: Option<&serde_json::Value>) -> String {
    metadata
        .and_then(|m| m.get(agentic_runtime::crud::RUN_ENVIRONMENT_KEY))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("production")
        .to_string()
}

/// The invocation a run wrote, from its `app_function_completed` event
/// (`run_scheduled_function` emits one per attempt, so the last is the
/// attempt whose outcome the run reports). `None` before the run finishes.
fn completed_invocation_id<'a>(
    events: impl Iterator<Item = (&'a str, &'a serde_json::Value)>,
) -> Option<Uuid> {
    events
        .filter(|(event_type, _)| *event_type == "app_function_completed")
        .filter_map(|(_, payload)| payload.get("invocation_id")?.as_str()?.parse().ok())
        .last()
}

/// Whether `caller` may read a run queued in `environment`: anyone the mount
/// let through for production, and outside production only a caller with
/// reach — never a publish token (`Caller::has_reach`).
///
/// The refusal is one answer for everyone refused, a bare not-found. A
/// publish token is not told `publish_token_refused` here, as it is when a
/// request *names* a non-production environment: it named only a run id, and
/// a coded refusal would confirm the run and name its environment.
async fn require_readable(
    db: &DatabaseConnection,
    app_id: Uuid,
    caller: &Caller<'_>,
    environment: &str,
    queued_at: DateTime<Utc>,
) -> Result<(), RouteError> {
    let agent = super::agent_scope::is_agent(caller.user);
    if environment_scope::is_production(environment) && !agent {
        return Ok(());
    }
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // A sandbox agent token reads a run only of a sandbox it created; a run
    // queued in production is not its to read. One bare not-found, as below.
    if agent {
        return own_run(db, &app, caller, environment, queued_at).await;
    }
    if caller.has_reach(db, &app).await {
        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

/// A sandbox agent token's read of a run queued at `queued_at` in
/// `environment`: the environment is a sandbox the token created, and the run
/// was queued in **that** sandbox — not in an earlier one that had the name
/// (`custom_apps_sandbox_instance`). Anything else is the bare not-found.
async fn own_run(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    environment: &str,
    queued_at: DateTime<Utc>,
) -> Result<(), RouteError> {
    use super::agent_scope::{instance_since_named, require_own_named};
    use crate::server::api::custom_apps_sandbox_instance::is_of_instance;
    let not_found = |_| RouteError::from(StatusCode::NOT_FOUND);
    require_own_named(db, app, caller.user, environment)
        .await
        .map_err(not_found)?;
    let since = instance_since_named(db, app, caller.user, environment)
        .await
        .map_err(not_found)?;
    match since {
        Some(since) if is_of_instance(queued_at, since) => Ok(()),
        _ => Err(StatusCode::NOT_FOUND.into()),
    }
}

/// The `function_log` lines among a run's events, in the order written.
fn log_lines(events: Vec<agentic_runtime::crud::EventRow>) -> Vec<FunctionRunLogLine> {
    let text = |payload: &serde_json::Value, key: &str, default: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    };
    events
        .into_iter()
        .filter(|e| e.event_type == "function_log")
        .map(|e| FunctionRunLogLine {
            seq: e.seq,
            level: text(&e.payload, "level", "log"),
            message: text(&e.payload, "message", ""),
        })
        .collect()
}

/// Run `run_id` of app `id`, for `caller`: a bare `404` when it is not this
/// app's function run or `caller` may not read it.
pub(crate) async fn detail(
    db: &DatabaseConnection,
    id: Uuid,
    run_id: &str,
    caller: &Caller<'_>,
) -> Result<FunctionRunDetail, RouteError> {
    let run = agentic_runtime::crud::get_run(db, run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Ownership guard: the run must be an app_function run seeded for THIS app,
    // so the app-scoped path can't read another app's (or any non-function) run.
    if !run_belongs_to_app(run.source_type.as_deref(), &run.question, id) {
        return Err(StatusCode::NOT_FOUND.into());
    }
    let environment = run_environment(run.metadata.as_ref());
    let queued_at = run.created_at.with_timezone(&Utc);
    require_readable(db, id, caller, &environment, queued_at).await?;
    // Report queued-vs-running honestly: a background job is only executing once
    // a worker has claimed its queue task. Otherwise a run sitting in the queue
    // (e.g. no global worker draining it) reads as a perpetual "running" spinner.
    let queue_status = agentic_runtime::crud::get_queue_entry(db, run_id)
        .await
        .ok()
        .flatten()
        .map(|q| q.queue_status);
    let status = effective_run_status(run.task_status.as_deref(), queue_status.as_deref());
    let trigger = run
        .metadata
        .as_ref()
        .and_then(|m| m.get("trigger"))
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let events = agentic_runtime::crud::get_all_events(db, run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let invocation_id =
        completed_invocation_id(events.iter().map(|e| (e.event_type.as_str(), &e.payload)));
    Ok(FunctionRunDetail {
        run_id: run_id.to_string(),
        status: Some(status),
        trigger,
        environment,
        invocation_id,
        answer: run.answer,
        error: run.error_message,
        logs: log_lines(events),
    })
}

#[cfg(test)]
#[path = "function_run_tests.rs"]
mod tests;
