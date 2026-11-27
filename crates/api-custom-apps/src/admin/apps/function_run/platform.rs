//! A function run as the platform that queued it reads it, with no caller
//! and so no reach question: the promote checks' `functions` check queued the
//! run itself (`custom_apps_build_checks::functions`) and reads back how it
//! ended, **which build it ran** and **how long the function took**.
//!
//! Both are the invocation row's (`app_function_invocations`), the one record
//! of what actually ran. The build matters because a queued run resolves its
//! environment's build when the worker runs it, not when it was queued
//! (`build_id`). The time is the function's own (`duration_ms`), not how long
//! its run sat in the queue or how long anybody waited for it.

use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::{attempt_invocation_id, effective_run_status};

/// A run's effective status, its answer and error, and the invocation it
/// wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlatformRunReading {
    pub status: String,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub invocation_id: Option<Uuid>,
    /// The build that invocation ran (`app_function_invocations.build_id`):
    /// the one the worker resolved when it ran the task, which is not the one
    /// the environment served when the run was queued if its pointer moved in
    /// between. `None` when the run wrote no invocation.
    pub ran_build: Option<Uuid>,
    /// How long the function itself ran, in milliseconds, as that invocation
    /// recorded it. `None` when the run wrote no invocation, or it has not
    /// finished.
    pub duration_ms: Option<u64>,
}

/// The build the invocation `invocation_id` ran, and how long it took.
async fn invocation_ran(
    db: &DatabaseConnection,
    invocation_id: Option<Uuid>,
) -> Result<Option<(Uuid, Option<i64>)>, sea_orm::DbErr> {
    use entity::app_function_invocations::{Column, Entity};
    use sea_orm::QuerySelect;
    let Some(invocation_id) = invocation_id else {
        return Ok(None);
    };
    Entity::find_by_id(invocation_id)
        .select_only()
        .column(Column::BuildId)
        .column(Column::DurationMs)
        .into_tuple()
        .one(db)
        .await
}

/// The run `run_id` as it stands; `None` for a run that does not exist.
pub(crate) async fn read_for_platform(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<PlatformRunReading>, sea_orm::DbErr> {
    let Some(run) = agentic_runtime::crud::get_run(db, run_id).await? else {
        return Ok(None);
    };
    let queue_status = agentic_runtime::crud::get_queue_entry(db, run_id)
        .await?
        .map(|q| q.queue_status);
    let status = effective_run_status(run.task_status.as_deref(), queue_status.as_deref());
    let events = agentic_runtime::crud::get_all_events(db, run_id).await?;
    let invocation_id =
        attempt_invocation_id(events.iter().map(|e| (e.event_type.as_str(), &e.payload)));
    let ran = invocation_ran(db, invocation_id).await?;
    Ok(Some(PlatformRunReading {
        status,
        answer: run.answer,
        error: run.error_message,
        invocation_id,
        ran_build: ran.map(|(build, _)| build),
        duration_ms: ran.and_then(|(_, took)| u64::try_from(took?).ok()),
    }))
}
