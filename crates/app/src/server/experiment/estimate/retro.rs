//! A retrospective run's pins. No check ever runs on one, so nothing pinned its pool,
//! membership or fleet; the estimate pins them here, once, before it reads them — from
//! the units the warehouse held in the design's own **pre window**, never from the units
//! it holds today, so a store opened after the experiment cannot drift into an arm.
//! Write-once, like the first check's pin (`check::pin`): a later estimate keeps it.

use chrono::NaiveDate;
use entity::experiment_runs;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::timeline::first_switch;
use sea_orm::DatabaseConnection;

use crate::server::experiment::check::executor::panel_context;
use crate::server::experiment::check::pin::{FLEET_LOOKBACK_DAYS, untreated_pool};
use crate::server::experiment::check::reader::{Membership, PanelGroups};
use crate::server::experiment::panel::{HistoryBounds, PanelContext, PanelOutcome, PanelRequest};
use crate::server::experiment::reads::time_dimension_for;
use crate::server::experiment::source::PanelSource;
use crate::server::experiment::store as runs;

/// `run` with its pins in place: unchanged unless it is retrospective and unpinned.
pub async fn pin_retrospective(
    db: &DatabaseConnection,
    panels: &dyn PanelSource,
    run: experiment_runs::Model,
) -> Result<experiment_runs::Model, String> {
    if !run.retrospective || pinned(&run) {
        return Ok(run);
    }
    let c = runs::pinned_config(db, &run).await?;
    let ctx = panel_context(db, run.workspace_id).await?;
    let db_err = |e: sea_orm::DbErr| format!("pin the retrospective run: {e}");
    if c.design.switchback.is_some() {
        let lookback = c.design.grain.periods_covering(FLEET_LOOKBACK_DAYS);
        let fleet = pre_window_units(panels, &ctx, &c, lookback).await?;
        runs::pin_fleet(db, run.id, &fleet).await.map_err(db_err)?;
    } else if let Some(dimension) = c.assign_by.as_deref() {
        let groups = PanelGroups {
            db: db.clone(),
            ctx,
        };
        let membership = groups.groups(&c.for_each, dimension).await?;
        let names: Vec<String> = membership.0.keys().cloned().collect();
        let pool = untreated_pool(&c, &names);
        runs::pin_control_pool(db, run.id, &pool, Some(&membership))
            .await
            .map_err(db_err)?;
    } else {
        let pre_days = c.design.pre_days.ok_or("the design has no pre_days")?;
        let units = pre_window_units(panels, &ctx, &c, pre_days).await?;
        let pool = untreated_pool(&c, &units);
        runs::pin_control_pool(db, run.id, &pool, None)
            .await
            .map_err(db_err)?;
    }
    runs::run_by_id(db, run.id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| format!("experiment run {} vanished while pinning", run.id))
}

/// What the design's shape needs pinned, already pinned.
fn pinned(run: &experiment_runs::Model) -> bool {
    runs::fleet(run).is_some() || runs::control_pool(run).is_some()
}

/// `[first switch − anticipation − days, first switch − anticipation)`, in periods of
/// `design.grain`: the pre window a live run's first check would have pinned from.
pub fn pre_window(c: &ExperimentConfig, days: u32) -> Option<(NaiveDate, NaiveDate)> {
    let grain = c.design.grain;
    let first = first_switch(c)?.date();
    let to = grain.sub_covering(first, c.design.anticipation_days)?;
    Some((grain.sub_covering(to, days)?, to))
}

/// Every unit with `outcome.target` rows in the pre window. An empty answer fails the
/// job (retried, then the owner is told): an empty pin would stand for good.
async fn pre_window_units(
    panels: &dyn PanelSource,
    ctx: &PanelContext,
    c: &ExperimentConfig,
    days: u32,
) -> Result<Vec<String>, String> {
    let (from, to) = pre_window(c, days).ok_or("the design has no first switch day")?;
    let target = &c.outcome.target;
    let request = PanelRequest {
        entity: c.for_each.clone(),
        measure: target.clone(),
        time_dimension: time_dimension_for(c.design.time_dimension.as_deref(), target)
            .map(str::to_string),
        grain: c.design.grain,
        timezone: Some(c.design.timezone.name().to_string()),
        history: HistoryBounds {
            from: Some(from),
            to_exclusive: to,
        },
        membership: None,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    };
    let units = match panels
        .load(ctx, &request)
        .await
        .map_err(|e| format!("read the units of `{target}`: {e}"))?
    {
        PanelOutcome::Panel(m) | PanelOutcome::Thinned { panel: m, .. } => m.units().to_vec(),
        PanelOutcome::NoHistory { .. } => Vec::new(),
        PanelOutcome::Refused { reason } => return Err(reason),
    };
    if units.is_empty() {
        return Err(format!(
            "the warehouse holds no `{target}` rows from {from} to before {to}, the design's \
             pre window, so a retrospective run has no units to compare"
        ));
    }
    Ok(units)
}
