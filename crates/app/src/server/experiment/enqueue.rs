//! Enqueueing a price request: one job per candidate.
//!
//! Nothing here runs pricing: the handler's job is to insert every candidate's
//! `pending` proposal, write each run row and queue row in one transaction, and
//! return the ids. The worker fleet does the rest
//! (`.claude/skills/oxy-task-spec-default/SKILL.md`).

use agentic_core::delegation::TaskSpec;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use oxy_experiment::ExperimentConfig;
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use uuid::Uuid;

use super::candidate::Candidate;
use super::design_store;
use super::proposal_store::{self, NewPending};
use super::{EXPERIMENT_PRICE_KIND, ExperimentPricePayload};

pub struct StartInput {
    pub workspace_id: Uuid,
    pub name: String,
    pub requested_by: Uuid,
    pub candidates: Vec<Candidate>,
}

/// The ids a request produced; `proposal_ids` are in candidate order.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub request_id: Uuid,
    pub proposal_ids: Vec<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("no experiment named `{0}`")]
    NotFound(String),
    #[error("the design of `{0}` could not be read")]
    Unreadable(String),
    #[error("database: {0}")]
    Db(#[from] DbErr),
    #[error("encoding the request: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Queue one price job per candidate; each proposal's id is also its run and task id.
///
/// The pending proposals, the run rows and the queue rows are written in one
/// transaction — each run row before its queue row, because
/// `agentic_task_queue.run_id` has an FK to `agentic_runs.id`
/// (`crates/app/src/server/api/simulation/runs.rs:377-383`) — so a caller
/// polling the proposal list finds every row the instant this returns, and a
/// failure part-way leaves nothing behind.
pub async fn start_price(
    db: &DatabaseConnection,
    input: StartInput,
) -> Result<Started, StartError> {
    // Pricing prices the draft; the proposal remembers its hash, so a later edit
    // shows it priced an earlier draft.
    let Some(experiment) =
        design_store::experiment_by_name(db, input.workspace_id, &input.name).await?
    else {
        return Err(StartError::NotFound(input.name));
    };
    let Some(design) = design_store::design_of(db, experiment.id).await? else {
        return Err(StartError::NotFound(input.name));
    };
    let (design_id, design_hash) = (design.id, design.content_hash.clone());
    let file_alpha = serde_json::from_value::<ExperimentConfig>(design.config)
        .map_err(|_| StartError::Unreadable(input.name.clone()))?
        .outcome
        .alpha;
    let request_id = Uuid::new_v4();
    let txn = db.begin().await?;
    let total = input.candidates.len();
    let mut proposal_ids = Vec::with_capacity(total);
    for (i, candidate) in input.candidates.iter().enumerate() {
        let proposal_id = Uuid::new_v4();
        let id = proposal_id.to_string();
        proposal_store::insert_pending(
            &txn,
            NewPending {
                id: proposal_id,
                request_id,
                workspace_id: input.workspace_id,
                name: input.name.clone(),
                design_id,
                design_hash: design_hash.clone(),
                request: serde_json::to_value(candidate)?,
                alpha: candidate.alpha().unwrap_or(file_alpha),
                requested_by: input.requested_by,
            },
        )
        .await?;
        agentic_runtime::crud::insert_run(
            &txn,
            &id,
            &format!(
                "price experiment {} (candidate {} of {total})",
                input.name,
                i + 1
            ),
            None,
            EXPERIMENT_PRICE_KIND,
            Some(serde_json::json!({
                "workspace_id": input.workspace_id,
                "experiment": input.name,
                "proposal_id": proposal_id,
                "request_id": request_id,
            })),
            input.workspace_id,
        )
        .await?;
        let spec = TaskSpec::Custom {
            kind: EXPERIMENT_PRICE_KIND.to_string(),
            payload: serde_json::to_value(ExperimentPricePayload { proposal_id })?,
        };
        agentic_runtime::crud::enqueue_task(&txn, &id, &id, None, &spec, None, TaskScope::Global)
            .await?;
        proposal_ids.push(proposal_id);
    }
    txn.commit().await?;
    tracing::info!(workspace_id = %input.workspace_id, experiment = %input.name, %request_id, candidates = total, "experiment price jobs enqueued");
    Ok(Started {
        request_id,
        proposal_ids,
    })
}
