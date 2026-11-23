//! The memory merge for served chats, queued on the worker fleet.
//!
//! A served run's reflection turn may add notes, and the prompt lists at most
//! [`agentic_runtime::react::notes_section`]'s caps, oldest dropped first. So
//! after a served run that could write notes ends `Done`, [`MergeTrigger`]
//! re-reads the owner's notes and, when they no longer fit, queues one
//! `react_memory_merge` task for that owner, unless one is already queued or
//! claimed. A missed trigger heals on the next run that writes.
//!
//! The task ([`run_memory_merge`]) runs as the user who owns the notes: the
//! host rebuilds the triggering run's requester context, and the merge asks
//! that run's agent model. The model is asked with no lock held; the apply
//! checks that no run changed the old notes meanwhile
//! (`DbMemoryStore::apply_merge`), so concurrent chats are safe.
//!
//! The payload and the run row hold ids and counts only, never note text.

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud::{TaskScope, enqueue_task};
use agentic_runtime::lifecycle::crud::runs::insert_run;
use agentic_runtime::react::{
    DbMemoryStore, MemoryStore, MergeError, MergeReport, NoteOwner, fits, latest, merge_notes,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::platform::PlatformContext;

/// The task kind and the run's `source_type`.
pub const MEMORY_MERGE_KIND: &str = "react_memory_merge";

/// What a merge task carries: whose notes, which agent's model, and the run
/// whose requester the task runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryMergeRequest {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub agent_id: String,
    pub source_run_id: String,
}

impl MemoryMergeRequest {
    pub fn owner(&self) -> NoteOwner {
        NoteOwner {
            workspace_id: self.workspace_id,
            user_id: self.user_id,
        }
    }
}

/// Set on a served run that may write notes ([`crate::react_served`]).
#[derive(Clone)]
pub struct MergeTrigger {
    pub db: DatabaseConnection,
    pub owner: NoteOwner,
    pub agent_id: String,
}

impl MergeTrigger {
    /// After the run `run_id` ended `Done`: queues a merge when the owner's
    /// notes overflow the prompt. Never fails the run; a failure is logged.
    pub async fn after_done(&self, run_id: &str) {
        let store = DbMemoryStore::new(self.db.clone(), self.owner, None);
        let overflows = match store.facts().await {
            Ok(facts) => !fits(&latest(facts)),
            Err(e) => {
                return tracing::warn!(run_id, error = %e, "react memory: notes unreadable; no merge queued");
            }
        };
        if !overflows {
            return;
        }
        let request = MemoryMergeRequest {
            workspace_id: self.owner.workspace_id,
            user_id: self.owner.user_id,
            agent_id: self.agent_id.clone(),
            source_run_id: run_id.to_string(),
        };
        match enqueue_memory_merge(&self.db, &request).await {
            Ok(Some(merge_run)) => tracing::info!(run_id, merge_run, "react memory: merge queued"),
            Ok(None) => {}
            Err(e) => tracing::warn!(run_id, error = %e, "react memory: merge not queued"),
        }
    }
}

/// Queues a merge for `request`'s owner and returns its run id, or `None`
/// when one is already queued or claimed for that owner.
pub async fn enqueue_memory_merge(
    db: &DatabaseConnection,
    request: &MemoryMergeRequest,
) -> Result<Option<String>, String> {
    if merge_is_pending(db, request.owner()).await? {
        return Ok(None);
    }
    let run_id = Uuid::new_v4().to_string();
    let spec = TaskSpec::Custom {
        kind: MEMORY_MERGE_KIND.into(),
        payload: serde_json::to_value(request).map_err(|e| e.to_string())?,
    };
    // The run row and its queue row commit together: a run committed alone
    // is "never started" to a booting server's cleanup, which fails it.
    let txn = db.begin().await.map_err(|e| e.to_string())?;
    let question = "Merge memory notes";
    insert_run(
        &txn,
        &run_id,
        question,
        None,
        MEMORY_MERGE_KIND,
        None,
        request.workspace_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global)
        .await
        .map_err(|e| e.to_string())?;
    txn.commit().await.map_err(|e| e.to_string())?;
    Ok(Some(run_id))
}

/// Whether a merge for `owner` is queued or claimed. A claimed one counts:
/// a second would ask the model again for notes the first already merges.
async fn merge_is_pending(db: &impl ConnectionTrait, owner: NoteOwner) -> Result<bool, String> {
    #[derive(FromQueryResult)]
    struct Pending {
        pending: i64,
    }
    let row = Pending::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*)::int8 AS pending FROM agentic_task_queue \
         WHERE queue_status IN ('queued', 'claimed') \
           AND spec->>'kind' = $1 \
           AND spec->'payload'->>'workspace_id' = $2 \
           AND spec->'payload'->>'user_id' = $3",
        [
            MEMORY_MERGE_KIND.into(),
            owner.workspace_id.to_string().into(),
            owner.user_id.to_string().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.is_some_and(|r| r.pending > 0))
}

/// What a merge task did.
#[derive(Debug)]
pub enum MergeOutcome {
    Merged(MergeReport),
    /// The notes fit the prompt: no model call.
    Fit,
    /// A run changed a note the plan replaced: nothing written.
    Changed,
    /// The org's `react_memory` flag is off now: nothing done.
    Off,
}

impl MergeOutcome {
    /// The task's answer: counts and usage, no note text.
    pub fn summary(&self) -> serde_json::Value {
        match self {
            Self::Merged(r) => serde_json::json!({
                "merged": true, "before": r.before, "after": r.after,
                "output_tokens": r.usage.output_tokens, "cost_usd": r.usage.cost_usd,
            }),
            Self::Fit => serde_json::json!({ "merged": false, "reason": "fits" }),
            Self::Changed => serde_json::json!({ "merged": false, "reason": "changed" }),
            Self::Off => serde_json::json!({ "merged": false, "reason": "flag off" }),
        }
    }
}

/// Merges `request`'s owner's notes with `request.agent_id`'s model, on
/// `platform` (the triggering run's requester context).
pub async fn run_memory_merge(
    platform: &dyn PlatformContext,
    db: &DatabaseConnection,
    request: &MemoryMergeRequest,
) -> Result<MergeOutcome, String> {
    if !platform.react_memory_enabled().await {
        return Ok(MergeOutcome::Off);
    }
    let engine = crate::headless::memory_merge::memory_engine(platform, &request.agent_id).await?;
    merge_with(&*engine, db, request).await
}

/// [`run_memory_merge`]'s merge, with `engine` given.
pub async fn merge_with(
    engine: &dyn agentic_core::react::Engine,
    db: &DatabaseConnection,
    request: &MemoryMergeRequest,
) -> Result<MergeOutcome, String> {
    let store = DbMemoryStore::new(db.clone(), request.owner(), None);
    let run_id = format!("merge-{}", request.source_run_id);
    match merge_notes(engine, &store, &run_id).await {
        Ok(Some(report)) => Ok(MergeOutcome::Merged(report)),
        Ok(None) => Ok(MergeOutcome::Fit),
        Err(MergeError::Changed) => Ok(MergeOutcome::Changed),
        Err(e) => Err(e.to_string()),
    }
}
