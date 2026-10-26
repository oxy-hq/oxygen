//! Orchestrator-side CRUD: the durable task queue, per-child outcomes,
//! and startup recovery enumeration.
//!
//! Outcomes/recovery touch the `agentic_runs` row (lifecycle-owned)
//! transactionally with the orchestrator's tables; they pull `now()`
//! and `transition_run` from `lifecycle::crud` rather than duplicating
//! them.

pub mod adopt;
pub mod boot_cleanup;
pub mod dead_letter;
pub mod outcomes;
pub mod queue;
pub mod recovery;
pub mod retry;

pub use adopt::adopt_queued_task;
pub use boot_cleanup::cleanup_stale_runs;
pub use dead_letter::{
    DeadLetterRun, deferral_dead_letter_message, fail_dead_lettered_run, reaper_dead_letter_message,
};
pub use outcomes::{
    complete_child_done_txn, complete_child_failed_txn, get_outcomes_for_parent, get_run_answer,
    insert_child_run, insert_task_outcome, suspend_with_data_txn,
};
pub use queue::{
    DeadTask, DeferOutcome, HeartbeatOutcome, QueueStats, QueueTaskRow, ReapOutcome,
    TASKS_DEAD_LETTERED, TASKS_REQUEUED, TaskScope, TerminalWrite, cancel_queued_task,
    cancel_queued_task_owned, cancel_queued_tasks_for_run, claim_task, claim_task_under_root,
    complete_queue_task, defer_task, drain_claims_for_worker, enqueue_task, fail_orphaned_claim,
    fail_queue_task, get_queue_entry, get_queue_stats, heartbeat_queue_task,
    mark_released_roots_global, mark_task_global, purge_old_terminal_tasks, reap_stale_tasks,
    release_claim, release_claims_for_worker, requeue_task, reset_task_to_queued,
    set_terminal_status_owned, update_queue_heartbeat,
};
pub use recovery::{
    StuckRun, complete_run_with_lost_outcome, find_pending_global_runs, find_stuck_automation_runs,
    find_stuck_runs, get_active_root_runs, get_max_child_counter, get_resumable_root_runs,
    increment_attempt, mark_recovery_failed, owning_run_id, retire_run,
};
pub use retry::{
    RetryState, cancel_waiting_retry, final_failure_message, requeue_failed_attempt, retry_state,
};
