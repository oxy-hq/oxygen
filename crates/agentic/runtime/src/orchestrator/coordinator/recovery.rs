//! Rebuild the coordinator's in-memory state from persisted runs after a restart.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::delegation::{TaskPolicy, TaskSpec};
use agentic_core::transport::CoordinatorTransport;
use sea_orm::DatabaseConnection;

use crate::crud;
use crate::lifecycle::state::RuntimeState;

use super::{
    ChildResult, Coordinator, DEFAULT_DRAIN_TIMEOUT, DEFAULT_SUSPEND_TIMEOUT, TaskNode, TaskStatus,
};

/// A parent task that needs to be resumed after crash recovery because all
/// its children completed (outcomes found in `agentic_task_outcomes`) but the
/// parent was never resumed before the crash.
#[derive(Debug)]
pub struct PendingResume {
    pub parent_task_id: String,
    pub answer: String,
    /// Whether the coordinator holds a checkpoint to resume this parent from.
    /// Without one `resume_parent` can only log and leave it waiting, so a
    /// caller deciding who continues the parent must not count on the resume.
    pub has_checkpoint: bool,
    /// Set by the caller, never by `from_db`: this resume was assigned before
    /// the crash and its queue entry survived. `from_db` cannot tell — the
    /// outcomes are the same rows, and a boot may have stamped the parent
    /// `needs_resume` — so it reports the parent as pending either way.
    /// `process_pending_resumes` then records the task as running instead of
    /// assigning the resume a second time over the one a worker is claiming.
    pub already_assigned: bool,
}

/// The outcomes on record for the children `parent_id` is waiting on **now**.
///
/// `agentic_task_outcomes` keeps every outcome a parent has ever been sent,
/// and a parent that delegates more than once — an automation does, once per
/// step — still has the rows from its earlier delegations. Only the children
/// of the delegation it is parked at count. Counting all of them made a parent
/// whose one child was still running read as "every child in" (one old outcome
/// against one expected child), and recovery resumed it with an answer
/// aggregated from children that had not reported.
async fn completed_children(
    db: &DatabaseConnection,
    parent_id: &str,
    child_task_ids: &[String],
) -> Result<HashMap<String, ChildResult>, sea_orm::DbErr> {
    let outcomes = crud::get_outcomes_for_parent(db, parent_id).await?;
    Ok(outcomes
        .into_iter()
        .filter(|o| child_task_ids.contains(&o.child_id))
        .map(|o| {
            let result = match o.status.as_str() {
                "done" => ChildResult::Done(o.answer.unwrap_or_default()),
                _ => ChildResult::Failed(o.answer.unwrap_or_default()),
            };
            (o.child_id, result)
        })
        .collect())
}

impl Coordinator {
    /// Reconstruct a coordinator from persisted task tree state.
    ///
    /// Loads the task tree for `root_run_id` from the database and rebuilds
    /// the in-memory `tasks` map. Event sequence counters are derived from
    /// `get_max_seq`. Suspended tasks get a fresh timeout clock.
    ///
    /// **Crash-consistency**: For tasks in `WaitingOnChildren`, the `completed`
    /// map is rebuilt from the `agentic_task_outcomes` table (the atomic source
    /// of truth), not from `task_metadata` JSONB. This closes the window where
    /// a child completes but the parent's metadata hasn't been updated yet.
    ///
    /// After rebuilding, any parent whose children are all terminal will be
    /// detected and queued for resume via the returned `pending_resumes` list.
    pub async fn from_db(
        db: DatabaseConnection,
        state: Arc<RuntimeState>,
        transport: Arc<dyn CoordinatorTransport>,
        root_run_id: &str,
    ) -> Result<(Self, Vec<PendingResume>), sea_orm::DbErr> {
        let tree = crud::load_task_tree(&db, root_run_id).await?;
        let mut tasks = HashMap::new();

        for row in &tree {
            let next_seq = crud::get_max_seq(&db, &row.id).await? + 1;

            let status = match row.task_status.as_deref() {
                Some("running") => TaskStatus::Running,
                Some("awaiting_input") => TaskStatus::SuspendedHuman,
                Some("delegating") => {
                    let meta = row.task_metadata.as_ref();

                    // Get child_task_ids from metadata (needed for the list of
                    // expected children).
                    let child_task_ids: Vec<String> = meta
                        .and_then(|m| m["child_task_ids"].as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(ToString::to_string))
                                .collect()
                        })
                        .or_else(|| {
                            // Legacy single-child format.
                            meta.and_then(|m| m["child_task_id"].as_str())
                                .map(|id| vec![id.to_string()])
                        })
                        .unwrap_or_default();

                    // Rebuild `completed` from the task_outcomes table — the
                    // atomic source of truth — instead of task_metadata JSONB.
                    let completed = completed_children(&db, &row.id, &child_task_ids).await?;

                    let failure_policy = meta
                        .and_then(|m| serde_json::from_value(m["failure_policy"].clone()).ok())
                        .unwrap_or_default();

                    TaskStatus::WaitingOnChildren {
                        child_task_ids,
                        completed,
                        failure_policy,
                    }
                }
                Some("done") => TaskStatus::Done,
                Some("failed") => TaskStatus::Failed,

                // "needs_resume" / "shutdown" / "running" — check if this task
                // has children in the tree. If so, it was delegating before the
                // crash and the reaper changed its status; reconstruct as
                // WaitingOnChildren so the coordinator correctly waits for
                // children to complete before resuming this task.
                _ => {
                    let has_children = tree
                        .iter()
                        .any(|t| t.parent_run_id.as_deref() == Some(&row.id));
                    if has_children {
                        let meta = row.task_metadata.as_ref();
                        let child_task_ids: Vec<String> = meta
                            .and_then(|m| m["child_task_ids"].as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(ToString::to_string))
                                    .collect()
                            })
                            .or_else(|| {
                                // Derive from tree if metadata is missing.
                                Some(
                                    tree.iter()
                                        .filter(|t| t.parent_run_id.as_deref() == Some(&row.id))
                                        .map(|t| t.id.clone())
                                        .collect(),
                                )
                            })
                            .unwrap_or_default();

                        let completed = completed_children(&db, &row.id, &child_task_ids).await?;

                        let failure_policy = meta
                            .and_then(|m| serde_json::from_value(m["failure_policy"].clone()).ok())
                            .unwrap_or_default();

                        TaskStatus::WaitingOnChildren {
                            child_task_ids,
                            completed,
                            failure_policy,
                        }
                    } else {
                        TaskStatus::Running
                    }
                }
            };

            // Carry the suspend clock across the restart instead of restarting
            // it.
            //
            // `suspended_at` is a `tokio::time::Instant` — process-local, so a
            // recovered task used to get a fresh full timeout. Airway's
            // single-flight lease, the other bound on the same stuck pipeline,
            // is an *absolute* `expires_at`. Resetting therefore let the two
            // invert: a restart three hours into a suspension gave the
            // coordinator four more (seven total) against a lease expiring at
            // six, so the lease fired first and the operator lost the named
            // failure — exactly what the ordering tests in
            // `agentic-pipeline::airway_run` exist to prevent, and something
            // those constant-level tests cannot see. At the old 30-minute
            // ceiling a reset cost 30 minutes; at 4h it costs 4h, and a deploy
            // cadence under 4h meant the ceiling could never fire at all.
            //
            // One read for both the clock and the checkpoint — two `find_by_id`
            // calls against the same row could disagree.
            let suspension = match &status {
                TaskStatus::SuspendedHuman | TaskStatus::WaitingOnChildren { .. } => {
                    crud::get_suspension_with_start(&db, &row.id).await?
                }
                _ => None,
            };

            // `elapsed` is clamped to the ceiling itself, which is what makes
            // `checked_sub` total: subtracting at most 4h from `Instant::now()`
            // is always representable, so the `unwrap_or_else` below is
            // unreachable rather than merely unlikely. That matters because its
            // only sane fallback is `Instant::now()` — a fresh full timeout,
            // i.e. precisely the reset this whole block removes. Without the
            // clamp, a long outage (a run suspended for days, recovered on a
            // machine booted an hour ago) could reach it.
            //
            // "At most fully elapsed" is also the semantics we want: a task
            // suspended for longer than the ceiling should time out on the next
            // check, not be handed more time.
            //
            // A missing row, or one somehow stamped in the future, yields zero
            // elapsed — "just suspended" — via `to_std()` failing on a negative
            // delta. Note `suspended_at` stays `Some` for any suspended status
            // even without a row: `check_suspend_timeouts` skips `None`, so
            // returning `None` here would mean the task could never time out.
            let suspended_at = match &status {
                TaskStatus::SuspendedHuman | TaskStatus::WaitingOnChildren { .. } => {
                    let elapsed = suspension
                        .as_ref()
                        .and_then(|(started, _)| (crud::now() - *started).to_std().ok())
                        .unwrap_or_default()
                        .min(DEFAULT_SUSPEND_TIMEOUT);
                    Some(
                        tokio::time::Instant::now()
                            .checked_sub(elapsed)
                            .unwrap_or_else(tokio::time::Instant::now),
                    )
                }
                _ => None,
            };

            // An unparseable checkpoint yields `None` here — the task cannot be
            // resumed — but the clock above still ran, so it reaches the ceiling
            // instead of being renewed forever. See `get_suspension_with_start`.
            let suspend_data = suspension.and_then(|(_, data)| data);

            // Restore retry state from task_metadata if present.
            let meta = row.task_metadata.as_ref();
            let attempt = meta.and_then(|m| m["attempt"].as_u64()).unwrap_or(0) as u32;
            let fallback_index =
                meta.and_then(|m| m["fallback_index"].as_u64()).unwrap_or(0) as usize;
            let policy: Option<TaskPolicy> =
                meta.and_then(|m| serde_json::from_value(m["policy"].clone()).ok());
            let original_spec: Option<TaskSpec> =
                meta.and_then(|m| serde_json::from_value(m["original_spec"].clone()).ok());

            tasks.insert(
                row.id.clone(),
                TaskNode {
                    run_id: row.id.clone(),
                    parent_task_id: row.parent_run_id.clone(),
                    status,
                    suspend_data,
                    next_seq,
                    suspended_at,
                    original_spec,
                    policy,
                    attempt,
                    fallback_index,
                    // Recovery doesn't reconstruct loop_iteration —
                    // it's only used live by record_child_result to
                    // emit progress events, and a recovered run that
                    // was mid-fanout will re-emit all per-step
                    // events when its decider next runs. Worst case:
                    // the progress bar misses incremental updates
                    // for iterations that completed before the crash
                    // and shows them all at once when the next
                    // commit_decision lands.
                    loop_iteration: None,
                },
            );
        }

        // Detect parents whose children are all terminal — these need to be
        // resumed. This handles the crash window where a child's outcome was
        // written to `agentic_task_outcomes` but the parent was never resumed.
        let mut pending_resumes = Vec::new();
        let task_ids: Vec<String> = tasks.keys().cloned().collect();
        for task_id in &task_ids {
            let all_done = {
                let Some(node) = tasks.get(task_id) else {
                    continue;
                };
                match &node.status {
                    TaskStatus::WaitingOnChildren {
                        child_task_ids,
                        completed,
                        ..
                    } => !child_task_ids.is_empty() && completed.len() >= child_task_ids.len(),
                    _ => false,
                }
            };

            if all_done {
                // Aggregate the answer exactly as the live code path does.
                let answer = Self::aggregate_child_results_static(&tasks, task_id);
                let has_checkpoint = tasks
                    .get(task_id)
                    .is_some_and(|node| node.suspend_data.is_some());
                pending_resumes.push(PendingResume {
                    parent_task_id: task_id.clone(),
                    answer,
                    has_checkpoint,
                    already_assigned: false,
                });
            }
        }

        // Query DB for the max child counter across ALL runs in the tree,
        // not just the ones loaded into the tasks HashMap. This prevents PK
        // collisions when previous recovery attempts created children that
        // may not be in the current tree (e.g., if they were orphaned).
        let max_counter = crud::get_max_child_counter(&db, root_run_id).await?;

        // Get attempt from root run (already incremented by recovery caller).
        let root_run = crud::get_run(&db, root_run_id).await?;
        let attempt = root_run.map(|r| r.attempt).unwrap_or(0);

        Ok((
            Self {
                db,
                state,
                transport,
                tasks,
                child_counter: max_counter,
                attempt,
                answer_rxs: HashMap::new(),
                suspend_timeout: DEFAULT_SUSPEND_TIMEOUT,
                drain_timeout: DEFAULT_DRAIN_TIMEOUT,
                // Recovered coordinators get the default policy +
                // resolver; the pipeline-side recovery wrapper calls
                // `.with_completion_policy(...)` and
                // `.with_delegation_resolver(...)` on the returned
                // Coordinator before driving it.
                completion_policy: Arc::new(super::DefaultCompletionPolicy),
                delegation_resolver: Arc::new(super::DefaultDelegationResolver),
            },
            pending_resumes,
        ))
    }

    /// Leave `task_id` as `resume_parent` does, minus what belongs to an
    /// assignment that already happened (the status write, the
    /// `input_resolved` event, the queue entry). See
    /// [`PendingResume::already_assigned`].
    pub(super) fn resume_already_assigned(&mut self, task_id: &str) {
        let Some(node) = self.tasks.get_mut(task_id) else {
            return;
        };
        node.status = TaskStatus::Running;
        node.suspended_at = None;
        node.suspend_data = None;
    }

    /// Aggregate child results without needing `&self` (used during recovery).
    fn aggregate_child_results_static(
        tasks: &HashMap<String, TaskNode>,
        parent_id: &str,
    ) -> String {
        let Some(parent_node) = tasks.get(parent_id) else {
            return "No results".to_string();
        };
        let TaskStatus::WaitingOnChildren {
            child_task_ids,
            completed,
            ..
        } = &parent_node.status
        else {
            return "No results".to_string();
        };

        // Single-child: return the answer directly (backward compatible).
        if child_task_ids.len() == 1
            && let Some(result) = completed.get(&child_task_ids[0])
        {
            return match result {
                ChildResult::Done(a) => a.clone(),
                ChildResult::Failed(msg) => format!("Delegation failed: {msg}"),
            };
        }

        // Multi-child: aggregate as JSON, fan-out-ordered. Resolve each
        // child's original loop iteration index (when present) so a
        // recovered partial fan-out aggregates the same way the live
        // path does.
        let aggregated = Self::serialize_completed(completed, child_task_ids, |id| {
            tasks
                .get(id)
                .and_then(|n| n.loop_iteration.as_ref())
                .map(|m| m.index)
        });
        serde_json::to_string(&aggregated).unwrap_or_else(|_| "{}".to_string())
    }
}
