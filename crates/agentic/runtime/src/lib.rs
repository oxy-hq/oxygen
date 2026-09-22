//! Transport-agnostic execution infrastructure for agentic pipelines.
//!
//! Provides run lifecycle management, event persistence, SSE streaming support,
//! and the `EventRegistry` for domain-aware event processing. Used by both the
//! HTTP server (`agentic-http`) and CLI (`oxy agentic`).
//!
//! This crate is **domain-agnostic** — it never imports analytics, builder, or
//! any domain-specific types. Domain behavior is injected via callbacks and
//! the `EventRegistry` pattern.
//!
//! # Layout
//!
//! The crate is split into two sub-layers — Stage 1 of the airway/airform
//! extraction. Pick the layer your code actually needs; the layered model
//! also makes it clearer which parts a future ELT or transformation
//! runtime would reuse.
//!
//! - [`lifecycle`] — run row + event log + suspensions + SSE plumbing.
//!   The "what a run *is*" layer. Zero orchestrator dependencies.
//! - [`orchestrator`] — durable task queue + coordinator + worker pool
//!   + transports. The "how a run *executes*" layer. Depends on
//!   `lifecycle` for storage primitives.
//!
//! Cross-cutting:
//! - [`migration`] — single SeaORM migrator covering both layers'
//!   tables (`seaql_migrations_orchestrator` tracking table).
//!
//! For backward compatibility with the pre-Stage-1 flat layout the
//! legacy top-level paths (`agentic_runtime::coordinator`,
//! `agentic_runtime::state`, `agentic_runtime::crud`, etc.) are
//! re-exported below so existing callers keep working without
//! touching every import.

pub mod cron;
pub mod lifecycle;
pub mod migration;
pub mod orchestrator;

// ── Back-compat top-level paths ────────────────────────────────────────────
//
// These flatten the new two-layer structure back into the pre-Stage-1
// surface so the ~180 external `agentic_runtime::<flat>::…` imports
// across analytics/builder/automation/pipeline/http keep compiling.
// New code should prefer the explicit `lifecycle::…` / `orchestrator::…`
// paths so the layering shows up in `use` statements.

pub use lifecycle::{bridge, event_registry, handle, state};
pub use orchestrator::{background, circuit_breaker, coordinator, router, transport, worker};

/// The hub-carrying `spawn` / `spawn_blocking` the agentic crates use in
/// place of tokio's, re-exported for `agentic-http`: it may name this crate
/// but not `agentic-core`, and its handlers spawn too.
pub use agentic_core::hub_task;

/// Union of lifecycle + orchestrator entities under the legacy
/// `agentic_runtime::entity::*` path.
pub mod entity {
    pub use crate::lifecycle::entity::{run, run_event, run_suspension, schedule};
    pub use crate::orchestrator::entity::{task_outcome, task_queue};
}

/// Union of lifecycle + orchestrator CRUD modules under the legacy
/// `agentic_runtime::crud::*` path. Submodule paths and re-exported
/// free functions both stay reachable so callers don't have to choose
/// between `agentic_runtime::crud::insert_event` and
/// `agentic_runtime::crud::events::insert_event`.
pub mod crud {
    pub use crate::lifecycle::crud::{
        AirwayTableSummary, AutomationStepSummary, EventRow, LlmTokenSummary, LlmTokenSummaryByRun,
        ScheduleDurationBaseline, ThreadHistoryTurn, ToolExchangeRow, airway_table_summary_for_run,
        automation_step_summary_for_run, batch_insert_events, delete_events_from_seq,
        fetch_duration_baselines, get_all_events, get_all_events_for_runs, get_effective_run_state,
        get_events_after, get_max_seq, get_run, get_run_by_thread, get_runs_by_thread,
        get_suspension, get_suspension_with_start, get_thread_history,
        get_thread_history_with_events, heartbeat_driver, insert_event, insert_run,
        insert_run_with_parent, insert_run_with_schedule, is_cancel_requested, list_active_runs,
        list_recent_runs, list_runs_filtered, llm_usage_for_run, llm_usage_for_runs,
        load_task_tree, load_task_tree_in_workspace, release_driver, request_cancel,
        runs_in_workspace, try_acquire_driver, update_run_done, update_run_failed,
        update_run_running, update_run_suspended, update_run_terminal_from_events,
        update_task_status, upsert_suspension,
    };
    pub use crate::lifecycle::crud::{
        DRIVER_LEASE_TTL_SECS, clear_run_error, now, reset_run_for_retry, transition_run,
        user_facing_status,
    };
    pub use crate::lifecycle::crud::{events, queries, runs, suspension};
    pub use crate::orchestrator::crud::{
        DeadTask, DeferOutcome, QueueStats, QueueTaskRow, ReapOutcome, StuckRun,
        TASKS_DEAD_LETTERED, TASKS_REQUEUED, TaskScope, TerminalWrite, cancel_queued_task,
        cancel_queued_task_owned, cancel_queued_tasks_for_run, claim_task, claim_task_under_root,
        cleanup_stale_runs, complete_child_done_txn, complete_child_failed_txn,
        complete_queue_task, defer_task, drain_claims_for_worker, enqueue_task,
        fail_orphaned_claim, fail_queue_task, find_pending_global_runs, find_stuck_automation_runs,
        find_stuck_runs, get_active_root_runs, get_max_child_counter, get_outcomes_for_parent,
        get_queue_entry, get_queue_stats, get_resumable_root_runs, get_run_answer,
        increment_attempt, insert_child_run, insert_task_outcome, mark_recovery_failed,
        mark_released_roots_global, mark_task_global, purge_old_terminal_tasks, reap_stale_tasks,
        release_claim, release_claims_for_worker, requeue_task, reset_task_to_queued, retire_run,
        set_terminal_status_owned, suspend_with_data_txn, update_queue_heartbeat,
    };
    pub use crate::orchestrator::crud::{outcomes, queue, recovery};
}
