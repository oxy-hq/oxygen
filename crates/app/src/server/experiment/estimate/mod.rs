//! `experiment_estimate` — the job that turns a closed run into its sealed report.
//!
//! One `TaskSpec::Custom { kind: jobs::ESTIMATE_KIND, payload: jobs::EstimatePayload }`
//! per run, queued by the tick (PR 4) rather than spawned by a handler: the estimate
//! runs a permutation test and a placebo, and must survive the instance that enqueued
//! it. Mirrors `SimulationTaskExecutor` (`crates/app/src/server/simulation/mod.rs`).
//!
//! The order is the design:
//! 1. an existing results row → done, nothing recomputed;
//! 2. a run that is not `closing`/`aborted`/`reported` → refused **before** any panel is
//!    read — the effect is sealed while the run is live;
//! 3. the run's pinned design, control pool, (under `assign_by`) group membership and (for
//!    a switchback) fleet → panels at the promoted revision → engines (blocking thread) →
//!    [`compute::assemble`] → one `ON CONFLICT DO NOTHING` insert.

pub mod carryover;
pub mod compute;
pub mod engines;
pub mod mirror;
pub mod plan;
pub mod retro;
pub mod series;
pub mod store;
pub mod target_estimate;

#[cfg(test)]
mod carryover_tests;
#[cfg(test)]
mod compute_tests;
#[cfg(test)]
mod engines_tests;
#[cfg(test)]
mod mirror_tests;
#[cfg(test)]
mod plan_cut_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod series_tests;
#[cfg(test)]
pub(crate) mod testdata;

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use entity::experiment_runs;
use oxy_experiment::report::Seeds;
use oxy_experiment::run::RunState;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use self::compute::{Engines, RunInputs};
use self::plan::derive_seeds;
use crate::server::experiment::check::executor::panel_context;
use crate::server::experiment::jobs::{ESTIMATE_KIND, EstimatePayload};
use crate::server::experiment::source::{PanelSource, WorkspacePanelSource};
use crate::server::experiment::store as runs;

#[derive(Debug, Clone, Copy)]
pub struct EstimateSettings {
    /// Placebo draws for the sibling's floor. Not registered anywhere: it changes
    /// the floor's Monte-Carlo noise, never what is being estimated.
    pub placebo_iterations: usize,
}

impl Default for EstimateSettings {
    fn default() -> Self {
        EstimateSettings {
            placebo_iterations: 400,
        }
    }
}

pub struct ExperimentEstimateExecutor {
    pub db: DatabaseConnection,
    pub panels: Arc<dyn PanelSource>,
    pub settings: EstimateSettings,
}

impl ExperimentEstimateExecutor {
    /// The production wiring: panels come from the workspace's warehouse (PR 3's source).
    pub fn warehouse(db: DatabaseConnection) -> Self {
        Self {
            panels: Arc::new(WorkspacePanelSource { db: db.clone() }),
            db,
            settings: EstimateSettings::default(),
        }
    }
}

/// The job's answers are neutral on purpose: `agentic_runs.answer` is readable
/// before the run is `reported`, and the effect must stay sealed until then.
const ALREADY: &str = "already estimated";
const ESTIMATED: &str = "estimated";

#[async_trait]
impl TaskExecutor for ExperimentEstimateExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for ExperimentEstimateExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != ESTIMATE_KIND {
            return Err(format!("ExperimentEstimateExecutor does not run `{kind}`"));
        }
        let payload: EstimatePayload = serde_json::from_value(payload.clone())
            .map_err(|e| format!("bad experiment_estimate payload: {e}"))?;

        let (event_tx, event_rx) = mpsc::channel(8);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let (db, panels, settings) = (self.db.clone(), Arc::clone(&self.panels), self.settings);
        tokio::spawn(async move {
            let _events = event_tx; // no progress events; held open until the outcome
            let outcome = match estimate_run(&db, panels.as_ref(), settings, payload.run_id).await {
                Ok(answer) => TaskOutcome::Done {
                    answer,
                    metadata: None,
                },
                Err(e) => TaskOutcome::Failed(e),
            };
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

/// Only these states may be estimated. `registered` and `live` are sealed, and so is a
/// state this binary does not recognise.
fn estimable(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Closing | RunState::Aborted | RunState::Reported
    )
}

/// Panels → engines. A missing pin (the control pool, under `assign_by` the group
/// membership, for a switchback the fleet) means no honest assignment: every section
/// refuses, and the job does not ask the warehouse which units exist — or which group
/// they sit in — now. A malformed pin reads as a missing one.
pub(crate) async fn engine_stage(
    db: &DatabaseConnection,
    panels: &dyn PanelSource,
    settings: EstimateSettings,
    run: &experiment_runs::Model,
    inputs: &RunInputs,
    seeds: Seeds,
) -> Result<Engines, String> {
    let membership = runs::group_membership(run);
    let fleet = runs::fleet(run);
    let pool = inputs.control_pool.as_deref();
    if let Some(why) =
        plan::missing_pin(&inputs.config, pool, membership.as_ref(), fleet.as_deref())
    {
        return Ok(compute::refused_engines(&inputs.config, why));
    }
    let pool = pool.map(<[String]>::to_vec).unwrap_or_default();
    // The promoted revision, read as the nil principal (PR 4's check does the same). A
    // workspace with none fails the job: it is retried, never sealed.
    let ctx = panel_context(db, run.workspace_id).await?;
    let restrict_to = plan::read_units(&inputs.config, &pool, fleet.as_deref());
    // A reading of past data drops a sparse or missing unit and says so.
    let loaded = engines::load_panels_for(
        run.retrospective,
        panels,
        &ctx,
        &inputs.config,
        membership.as_ref(),
        restrict_to.as_deref(),
        inputs.measured_until,
    )
    .await
    .map_err(|e| format!("load panels: {e}"))?;
    let carry = match inputs.carryover {
        Some((off, end)) => Some((
            off,
            engines::load_carryover(
                panels,
                &ctx,
                &inputs.config,
                membership.as_ref(),
                restrict_to.as_deref(),
                end,
            )
            .await
            .map_err(|e| format!("load the carryover panel: {e}"))?,
        )),
        None => None,
    };
    let cfg = Arc::clone(&inputs.config);
    tokio::task::spawn_blocking(move || {
        let mut e = engines::run_engines(&cfg, &pool, &loaded, seeds, settings.placebo_iterations);
        e.carryover = carry.map(|(off, panel)| carryover_part(&cfg, &pool, &panel, off, seeds));
        e
    })
    .await
    .map_err(|e| format!("estimate task panicked: {e}"))
}

/// The carryover effect, or the reason its panel could not be read.
fn carryover_part(
    cfg: &oxy_experiment::config::ExperimentConfig,
    pool: &[String],
    panel: &engines::Loaded,
    off: oxy_experiment::moment::Moment,
    seeds: Seeds,
) -> oxy_experiment::engine_in::EffectIn {
    match panel {
        engines::Loaded::Panel(m) => {
            let design = crate::server::experiment::assignment::to_design(
                cfg,
                &plan::assignment_units(cfg, pool),
            );
            carryover::carryover_effect(cfg, &design, m, off, seeds.effect)
        }
        engines::Loaded::Unavailable(why) => mirror::Judged::refused(why.clone()).view,
    }
}

async fn estimate_run(
    db: &DatabaseConnection,
    panels: &dyn PanelSource,
    settings: EstimateSettings,
    run_id: Uuid,
) -> Result<String, String> {
    let run = runs::run_by_id(db, run_id)
        .await
        .map_err(|e| format!("load run: {e}"))?
        .ok_or_else(|| format!("experiment run {run_id} does not exist"))?;
    if store::result_exists(db, run.id)
        .await
        .map_err(|e| format!("check result: {e}"))?
    {
        return Ok(ALREADY.to_string());
    }
    let state = RunState::parse(&run.state)
        .filter(estimable)
        .ok_or_else(|| {
            format!(
                "run {} is `{}`: its estimate is sealed until the run closes or is aborted",
                run.id, run.state
            )
        })?;
    let aborted = matches!(state, RunState::Aborted);
    // A retrospective run was never checked: pin it here, from the design's pre window.
    let run = retro::pin_retrospective(db, panels, run).await?;
    let inputs = store::load_inputs(db, &run, aborted).await?;
    let seed = estimate_seed(&inputs.config, &inputs.design_ref)?;
    let seeds = derive_seeds(seed);
    let engines = engine_stage(db, panels, settings, &run, &inputs, seeds).await?;
    let computed = compute::assemble(&inputs, &engines, seeds);
    let inserted = store::insert_result(db, run.id, &computed, aborted)
        .await
        .map_err(|e| format!("write result: {e}"))?;
    if !inserted {
        return Ok(ALREADY.to_string());
    }
    Ok(ESTIMATED.to_string())
}

/// The seed the estimate's placebo draws run from: the design's own, or for an
/// observed design — which nothing drew, so it has none — one taken from the
/// frozen design's hash, so the same design reads the same way every time.
fn estimate_seed(c: &oxy_experiment::ExperimentConfig, design_ref: &str) -> Result<u64, String> {
    if let Some(seed) = c.design.seed {
        return Ok(seed);
    }
    if c.design.assignment == oxy_experiment::Assignment::Observed {
        // FNV-1a: stable across builds and platforms, unlike `DefaultHasher`.
        let hash = design_ref.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        return Ok(hash);
    }
    Err("the registered design carries no seed".to_string())
}

#[cfg(test)]
mod seed_tests {
    use super::estimate_seed;
    use crate::server::experiment::test_design::design;

    #[test]
    fn a_drawn_design_uses_its_own_seed() {
        let c = design();
        assert_eq!(estimate_seed(&c, "v1"), Ok(c.design.seed.unwrap()));
    }

    #[test]
    fn an_observed_design_takes_a_stable_seed_from_its_hash() {
        let mut c = design();
        c.design.seed = None;
        c.design.assignment = oxy_experiment::Assignment::Observed;
        let a = estimate_seed(&c, "abc").unwrap();
        assert_eq!(estimate_seed(&c, "abc").unwrap(), a);
        assert_ne!(estimate_seed(&c, "abd").unwrap(), a);
    }

    #[test]
    fn a_drawn_design_without_a_seed_is_refused() {
        let mut c = design();
        c.design.seed = None;
        assert!(estimate_seed(&c, "abc").is_err());
    }
}
