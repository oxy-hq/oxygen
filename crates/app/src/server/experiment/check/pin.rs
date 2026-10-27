//! Pinning, once, at the first check: the control pool here; under
//! `assign_by` the group membership with it (Task 10a); a switchback's fleet
//! (Task 10b). The tick cannot read the warehouse, so registration cannot pin;
//! the first check can, and `WHERE control_pool IS NULL` makes it the only one.

use std::collections::HashSet;

use entity::experiment_runs;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::validate::validate_against_units;
use sea_orm::{DatabaseConnection, DbErr};

use super::Readers;
use super::reader::{Membership, UnitDays};
use crate::server::experiment::panel::GroupMembership;
use crate::server::experiment::store::Observations;
use crate::server::experiment::{notify, store};

/// The pinned control pool: this check's pin if it is the first, else the one
/// already pinned. `None` (with a problem) when the units cannot be read.
pub async fn control_pool(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    c: &ExperimentConfig,
    days: &dyn UnitDays,
    problems: &mut Vec<String>,
) -> Result<Option<Vec<String>>, DbErr> {
    if let Some(pool) = store::control_pool(run) {
        return Ok(Some(pool));
    }
    let Some(pre_days) = c.design.pre_days else {
        problems.push("pinning the control pool: the pinned design has no pre_days".into());
        return Ok(None);
    };
    // `units` looks back in calendar days; the pre window is `pre_days` periods.
    let lookback = c.design.grain.days_spanned(pre_days);
    let units = match days.units(&c.outcome.target, lookback).await {
        Ok(units) => units,
        Err(e) => {
            problems.push(format!("pinning the control pool: {e}"));
            return Ok(None);
        }
    };
    if units.is_empty() {
        // A warehouse that has not loaded is not a fleet of nobody: pinning
        // an empty pool would be permanent. Retried on the next check.
        problems.push("pinning the control pool: the warehouse returned no units yet".into());
        return Ok(None);
    }
    let pool = untreated_pool(c, &units);
    if !store::pin_control_pool(db, run.id, &pool, None).await? {
        // Another check pinned it first; that one stands, and it reported.
        return Ok(store::run_by_id(db, run.id)
            .await?
            .as_ref()
            .and_then(store::control_pool));
    }
    report_at_pinning(db, run, c, &units, problems).await;
    Ok(Some(pool))
}

/// Units with history, minus the treated, and — when the design is
/// stratified — only units a stratum names, so every pinned unit sits in
/// exactly one (C1 `Assignment.strata` refuses one that does not). Sorted.
pub fn untreated_pool(c: &ExperimentConfig, units: &[String]) -> Vec<String> {
    let treated: HashSet<&str> = c
        .design
        .waves
        .iter()
        .flat_map(|w| w.units.iter().map(String::as_str))
        .collect();
    let stratified: Option<HashSet<&str>> = c
        .design
        .strata
        .as_ref()
        .map(|s| s.iter().flatten().map(String::as_str).collect());
    let mut pool: Vec<String> = units
        .iter()
        .filter(|u| !treated.contains(u.as_str()))
        .filter(|u| stratified.as_ref().map_or(true, |s| s.contains(u.as_str())))
        .cloned()
        .collect();
    pool.sort();
    pool.dedup();
    pool
}

/// PR 2's check over exactly the units seen at pinning — never re-run with
/// later units. The run is registered, so a finding is a problem on this
/// check and an owner notice (sent once), not a refusal.
pub(super) async fn report_at_pinning(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    c: &ExperimentConfig,
    units: &[String],
    problems: &mut Vec<String>,
) {
    for finding in validate_against_units(c, units) {
        let reason = format!("at pinning: {finding}");
        notify::notify_owner(db, run, &c.owner, &reason).await;
        problems.push(reason);
    }
}

/// What the first check pinned, as this check must use it.
#[derive(Debug, Default)]
pub struct Pinned {
    pub pool: Option<Vec<String>>,
    pub membership: Option<GroupMembership>,
    /// A switchback's fleet (no control pool: it has no control arm).
    pub fleet: Option<Vec<String>>,
}

/// Pin by the design's shape — units, groups (here), a switchback's fleet
/// (Task 10b) — or read back what an earlier check pinned.
pub async fn pin_all(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    c: &ExperimentConfig,
    readers: &Readers<'_>,
    obs: &mut Observations,
) -> Result<Pinned, DbErr> {
    if c.design.switchback.is_some() {
        return Ok(Pinned {
            fleet: fleet(db, run, c, readers.days, &mut obs.problems).await?,
            ..Pinned::default()
        });
    }
    match c.assign_by.as_deref() {
        Some(dimension) => groups(db, run, c, dimension, readers.groups, &mut obs.problems).await,
        None => Ok(Pinned {
            pool: control_pool(db, run, c, readers.days, &mut obs.problems).await?,
            ..Pinned::default()
        }),
    }
}

/// Under `assign_by`: every group's members and the control groups, pinned in
/// ONE statement. Later answers from the warehouse change neither — a
/// reclassified member stays where it was randomised (Review Focus 8).
async fn groups(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    c: &ExperimentConfig,
    dimension: &str,
    reader: &dyn Membership,
    problems: &mut Vec<String>,
) -> Result<Pinned, DbErr> {
    if let (Some(pool), Some(m)) = (store::control_pool(run), store::group_membership(run)) {
        return Ok(Pinned {
            pool: Some(pool),
            membership: Some(m),
            ..Pinned::default()
        });
    }
    let membership = match reader.groups(&c.for_each, dimension).await {
        Ok(m) => m,
        Err(e) => {
            problems.push(format!("pinning the groups: {e}"));
            return Ok(Pinned::default());
        }
    };
    let names: Vec<String> = membership.0.keys().cloned().collect();
    if names.is_empty() {
        // As for units: a warehouse that has not loaded is not a design of
        // no groups, and pinning none would be permanent.
        problems.push("pinning the groups: the warehouse returned no groups yet".into());
        return Ok(Pinned::default());
    }
    let pool = untreated_pool(c, &names);
    if !store::pin_control_pool(db, run.id, &pool, Some(&membership)).await? {
        let run = store::run_by_id(db, run.id).await?;
        return Ok(Pinned {
            pool: run.as_ref().and_then(store::control_pool),
            membership: run.as_ref().and_then(store::group_membership),
            ..Pinned::default()
        });
    }
    report_at_pinning(db, run, c, &names, problems).await;
    Ok(Pinned {
        pool: Some(pool),
        membership: Some(membership),
        ..Pinned::default()
    })
}

/// A switchback has no `pre_days`; four weekly cycles decide who is in the fleet.
pub const FLEET_LOOKBACK_DAYS: u32 = 28;

/// A switchback's fleet: this check's pin if it is the first, else the one
/// already pinned. An empty read is a `problem`, never a pin (the next check
/// retries). No `validate_against_units`: a switchback names no units.
async fn fleet(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    c: &ExperimentConfig,
    days: &dyn UnitDays,
    problems: &mut Vec<String>,
) -> Result<Option<Vec<String>>, DbErr> {
    if let Some(fleet) = store::fleet(run) {
        return Ok(Some(fleet));
    }
    let mut fleet = match days.units(&c.outcome.target, FLEET_LOOKBACK_DAYS).await {
        Ok(units) => units,
        Err(e) => {
            problems.push(format!("pinning the switchback's fleet: {e}"));
            return Ok(None);
        }
    };
    fleet.sort();
    fleet.dedup();
    if fleet.is_empty() {
        // Never pin an empty fleet: the pin is write-once and would stand for
        // the whole run. The run stays unpinned, so the next check retries.
        problems.push(format!(
            "no rows for {} in the last {FLEET_LOOKBACK_DAYS} days; fleet not pinned yet",
            c.outcome.target
        ));
        return Ok(None);
    }
    if store::pin_fleet(db, run.id, &fleet).await? {
        return Ok(Some(fleet));
    }
    // Another check pinned it first; that one stands.
    Ok(store::run_by_id(db, run.id)
        .await?
        .as_ref()
        .and_then(store::fleet))
}
