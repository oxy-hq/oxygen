//! `experiment_interim` — a reading of a run still in progress.
//!
//! The same path as the sealed estimate (`estimate`): the run's pinned design, pool, pins
//! and seed → panels at the promoted revision → the registered engines → `build_report`.
//! Two differences, both from `oxy_experiment::interim`: the design is trimmed to the days
//! that have finished before today (in the design's timezone), and the report is relabelled
//! `Interim`, without the totals that would extrapolate a partial run or a write-back.
//!
//! What an interim never does: write `experiment_results`, propose a coefficient, move a
//! lever, or change the run's state. It writes one row of `experiment_interims` and nothing
//! else. Enqueued on demand (`POST /experiments/{name}/interim`) and once a day by the tick
//! while the run is live.

pub mod read;
pub mod store;

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxy_experiment::interim::{as_interim, interim_config};
use oxy_experiment::run::RunState;
use oxy_experiment::timeline::local_hour_in;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use self::store::{INTERIM_KIND, Reading};
use crate::server::experiment::estimate::compute::assemble;
use crate::server::experiment::estimate::plan::derive_seeds;
use crate::server::experiment::estimate::store::load_inputs_for;
use crate::server::experiment::estimate::{EstimateSettings, engine_stage};
use crate::server::experiment::source::{PanelSource, WorkspacePanelSource};
use crate::server::experiment::store as runs;

/// What an `experiment_interim` task carries: the run, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterimPayload {
    pub run_id: Uuid,
}

/// Only a run in progress has an interim: before `live` nothing was treated, and from
/// `reported`/`aborted` on the sealed report is the reading.
pub fn in_progress(state: &RunState) -> bool {
    matches!(state, RunState::Live | RunState::Closing)
}

pub struct ExperimentInterimExecutor {
    pub db: DatabaseConnection,
    pub panels: Arc<dyn PanelSource>,
    pub settings: EstimateSettings,
}

impl ExperimentInterimExecutor {
    /// The production wiring: panels come from the workspace's warehouse.
    pub fn warehouse(db: DatabaseConnection) -> Self {
        Self {
            panels: Arc::new(WorkspacePanelSource { db: db.clone() }),
            db,
            settings: EstimateSettings::default(),
        }
    }
}

#[async_trait]
impl TaskExecutor for ExperimentInterimExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for ExperimentInterimExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != INTERIM_KIND {
            return Err(format!("ExperimentInterimExecutor does not run `{kind}`"));
        }
        let payload: InterimPayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad experiment_interim payload: {e}"))?;
        let task = Uuid::parse_str(&assignment.task_id)
            .map_err(|e| format!("experiment_interim task id is not a uuid: {e}"))?;

        let (event_tx, event_rx) = mpsc::channel(8);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let (db, panels, settings) = (self.db.clone(), Arc::clone(&self.panels), self.settings);
        tokio::spawn(async move {
            let _events = event_tx; // no progress events; held open until the outcome
            let outcome =
                run_and_record(&db, panels.as_ref(), settings, payload.run_id, task).await;
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// Compute, then write the row. The answer stays neutral like the estimate's: it never
/// carries a number.
async fn run_and_record(
    db: &DatabaseConnection,
    panels: &dyn PanelSource,
    settings: EstimateSettings,
    run_id: Uuid,
    task: Uuid,
) -> TaskOutcome {
    let now = Utc::now();
    let reading = interim_reading(db, panels, settings, run_id, now).await;
    if let Err(e) = store::finish(db, run_id, task, &reading, now).await {
        return TaskOutcome::Failed(format!("write interim: {e}"));
    }
    match reading {
        Ok(_) => TaskOutcome::Done {
            answer: "interim read".to_string(),
            metadata: None,
        },
        Err(why) => TaskOutcome::Failed(why),
    }
}

/// The registered estimator over the days read so far. `Err` carries a sentence for the
/// page: a run not in progress, nothing finished yet, or a read that failed.
pub async fn interim_reading(
    db: &DatabaseConnection,
    panels: &dyn PanelSource,
    settings: EstimateSettings,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Reading, String> {
    let run = runs::run_by_id(db, run_id)
        .await
        .map_err(|e| format!("load run: {e}"))?
        .ok_or_else(|| format!("experiment run {run_id} does not exist"))?;
    if run.retrospective || !RunState::parse(&run.state).is_some_and(|s| in_progress(&s)) {
        return Err(format!(
            "the run is `{}`: an interim reads only a live or closing run",
            run.state
        ));
    }
    let mut inputs = load_inputs_for(db, &run, false, false).await?;
    let window = interim_config(
        &inputs.config,
        local_hour_in(inputs.config.design.timezone, now),
    )?;
    inputs.config = Arc::new(window.config);
    let seed = inputs
        .config
        .design
        .seed
        .ok_or_else(|| "the registered design carries no seed".to_string())?;
    let seeds = derive_seeds(seed);
    let engines = engine_stage(db, panels, settings, &run, &inputs, seeds).await?;
    let computed = assemble(&inputs, &engines, seeds);
    let report = as_interim(computed.report, window.through);
    Ok(Reading {
        through: window.through,
        report: serde_json::to_value(&report).map_err(|e| e.to_string())?,
        effect: computed.effect_raw,
        compliance: computed.compliance_json,
        series: computed.series_json,
    })
}
