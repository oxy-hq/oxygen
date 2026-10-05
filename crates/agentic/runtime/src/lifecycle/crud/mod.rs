//! Lifecycle-side CRUD: the run row, its event log, suspensions, and
//! the read-side queries that join them.
//!
//! The shared `now()` / `user_facing_status` / `transition_run`
//! helpers live here because they operate on the run row. Orchestrator
//! CRUD reaches into them via `crate::orchestrator::crud::super::…`
//! (re-exported at `crate::crud::*` for the back-compat surface).

use sea_orm::{ActiveValue::*, DatabaseConnection, DbErr, EntityTrait};
use serde_json::Value;

use crate::lifecycle::entity::run;

pub mod events;
pub mod queries;
pub mod runs;
pub mod suspension;
mod undriven;
pub mod visibility;

pub use events::{
    EventRow, batch_insert_events, delete_events_from_seq, get_all_events, get_all_events_for_runs,
    get_events_after, get_max_seq, insert_event,
};
pub use queries::{
    AirwayTableSummary, AutomationStepSummary, LlmTokenSummary, LlmTokenSummaryByRun,
    ScheduleDurationBaseline, ThreadHistoryTurn, ToolExchangeRow, airway_table_summary_for_run,
    automation_step_summary_for_run, fetch_duration_baselines, get_effective_run_state, get_run,
    get_run_by_thread, get_runs_by_thread, get_thread_history, get_thread_history_with_events,
    list_active_runs, list_recent_runs, list_runs_filtered, llm_usage_for_run, llm_usage_for_runs,
    runs_in_workspace,
};
pub use runs::{
    heartbeat_driver, insert_run, insert_run_with_parent, insert_run_with_schedule,
    is_cancel_requested, load_task_tree, load_task_tree_in_workspace, release_driver,
    request_cancel, try_acquire_driver, update_run_done, update_run_failed, update_run_running,
    update_run_suspended, update_run_terminal_from_events, update_task_status,
};
pub use suspension::{get_suspension, get_suspension_with_start, upsert_suspension};
pub use undriven::fail_undriven_run;
pub use visibility::{
    PREVIEW_STAMP_KEY, RUN_ENVIRONMENT_KEY, customer_run_sql, get_run_in_workspace,
};

pub fn now() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::Utc::now().fixed_offset()
}

/// How long a driver lease (`agentic_runs.driver_id` /
/// `driver_heartbeat_at`) is honored without a heartbeat before another
/// driver may steal it. The driving loop must heartbeat well inside this
/// window (Task 6 owns the ticker). Gates recovery selection so a periodic
/// loop cannot double-drive a run a live driver already owns.
pub const DRIVER_LEASE_TTL_SECS: i64 = 90;

/// Derive the user-facing status from the internal task_status.
/// Used by the API serialization layer — NOT stored in DB.
pub fn user_facing_status(task_status: Option<&str>) -> &str {
    match task_status {
        Some("running") | Some("delegating") | None => "running",
        Some("awaiting_input") => "suspended",
        Some("done") => "done",
        Some("failed") | Some("timed_out") => "failed",
        Some("cancelled") => "cancelled",
        _ => "running",
    }
}

/// Atomic state transition for a run. Sets task_status and optionally
/// answer/error_message/task_metadata in a single UPDATE.
pub async fn transition_run(
    db: &DatabaseConnection,
    run_id: &str,
    task_status: &str,
    task_metadata: Option<Value>,
    answer: Option<&str>,
    error_message: Option<&str>,
) -> Result<(), DbErr> {
    let mut model = run::ActiveModel {
        id: Set(run_id.to_string()),
        task_status: Set(Some(task_status.to_string())),
        updated_at: Set(now()),
        ..Default::default()
    };
    if let Some(meta) = task_metadata {
        model.task_metadata = Set(Some(meta));
    }
    if let Some(ans) = answer {
        model.answer = Set(Some(ans.to_string()));
    }
    if let Some(err) = error_message {
        model.error_message = Set(Some(err.to_string()));
    }
    // A terminal run needs no driver — clear the lease so it isn't left
    // dangling (and so observability doesn't show a "held" lease on a
    // finished run). Unconditional here is safe: the run is terminal, no
    // driver should still be acting on it.
    if matches!(task_status, "done" | "failed" | "cancelled" | "timed_out") {
        model.driver_id = Set(None);
        model.driver_heartbeat_at = Set(None);
    }
    run::Entity::update(model).exec(db).await?;
    Ok(())
}

/// Reset a terminal run back to `running` for a **reset-in-place retry**: set
/// `running` AND explicitly clear the prior attempt's `error_message`, `answer`,
/// and driver lease. Unlike [`transition_run`] / [`update_run_running`] (whose
/// `error_message: None` means "leave unchanged"), this NULLs the error so the
/// UI doesn't keep showing a stale failure after a retry. Pair it with
/// `delete_events_from_seq(.., 0)` to drop the failed attempt's events too.
pub async fn reset_run_for_retry(db: &DatabaseConnection, run_id: &str) -> Result<(), DbErr> {
    let model = run::ActiveModel {
        id: Set(run_id.to_string()),
        task_status: Set(Some("running".to_string())),
        error_message: Set(None),
        answer: Set(None),
        driver_id: Set(None),
        driver_heartbeat_at: Set(None),
        // Zero the recovery budget too. A user asking for a retry is the
        // explicit "try again" signal the automatic bound defers to, so it must
        // hand the run a full budget back — otherwise the retry path *spends*
        // budget (it goes through `mark_task_global` → `find_pending_global_runs`
        // → `recover_single_run`) and the fifth retry of a run would be
        // dead-lettered before doing any work, permanently, with nothing in the
        // tree able to lower `attempt` again.
        attempt: Set(0),
        updated_at: Set(now()),
        ..Default::default()
    };
    run::Entity::update(model).exec(db).await?;
    Ok(())
}

/// Clear a run's `error_message` and nothing else.
///
/// `cleanup_stale_runs` stamps a "server restarted: run will be resumed
/// automatically" placeholder into `error_message` on the runs it marks
/// `needs_resume`/leaves `shutdown` so the UI has something to say while the
/// run sits waiting to be re-claimed. That note is correct *while* the run
/// is stranded, but once recovery actually re-claims and re-drives it, the
/// note is stale — left in place it renders as a false red "Pipeline error"
/// banner over a run that's healthy and resuming.
///
/// Unlike [`reset_run_for_retry`], this does NOT touch `answer` or the
/// driver lease (`driver_id`/`driver_heartbeat_at`): a resume is not a
/// retry-from-scratch — the prior answer is real state to preserve, and
/// recovery is *acquiring* the driver lease right before calling this, not
/// releasing it. No `updated_at` bump, matching the driver-lease helpers in
/// `runs.rs` (`try_acquire_driver` / `heartbeat_driver` / `release_driver`):
/// this is a UI-facing correction, not new run progress, so it shouldn't
/// perturb staleness checks (e.g. `find_stuck_runs`'s grace window) that key
/// off `updated_at`.
pub async fn clear_run_error(db: &DatabaseConnection, run_id: &str) -> Result<(), DbErr> {
    let model = run::ActiveModel {
        id: Set(run_id.to_string()),
        error_message: Set(None),
        ..Default::default()
    };
    run::Entity::update(model).exec(db).await?;
    Ok(())
}
