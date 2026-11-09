//! Row access for `experiment_runs` and `experiment_flips`. Every write two
//! replicas can race on is a compare-and-swap, stated here once: the run-row
//! writes in [`super::store_runs`], the check's flip settlements and pins in
//! [`super::store_flips`], both re-exported here.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use entity::{experiment_flips, experiment_runs};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::compliance::FlipOutcome;
use oxy_experiment::run::AbortLevers;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::design_store::{self, Design};
use super::panel::GroupMembership;
pub use super::store_flips::{
    Recorded, pin_control_pool, pin_fleet, settle_flip, settle_flip_recorded,
};
pub use super::store_runs::{
    CHECK_FAILED, Persist, claim, persist, record_check_failure, save_observations,
};

/// What the last `experiment_check` saw. Written by the check, read by the
/// tick (`coverage`) and the status route (everything).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Observations {
    #[serde(default)]
    pub coverage: Option<f64>,
    #[serde(default)]
    pub controls_checked_on: Option<NaiveDate>,
    /// Control unit → first day `when_off` failed. Sticky: a leak is never un-seen.
    #[serde(default)]
    pub control_leaks: BTreeMap<String, NaiveDate>,
    #[serde(default)]
    pub guardrails: Vec<GuardrailStatus>,
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    /// Refusals as values: what the last check could not evaluate, and why.
    #[serde(default)]
    pub problems: Vec<String>,
    /// Under `assign_by`: each treated member's read-back, by member name.
    /// Sticky: a decided member is never re-read.
    #[serde(default)]
    pub member_outcomes: BTreeMap<String, FlipOutcome>,
    /// A switchback: each unit's read-back in each period, keyed
    /// `"<unit>@<period from>"` (C4) — sent periods and cancelled ones alike.
    /// Sticky: a decided period is never re-read.
    #[serde(default)]
    pub period_outcomes: BTreeMap<String, FlipOutcome>,
}

/// One criterion, rolled up over the treated units it watched (`units`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardrailStatus {
    pub criteria: String,
    pub status: GuardrailState,
    pub detail: Option<String>,
    #[serde(default)]
    pub units: Vec<UnitGuardrail>,
}

/// One treated unit (or group) under one criterion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitGuardrail {
    pub unit: String,
    pub status: GuardrailState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailState {
    Ok,
    Breached,
    NotChecked,
    Refused,
}

/// The design the run was registered with — the only one it ever acts on. Its
/// `experiment` is the experiment's name: a design cannot rename its experiment.
pub async fn pinned_config(
    db: &impl ConnectionTrait,
    run: &experiment_runs::Model,
) -> Result<ExperimentConfig, String> {
    Ok(pinned(db, run).await?.config)
}

/// The experiment's name, for a log line or a notice: the run row carries only its id.
pub async fn experiment_name(
    db: &impl ConnectionTrait,
    run: &experiment_runs::Model,
) -> Result<String, DbErr> {
    Ok(design_store::experiment_by_id(db, run.experiment_id)
        .await?
        .map_or_else(|| run.experiment_id.to_string(), |e| e.name))
}

/// [`pinned_config`] with its (frozen) design row.
pub async fn pinned(
    db: &impl ConnectionTrait,
    run: &experiment_runs::Model,
) -> Result<Design, String> {
    let row = design_store::design_by_id(db, run.design_id)
        .await
        .map_err(|e| format!("load the design of run {}: {e}", run.id))?
        .ok_or_else(|| format!("run {} is pinned to a design that is gone", run.id))?;
    design_store::decode(row).map_err(|e| {
        format!(
            "run {} carries a design this binary cannot read: {e}",
            run.id
        )
    })
}

pub fn observations(run: &experiment_runs::Model) -> Observations {
    run.observations
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn control_pool(run: &experiment_runs::Model) -> Option<Vec<String>> {
    run.control_pool
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// Under `assign_by`: every group's members as the first check pinned them
/// (Task 10a) — treated and control groups alike (C3).
pub fn group_membership(run: &experiment_runs::Model) -> Option<GroupMembership> {
    let v = run.group_membership.clone()?;
    match serde_json::from_value(v) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(
                target: "experiment", run_id = %run.id, error = %e,
                "the pinned group membership cannot be read; group levers wait"
            );
            None
        }
    }
}

/// A switchback's fleet, as the first check pinned it (C3 `fleet`).
pub fn fleet(run: &experiment_runs::Model) -> Option<Vec<String>> {
    run.fleet
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
}

pub async fn run_by_name(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
) -> Result<Option<experiment_runs::Model>, DbErr> {
    let Some(experiment) = design_store::experiment_by_name(db, workspace_id, name).await? else {
        return Ok(None);
    };
    run_of(db, experiment.id).await
}

/// The experiment's run — at most one, ever.
pub async fn run_of(
    db: &impl ConnectionTrait,
    experiment_id: Uuid,
) -> Result<Option<experiment_runs::Model>, DbErr> {
    experiment_runs::Entity::find()
        .filter(experiment_runs::Column::ExperimentId.eq(experiment_id))
        .one(db)
        .await
}

/// The runs of `experiment_ids`, by experiment: one query for a list.
pub async fn runs_of(
    db: &impl ConnectionTrait,
    experiment_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, experiment_runs::Model>, DbErr> {
    if experiment_ids.is_empty() {
        return Ok(Default::default());
    }
    Ok(experiment_runs::Entity::find()
        .filter(experiment_runs::Column::ExperimentId.is_in(experiment_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.experiment_id, r))
        .collect())
}

pub async fn run_by_id(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<experiment_runs::Model>, DbErr> {
    experiment_runs::Entity::find_by_id(id).one(db).await
}

pub async fn due_runs(
    db: &impl ConnectionTrait,
    now: DateTime<Utc>,
    limit: u64,
) -> Result<Vec<experiment_runs::Model>, DbErr> {
    experiment_runs::Entity::find()
        .filter(experiment_runs::Column::NextCheckAt.lte(now.fixed_offset()))
        .order_by_asc(experiment_runs::Column::NextCheckAt)
        .limit(limit)
        .all(db)
        .await
}

pub async fn flips_of(
    db: &impl ConnectionTrait,
    run_id: Uuid,
) -> Result<Vec<experiment_flips::Model>, DbErr> {
    experiment_flips::Entity::find()
        .filter(experiment_flips::Column::RunId.eq(run_id))
        .order_by_asc(experiment_flips::Column::CreatedAt)
        .order_by_asc(experiment_flips::Column::Unit)
        .all(db)
        .await
}

/// What the run's abort does (or did) with the levers; `None` until one is requested.
pub fn abort_levers(run: &experiment_runs::Model) -> Option<AbortLevers> {
    run.abort_levers.as_deref().and_then(AbortLevers::parse)
}
