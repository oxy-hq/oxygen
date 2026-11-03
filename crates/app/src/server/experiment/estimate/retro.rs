//! A retrospective run's pins. No check ever runs on one, so nothing pinned its pool,
//! membership or fleet; the estimate pins them here, once, before it reads them — from
//! the units the warehouse held in the design's own **pre window**, never from the units
//! it holds today, so a store opened after the experiment cannot drift into an arm.
//! Write-once, like the first check's pin (`check::pin`): a later estimate keeps it.

use entity::experiment_runs;
use sea_orm::DatabaseConnection;

use crate::server::experiment::check::executor::panel_context;
use crate::server::experiment::check::pin::{FLEET_LOOKBACK_DAYS, untreated_pool};
use crate::server::experiment::check::reader::{Membership, PanelGroups};
use crate::server::experiment::check::universe::pre_window_units;
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
