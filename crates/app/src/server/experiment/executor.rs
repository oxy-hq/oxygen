//! `ExperimentPriceExecutor` — the `TaskExecutor` for `kind = "experiment_price"`.
//!
//! Registered exactly like `SimulationTaskExecutor`
//! (`crates/app/src/server/simulation/mod.rs`): decode, hand the work to a task
//! the worker owns, return the channels. The `tokio::spawn` here is the
//! worker's own driver task, not a spawn in an HTTP handler.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::job::{Finished, PriceJob, run_job};
use super::price::Iterations;
use super::proposal_store;
use super::source::{PanelSource, WorkspacePanelSource};
use super::{EXPERIMENT_PRICE_KIND, ExperimentPricePayload};

pub struct ExperimentPriceExecutor {
    pub db: DatabaseConnection,
}

pub(super) fn decode(assignment: &TaskAssignment) -> Result<ExperimentPricePayload, String> {
    let TaskSpec::Custom { kind, payload } = &assignment.spec else {
        return Err(format!(
            "unexpected spec for ExperimentPriceExecutor: {:?}",
            assignment.spec
        ));
    };
    if kind != EXPERIMENT_PRICE_KIND {
        return Err(format!("unknown experiment kind: {kind}"));
    }
    serde_json::from_value(payload.clone())
        .map_err(|e| format!("bad experiment_price payload: {e}"))
}

#[async_trait]
impl TaskExecutor for ExperimentPriceExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let payload = decode(&assignment)?;
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let (db, run_cancel) = (self.db.clone(), cancel.clone());
        tokio::spawn(async move {
            let outcome = drive(db, payload, event_tx, run_cancel).await;
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

async fn drive(
    db: DatabaseConnection,
    payload: ExperimentPricePayload,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    cancel: CancellationToken,
) -> TaskOutcome {
    let id = payload.proposal_id;
    if cancel.is_cancelled() {
        // Settle the row so a request never sits `pending` behind a task that will not run.
        let _ = proposal_store::finish_failed(&db, id, "the price job was cancelled before it ran")
            .await;
        return TaskOutcome::Cancelled;
    }
    let _ = event_tx.try_send((
        "experiment.price.started".to_string(),
        serde_json::json!({ "proposal_id": id }),
    ));
    run_guarded(&db, id, &WorkspacePanelSource { db: db.clone() }).await
}

/// Run the job for `id` against `source`. A panic anywhere in it — outside the
/// blocking thread the source already guards — would unwind the driver task and
/// leave the row `pending` behind a terminal `Failed`, so it is caught here and
/// the row settled `failed` naming the panic.
pub async fn run_guarded(
    db: &DatabaseConnection,
    id: Uuid,
    source: &dyn PanelSource,
) -> TaskOutcome {
    let job = PriceJob {
        db,
        proposal_id: id,
        now: chrono::Utc::now(),
        iterations: Iterations::default(),
    };
    match std::panic::AssertUnwindSafe(run_job(job, source))
        .catch_unwind()
        .await
    {
        Ok(finished) => outcome(finished),
        Err(panic) => {
            let reason = format!("the price job panicked: {}", panic_message(&*panic));
            tracing::error!(proposal_id = %id, %reason, "experiment price job panicked");
            let _ = proposal_store::finish_failed(db, id, &reason).await;
            TaskOutcome::Failed(reason)
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// A refusal is a finished job — `Done`, with the row's status as the answer;
/// only a fault is `Failed`. Either way the proposal row was settled first.
fn outcome(finished: Finished) -> TaskOutcome {
    if let Some(reason) = finished.error {
        return TaskOutcome::Failed(reason);
    }
    TaskOutcome::Done {
        answer: finished.status.as_str().to_string(),
        metadata: Some(serde_json::json!({
            "proposal_id": finished.proposal_id,
            "already_finished": finished.already_finished,
        })),
    }
}

#[cfg(test)]
mod tests {
    use agentic_core::delegation::{TaskAssignment, TaskSpec};
    use uuid::Uuid;

    use super::decode;
    use crate::server::experiment::EXPERIMENT_PRICE_KIND;

    fn assignment(spec: TaskSpec) -> TaskAssignment {
        TaskAssignment {
            task_id: "t".to_string(),
            parent_task_id: None,
            run_id: "r".to_string(),
            spec,
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        }
    }

    fn custom(kind: &str, payload: serde_json::Value) -> TaskSpec {
        TaskSpec::Custom {
            kind: kind.to_string(),
            payload,
        }
    }

    #[test]
    fn a_well_formed_assignment_decodes() {
        let id = Uuid::from_u128(9);
        let payload = decode(&assignment(custom(
            EXPERIMENT_PRICE_KIND,
            serde_json::json!({ "proposal_id": id }),
        )))
        .unwrap();
        assert_eq!(payload.proposal_id, id);
    }

    #[test]
    fn another_kind_is_rejected_by_name() {
        let err = decode(&assignment(custom("preagg_cycle", serde_json::json!({})))).unwrap_err();
        assert!(err.contains("preagg_cycle"), "{err}");
    }

    #[test]
    fn a_non_custom_spec_is_rejected() {
        let spec = TaskSpec::Agent {
            agent_id: "a".into(),
            question: "q".into(),
            extra: None,
        };
        let err = decode(&assignment(spec)).unwrap_err();
        assert!(err.contains("unexpected spec"), "{err}");
    }

    #[test]
    fn a_payload_without_a_proposal_id_is_rejected() {
        let err = decode(&assignment(custom(
            EXPERIMENT_PRICE_KIND,
            serde_json::json!({}),
        )))
        .unwrap_err();
        assert!(err.contains("bad experiment_price payload"), "{err}");
    }
}
