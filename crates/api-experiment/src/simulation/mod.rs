//! Running a declared world against the shipped semantic layer.
//!
//! One `TaskSpec::Custom { kind: "simulation_run" }` per run. Queued rather
//! than spawned because a 40-period run is minutes of warehouse queries: a
//! `tokio::spawn` in a handler dies with the instance and can neither be
//! resumed nor cancelled.
//!
//! The layering, which is the point:
//!
//! * [`world_dir`] materialises a throwaway workspace — `config.yml`, a
//!   `.view.yml`, a dataset directory — so the rows are read by the real
//!   loader through the path a customer's data takes.
//! * [`probe`] runs the shipped fitter over it. Nothing in there is
//!   simulation-shaped.
//! * The loop itself lives in `oxy-simulation`, which knows nothing about
//!   either. That is what lets its properties be tested without a warehouse.
//!
//! Truth crosses exactly once, in [`store::finish_run`].

pub mod probe;
pub mod store;
pub mod world_dir;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use oxy_simulation::{
    PolicyKind, RunSummary, Runner, SimulationError, SimulationSpec, World, policy,
};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use self::probe::FitProbe;
use self::world_dir::{CsvSink, WorldDir};

/// `TaskSpec::Custom` discriminator. Becomes the run's `source_type`.
pub const SIMULATION_RUN_KIND: &str = "simulation_run";

/// What the queue carries. The spec travels **by value**, not as a pointer at
/// `simulation_definitions`: a run is evidence, and one that re-read its world
/// at execution time would silently run a different world from the one that was
/// enqueued if someone retuned the file in between.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationRunPayload {
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<Uuid>,
    /// The compiled `.simulation.yml` body — with this replicate's seed already
    /// substituted, so the snapshot the run stores is the world it actually ran
    /// rather than the file's declared seed plus arithmetic a reader has to
    /// redo.
    pub spec: serde_json::Value,
    /// Which arm of the race this run is.
    ///
    /// On the payload rather than in the spec because a policy is not a property
    /// of a world: `hold` and `machine` have to be able to run the *same* world,
    /// same seed, same shocks, or the profit race compares two worlds that
    /// happen to look alike.
    #[serde(default)]
    pub policy: PolicyKind,
    /// Which draw of the world this is. `0` is the declared seed.
    #[serde(default)]
    pub replicate: u32,
}

pub struct SimulationTaskExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for SimulationTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for SimulationTaskExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != SIMULATION_RUN_KIND {
            return Err(format!("unknown simulation kind: {kind}"));
        }
        let payload: SimulationRunPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad simulation payload: {e}"))?;

        let (event_tx, event_rx) = mpsc::channel(256);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let db = self.db.clone();

        // The clone is the whole point of the token: the worker cancels the one
        // it got back on `ExecutingTask`, and this is the end that watches it.
        let run_cancel = cancel.clone();
        tokio::spawn(async move {
            let outcome = execute_run(db, payload, event_tx, run_cancel).await;
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

/// Read the queued snapshot back into a spec.
///
/// Deserialising here rather than at compile time is deliberate: a spec that
/// parses but declares an incoherent world (an unreachable optimum, too little
/// history to fit on) is a run failure with a diagnosable message, not a
/// compile failure that takes the whole revision down with it.
fn parse_spec(payload: &SimulationRunPayload) -> Result<SimulationSpec, String> {
    serde_json::from_value(payload.spec.clone())
        .map_err(|e| e.to_string())
        .and_then(|s: SimulationSpec| {
            // `from_yaml` validates; going through the value path skips that,
            // so re-run it here or an incoherent world reaches the loop.
            serde_yaml::to_string(&s)
                .map_err(|e| e.to_string())
                .and_then(|y| SimulationSpec::from_yaml(&y).map_err(|e| e.to_string()))
        })
}

async fn execute_run(
    db: DatabaseConnection,
    payload: SimulationRunPayload,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    cancel: CancellationToken,
) -> TaskOutcome {
    let spec = match parse_spec(&payload) {
        Ok(spec) => spec,
        Err(e) => {
            // The row already exists — `queue_run` wrote it synchronously
            // when this run was enqueued — so a validation failure here has
            // to close it out explicitly, or it sits at `queued` forever
            // and looks like a run still waiting for a worker rather than
            // one that already died.
            let message = format!("invalid simulation spec: {e}");
            let _ = store::fail_run(&db, payload.run_id, message.clone()).await;
            return TaskOutcome::Failed(message);
        }
    };

    match on_claim(store::mark_running(&db, payload.run_id).await) {
        ClaimStep::Run => {}
        ClaimStep::Skip => {
            // A requeued attempt of a run some earlier attempt already closed
            // out. Nothing to do and nothing to write: the first terminal
            // status wins, which is the same rule the queue applies to its own
            // rows. `Done` rather than `Failed` because the work really is
            // finished — and it stops the queue requeueing this payload again.
            tracing::info!(
                run_id = %payload.run_id,
                "simulation run is already closed; not re-running it"
            );
            return TaskOutcome::Done {
                answer: "run was already closed by an earlier attempt".to_string(),
                metadata: None,
            };
        }
        ClaimStep::Close(message) => {
            // Same reasoning as the bad-spec arm above, and it was missing
            // here: `queue_run` wrote the row at enqueue, so an attempt that
            // gives up without closing it leaves it at `queued` with nothing
            // alive to move it — `Failed` stamps the *queue* row terminal, so
            // there is no retry coming. Best-effort, like every other terminal
            // write in this function; `IN_FLIGHT_MAX_AGE_HOURS` is still the
            // backstop, it is just no longer the only thing between a failed
            // claim and a six-hour cap slot.
            let _ = store::fail_run(&db, payload.run_id, message.clone()).await;
            return TaskOutcome::Failed(message);
        }
    }

    let run_id = payload.run_id;
    match run_blocking(db.clone(), run_id, spec, payload.policy, event_tx, cancel).await {
        Ok(RunEnd::Completed(summary)) => {
            if let Err(e) = store::finish_run(&db, run_id, summary.truth).await {
                return TaskOutcome::Failed(format!("finish run: {e}"));
            }
            TaskOutcome::Done {
                answer: format!(
                    "ran {} periods, cumulative profit {:.2}",
                    summary.periods, summary.cumulative_profit
                ),
                metadata: None,
            }
        }
        Ok(RunEnd::Cancelled) => {
            // Best-effort, same as the failure arm below.
            let _ = store::cancel_run(&db, run_id).await;
            TaskOutcome::Cancelled
        }
        Err(e) => {
            // Best-effort: if this write also fails the run stays `running`
            // forever, which is worse than a wrong message but not something
            // the executor can fix from here.
            let _ = store::fail_run(&db, run_id, e.clone()).await;
            TaskOutcome::Failed(e)
        }
    }
}

/// What the executor does about the claim write's answer.
///
/// Three ways rather than a `Result`, because the two unhappy ones call for
/// opposite writes. A claim that *errored* leaves a row at `queued` that
/// nothing will ever move, so the run has to be closed out. A claim that moved
/// *nothing* means an earlier attempt already closed it, and writing anything
/// would overwrite that run's evidence.
#[derive(Debug)]
enum ClaimStep {
    /// This attempt owns the run; execute it.
    Run,
    /// The run is already terminal. Stop, and touch nothing.
    Skip,
    /// The claim write itself failed. Close the run out under this message.
    Close(String),
}

/// Read a claim write's answer.
///
/// Split from [`execute_run`] so the mapping is assertable without a database:
/// forcing a `DbErr` out of a live pool needs a broken connection, and what was
/// wrong here was never the SQL — it was which of the three the executor picked.
fn on_claim(result: Result<store::Claim, sea_orm::DbErr>) -> ClaimStep {
    match result {
        Ok(store::Claim::Claimed) => ClaimStep::Run,
        Ok(store::Claim::AlreadyClosed) => ClaimStep::Skip,
        Err(e) => ClaimStep::Close(format!("mark run running: {e}")),
    }
}

/// Which way the loop ended.
///
/// A cancelled run is not a failed one — it keeps its periods and closes out as
/// `cancelled` — so the two have to be told apart before a `TaskOutcome` is
/// picked. An enum rather than reading the error: the loop stops *by* returning
/// an error from the period callback, and matching on its message would also
/// catch a genuine engine failure worded the same way.
enum RunEnd {
    Completed(RunSummary),
    Cancelled,
}

/// Stop the loop when the task has been cancelled.
///
/// Checked at the top of the per-period callback, which is the granularity that
/// matters: `Runner::run` only inspects the callback's result *between*
/// periods, so every period already written stays written and the cancelled one
/// is never persisted.
///
/// `SimulationError` has no cancellation variant and this crate does not own
/// it, so the variant returned here is only the vehicle that unwinds the loop —
/// `flag` is what [`run_end`] reads, and the outcome never depends on the
/// message.
fn observe_cancel(cancel: &CancellationToken, flag: &AtomicBool) -> Result<(), SimulationError> {
    if !cancel.is_cancelled() {
        return Ok(());
    }
    flag.store(true, Ordering::Relaxed);
    Err(SimulationError::Drift(
        "run cancelled between periods".to_string(),
    ))
}

/// Read the loop's result as an ending.
///
/// The flag is the authority, not the error, because only [`observe_cancel`]
/// sets it: a genuine drift raised by the engine while a cancel happens to be
/// in flight still closes the run out as failed rather than being quietly
/// relabelled as somebody's decision.
fn run_end(result: Result<RunSummary, SimulationError>, cancelled: bool) -> Result<RunEnd, String> {
    match result {
        Ok(summary) => Ok(RunEnd::Completed(summary)),
        Err(_) if cancelled => Ok(RunEnd::Cancelled),
        Err(e) => Err(e.to_string()),
    }
}

/// The loop, on a blocking thread.
///
/// DuckDB's connection and the fitter are both synchronous and CPU-bound, and a
/// 40-period run holds them for minutes. Running that on a runtime worker
/// starves every other task on the thread.
async fn run_blocking(
    db: DatabaseConnection,
    run_id: Uuid,
    spec: SimulationSpec,
    arm: PolicyKind,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    cancel: CancellationToken,
) -> Result<RunEnd, String> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        // Nothing below observes the token until the first period lands, and
        // materialising the world plus its first probe is already real work —
        // so a cancel that arrives between the claim and period 1 would
        // otherwise buy a whole run.
        if cancel.is_cancelled() {
            return Ok(RunEnd::Cancelled);
        }

        let world_dir = WorldDir::create(&spec).map_err(|e| e.to_string())?;
        let curve = spec.curve().map_err(|e| e.to_string())?;
        let mut probe = FitProbe::new(&world_dir, &spec).map_err(|e| e.to_string())?;
        let mut sink = CsvSink::new(&world_dir);
        let mut policy = policy::build(arm, &spec, curve);
        let mut world = World::new(spec).map_err(|e| e.to_string())?;

        let db = Arc::new(db);
        let cancelled = AtomicBool::new(false);
        let result = Runner::new(&mut world, &mut *policy, &mut sink, &mut probe).run(|result| {
            // Before the write, so the period a cancel lands on is never
            // persisted and `periods_done` stays the count of periods that
            // actually completed.
            observe_cancel(&cancel, &cancelled)?;
            let db = Arc::clone(&db);
            // Persist before the event, so a listener that reacts to the
            // event can always read the period it names.
            handle
                .block_on(store::record_period(&db, run_id, result))
                .map_err(|e| SimulationError::Write(format!("record period: {e}")))?;
            let _ = event_tx.try_send((
                "simulation.period".to_string(),
                serde_json::json!({
                    "period": result.period,
                    "mean_spend": result.mean_spend,
                    "cumulative_profit": result.cumulative_profit,
                    "outcomes": result
                        .fits
                        .iter()
                        .map(|f| f.outcome.as_str())
                        .collect::<Vec<_>>(),
                }),
            ));
            Ok(())
        });

        run_end(result, cancelled.load(Ordering::Relaxed))
    })
    .await
    .map_err(|e| format!("simulation task panicked: {e}"))?
}

#[cfg(test)]
mod tests;
