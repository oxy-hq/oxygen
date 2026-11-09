//! Atomic write-point for a workflow decision boundary.
//!
//! A workflow decision ("decide what to do next, write the result") used to be
//! a scatter of independent writes: `update_state_in_txn` for the CAS, event
//! inserts through an mpsc channel + bridge, and — only after the worker's
//! outcome returned to the coordinator — the queue-status flip on the decision
//! task. Any failure or silent drop between those writes could advance the
//! workflow state without persisting the matching events or enqueueing the
//! follow-up, stranding the workflow indefinitely.
//!
//! [`commit_decision`] collapses the whole boundary into one SeaORM
//! transaction, conditional on `decision_version`. If the transaction commits,
//! state + events + (for terminal decisions) run-status + queue-status all
//! advanced together. If anything fails, nothing persists and the next worker
//! claim can retry cleanly.
//!
//! This is the same pattern Temporal calls `UpdateAutomationExecutionAsActive`
//! (`service/history/workflow/transaction_impl.go`): history events, mutable
//! state, and transfer-queue tasks are written conditionally on the stored
//! `dbRecordVersion`, so a workflow task is either fully applied or fully
//! rolled back.

use agentic_core::delegation::{ClaimEpoch, NO_CLAIM};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;

use super::AutomationRunState;
use super::crud::apply_result_delta_in_txn;

/// A single decision-boundary commit.
///
/// **Invariant:** `run_id == new_state.run_id`. Both fields exist because the
/// commit writes to two tables (`agentic_run_events` keyed by `run_id`, and
/// `agentic_workflow_state` keyed by `new_state.run_id`). Callsites must keep
/// them in lockstep — a divergence would silently route events to the wrong
/// run while the state update succeeds.
#[derive(Debug)]
pub struct DecisionCommit {
    /// Automation run id — both the `agentic_runs.id` and `agentic_workflow_state.run_id`.
    /// Must equal `new_state.run_id`.
    pub run_id: String,
    /// Queue task id for the decision worker's claim. Flipped to `completed`
    /// or `failed` only for terminal variants; left untouched on `Continuing`.
    pub decision_task_id: String,
    /// Who holds `decision_task_id`'s queue claim. See [`DecisionClaim`] —
    /// production callers must pass [`DecisionClaim::HeldBy`], and the field
    /// is deliberately not defaultable so that choosing otherwise is visible
    /// at every construction site.
    pub claim: DecisionClaim,
    /// `decision_version` read alongside `new_state` — the CAS predicate.
    pub expected_version: i64,
    /// Post-decision state. Version is managed by the persistence layer; the
    /// decider does not set it.
    pub new_state: AutomationRunState,
    /// The single new step result produced by this decision, as a one-key JSON
    /// object `{"step_name": result}`.  Pass `serde_json::json!({})` when this
    /// decision did not produce a new result (delegation, wait-for-siblings).
    ///
    /// Used for an incremental JSONB merge (`results || delta`) so each decision
    /// writes O(1 result) instead of O(all results), eliminating the O(S²) write
    /// pattern that caused slowdowns for long and loop-heavy workflows.
    pub result_delta: Value,
    /// The single new step hash produced by this decision, paired with
    /// `result_delta`. Same shape: `{"step_name": "<hex>"}` when a step
    /// completed, `{}` otherwise. Merged into `step_hashes` in the same
    /// UPDATE as `result_delta` so a hash is never persisted without its
    /// matching result.
    pub step_hash_delta: Value,
    /// Events to append to `agentic_run_events`. Assigned monotonic seqs
    /// starting at `max(seq) + 1` within the transaction.
    pub events: Vec<(String, Value)>,
    /// `attempt` value to tag inserted events with.
    pub attempt: i32,
    /// How to finalize the decision task's queue row + the workflow run.
    pub terminal: DecisionTerminal,
}

/// Who owns the decision task's `agentic_task_queue` row for this commit.
///
/// The terminal branches of [`commit_decision`] stamp that row `completed` /
/// `failed`. Doing so by primary key alone is the same hazard the durable
/// transport's outcome path carries, and on the automation path it is the
/// *primary* durable-queue workload rather than an edge case:
///
///  1. this process claims the decision task and starts the inline step;
///  2. SIGTERM — graceful release flips the claim back to `queued`, NOTIFY
///     wakes the fleet, and a peer re-claims within milliseconds;
///  3. this process's inline step (an SQL or LLM call — seconds, not
///     microseconds) finally finishes and commits;
///  4. `completed` lands on **the peer's live claim**. If the peer then dies,
///     the reaper skips the row because it is terminal, the task is never
///     requeued, and the parent coordinator waits forever.
///
/// [`DecisionClaim::HeldBy`] scopes the write so step 4 matches nothing and
/// the whole transaction rolls back instead — run state must never advance
/// against a queue row this process no longer owns.
#[derive(Debug, Clone)]
pub enum DecisionClaim {
    /// This worker holds the decision task's claim; terminal queue writes are
    /// scoped to it. The id is the process worker identity
    /// (`agentic_runtime::transport::process_worker_id`) — the same value the
    /// claiming UPDATE wrote into `worker_id`.
    ///
    /// `claim_epoch` is the claim's identity (`TaskAssignment::claim_epoch`).
    /// `worker_id` is process-stable, so on its own it cannot tell this
    /// execution from this process's re-claim of the same row after a reap;
    /// the epoch can. [`NO_CLAIM`] fences on `worker_id` only.
    HeldBy {
        worker_id: String,
        claim_epoch: ClaimEpoch,
    },
    /// This commit is not driven by a durable-queue claim, so there is no
    /// ownership to check: the caller reached `commit_decision` without going
    /// through `recv_assignment` (tests that drive the decider directly), or
    /// this process has never constructed a `DurableTransport` and therefore
    /// provably holds no claims at all.
    ///
    /// Prefer [`DecisionClaim::HeldBy`] whenever a worker id is available —
    /// this variant restores the unscoped write and with it the hazard above.
    Unclaimed,
}

#[derive(Debug)]
pub enum DecisionTerminal {
    /// Automation continuing — decision queue row untouched. The worker will
    /// still emit a `Suspended`/`Done` outcome that drives the next action
    /// (child enqueue, chaining) via the coordinator.
    Continuing,
    /// Automation complete. Flips `agentic_runs.task_status` to `done` (with
    /// `final_answer`) and `agentic_task_queue.queue_status` to `completed`
    /// inside this transaction.
    CompleteAutomation { final_answer: String },
    /// Automation failed. Flips `agentic_runs.task_status` to `failed` (with
    /// `error`) and `agentic_task_queue.queue_status` to `failed`.
    FailAutomation { error: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    /// All writes committed atomically.
    Committed,
    /// The `decision_version` CAS predicate didn't match. Nothing persisted —
    /// the decision task that produced this commit is stale and should be
    /// discarded. Another worker already advanced the workflow.
    VersionConflict,
    /// A peer holds the decision task's queue claim (this process was
    /// gracefully released and the row re-claimed while the decision was still
    /// running). Nothing persisted — see [`DecisionClaim`]. Like
    /// [`CommitOutcome::VersionConflict`] this is not an error: the peer that
    /// owns the row will re-drive the decision from durable state, so the
    /// caller should stop quietly rather than fail the run.
    ClaimLost,
}

/// Atomically apply a workflow decision.
///
/// See module docs for the write ordering guarantee. On `CompleteAutomation`,
/// the state invariant `current_step >= workflow.tasks.len()` is enforced;
/// violating it is a decider bug and returns `DbErr::Custom` without writing
/// anything.
pub async fn commit_decision(
    db: &DatabaseConnection,
    commit: DecisionCommit,
) -> Result<CommitOutcome, DbErr> {
    debug_assert_eq!(
        commit.run_id, commit.new_state.run_id,
        "DecisionCommit.run_id must match new_state.run_id; \
         divergence would route events to a different run than the state update"
    );
    validate_invariants(&commit)?;

    let txn = db.begin().await?;

    let pending_json = serde_json::to_value(&commit.new_state.pending_children)
        .map_err(|e| DbErr::Custom(format!("serialize pending_children: {e}")))?;

    // Use the incremental delta path: merge only the new step result via
    // JSONB `||` rather than overwriting the full results map.  render_context
    // is always written as `{}` because it is derived from results at load time.
    let updated = apply_result_delta_in_txn(
        &txn,
        &commit.new_state.run_id,
        commit.expected_version,
        commit.new_state.current_step as i32,
        commit.result_delta,
        commit.step_hash_delta,
        pending_json,
    )
    .await?;
    if !updated {
        txn.rollback().await?;
        return Ok(CommitOutcome::VersionConflict);
    }

    append_events_in_txn(&txn, &commit.run_id, &commit.events, commit.attempt).await?;

    // Terminal branches stamp the decision task's queue row. Do that *before*
    // the run row so an unowned claim aborts the transaction without having
    // written anything that matters, and roll back rather than commit — the
    // whole point of the ownership check is that run state must not advance
    // against a row a peer is actively driving.
    let decision_row = DecisionRow {
        task_id: &commit.decision_task_id,
        run_id: &commit.run_id,
        claim: &commit.claim,
    };
    match &commit.terminal {
        DecisionTerminal::Continuing => {}
        DecisionTerminal::CompleteAutomation { final_answer } => {
            if !stamp_decision_queue_row(&txn, decision_row, "completed").await? {
                txn.rollback().await?;
                return Ok(CommitOutcome::ClaimLost);
            }
            set_run_terminal_in_txn(&txn, &commit.run_id, "done", Some(final_answer), None).await?;
        }
        DecisionTerminal::FailAutomation { error } => {
            if !stamp_decision_queue_row(&txn, decision_row, "failed").await? {
                txn.rollback().await?;
                return Ok(CommitOutcome::ClaimLost);
            }
            set_run_terminal_in_txn(&txn, &commit.run_id, "failed", None, Some(error)).await?;
        }
    }

    txn.commit().await?;
    Ok(CommitOutcome::Committed)
}

/// Stamp the decision task's queue row terminal inside the commit transaction.
///
/// Returns `false` when a *different* worker holds the row, which the caller
/// must turn into a full rollback. Every other outcome is `true`:
///
/// - already terminal under our own ownership (the requester cancelled the run
///   a beat before this decision landed) — the queue keeps its first terminal
///   status, but the run state this decision computed is still ours to commit;
/// - no queue row at all — the caller never went through the durable queue, so
///   there is nothing to own and nothing to protect.
async fn stamp_decision_queue_row(
    txn: &DatabaseTransaction,
    row: DecisionRow<'_>,
    status: &str,
) -> Result<bool, DbErr> {
    use agentic_runtime::crud::TerminalWrite;

    match row.claim {
        DecisionClaim::HeldBy {
            worker_id,
            claim_epoch,
        } => {
            let write = agentic_runtime::crud::set_terminal_status_owned(
                txn,
                row.task_id,
                worker_id,
                (*claim_epoch != NO_CLAIM).then_some(*claim_epoch),
                status,
            )
            .await?;
            match write {
                TerminalWrite::Stamped | TerminalWrite::AlreadyTerminal | TerminalWrite::NoRow => {
                    Ok(true)
                }
                TerminalWrite::NotOwned => {
                    tracing::warn!(
                        run_id = %row.run_id,
                        task_id = %row.task_id,
                        worker_id = %worker_id,
                        "decision commit aborted: another worker now holds this claim"
                    );
                    Ok(false)
                }
            }
        }
        DecisionClaim::Unclaimed => {
            set_queue_status_in_txn(txn, row.task_id, status).await?;
            Ok(true)
        }
    }
}

/// The three borrowed fields [`stamp_decision_queue_row`] needs, grouped so it
/// can be called after `commit` has been partially moved into
/// `apply_result_delta_in_txn`.
#[derive(Clone, Copy)]
struct DecisionRow<'a> {
    task_id: &'a str,
    run_id: &'a str,
    claim: &'a DecisionClaim,
}

fn validate_invariants(commit: &DecisionCommit) -> Result<(), DbErr> {
    if let DecisionTerminal::CompleteAutomation { .. } = &commit.terminal {
        let total = commit.new_state.workflow.tasks.len();
        if commit.new_state.current_step < total {
            return Err(DbErr::Custom(format!(
                "CompleteAutomation invariant: current_step ({}) < tasks.len ({})",
                commit.new_state.current_step, total
            )));
        }
    }
    Ok(())
}

/// Append the decision's events inside the commit's transaction.
///
/// Through `agentic_runtime::crud::append_events`, which is one multi-row
/// `INSERT` carrying the `NOTIFY oxy_run_events` the run's streams park on:
/// a queued automation run is driven on whichever pod claimed it, and the
/// pod serving its stream learns of these rows from that notification. A
/// raw insert here would leave that stream on its safety-net timer.
async fn append_events_in_txn(
    txn: &DatabaseTransaction,
    run_id: &str,
    events: &[(String, Value)],
    attempt: i32,
) -> Result<(), DbErr> {
    if events.is_empty() {
        return Ok(());
    }
    let next_seq = next_seq_in_txn(txn, run_id).await?;
    agentic_runtime::crud::append_events(txn, run_id, next_seq, events, attempt).await
}

async fn next_seq_in_txn(txn: &DatabaseTransaction, run_id: &str) -> Result<i64, DbErr> {
    #[derive(FromQueryResult)]
    struct MaxSeqRow {
        max_seq: Option<i64>,
    }

    let row = MaxSeqRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT MAX(seq) AS max_seq FROM agentic_run_events WHERE run_id = $1",
        [run_id.into()],
    ))
    .one(txn)
    .await?;
    Ok(row.and_then(|r| r.max_seq).map(|s| s + 1).unwrap_or(0))
}

async fn set_run_terminal_in_txn(
    txn: &DatabaseTransaction,
    run_id: &str,
    status: &str,
    answer: Option<&str>,
    error: Option<&str>,
) -> Result<(), DbErr> {
    let sql = "\
        UPDATE agentic_runs \
        SET task_status = $1, \
            answer = COALESCE($2, answer), \
            error_message = COALESCE($3, error_message), \
            updated_at = $4 \
        WHERE id = $5";
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        [
            status.into(),
            answer.into(),
            error.into(),
            chrono::Utc::now().fixed_offset().into(),
            run_id.into(),
        ],
    );
    txn.execute_raw(stmt).await?;
    // A terminal decision with no events to append still changes what the
    // run's streams should do; Postgres collapses this with the append's
    // notification when both are in this transaction.
    agentic_runtime::crud::notify_run_events(txn, run_id).await?;
    Ok(())
}

/// Unscoped queue-status write, used only on the [`DecisionClaim::Unclaimed`]
/// path. Do not reach for this from a caller that holds a claim — use
/// `agentic_runtime::crud::set_terminal_status_owned` (via
/// [`stamp_decision_queue_row`]) so a peer's live claim can't be stamped.
async fn set_queue_status_in_txn(
    txn: &DatabaseTransaction,
    task_id: &str,
    status: &str,
) -> Result<(), DbErr> {
    let sql = "\
        UPDATE agentic_task_queue \
        SET queue_status = $1, updated_at = $2 \
        WHERE task_id = $3";
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        [
            status.into(),
            chrono::Utc::now().fixed_offset().into(),
            task_id.into(),
        ],
    );
    txn.execute_raw(stmt).await?;
    Ok(())
}
