//! The engine calls. CPU-bound: the executor runs [`run_engines`] on `spawn_blocking`.
//!
//! The design is built from the run's pinned config and pinned control pool
//! (`plan::assignment_units`, then C2a's `to_design`) — never from the panel's own unit
//! list, which would let a store that opened mid-run drift into the control arm. Under
//! `assign_by` the panel is summed by the run's **pinned** `group_membership`
//! (`load_panels` hands it to `load_panel`), never by the dimension's current values. A
//! unit-level waves design reads only its assigned units (the waves plus the pinned pool),
//! so a store opened mid-run neither enters an arm nor trims the panel's common days.
//! A switchback has no control arm: it pins its **fleet** instead, and every read is
//! restricted to it, so a store opened mid-run never enters a period mean.

use chrono::NaiveDate;
use oxy_airlayer_compat::engine::experiment::{
    DroppedUnit, ExperimentDesign, PanelMatrix, estimate, estimate_ratio, placebo_power,
};
use oxy_experiment::config::{ExperimentConfig, MeasureFilter};
use oxy_experiment::engine_in::{EffectIn, PowerIn, RatioIn};
use oxy_experiment::report::{Seeds, SwitchbackFacts};
use oxy_shared::errors::OxyError;

use super::compute::Engines;
use super::mirror::{Judged, name_the_driver};
use super::plan::{
    assignment_units, history_end, never_treated, sibling_design_spec, switchback_facts,
};
use super::series::{DailySeries, daily_series};
use crate::server::experiment::assignment::to_design;
use crate::server::experiment::panel::{
    GroupMembership, HistoryBounds, PanelContext, PanelOutcome, PanelRequest, is_revision_refusal,
};
use crate::server::experiment::reads::time_dimension_for;
use crate::server::experiment::source::PanelSource;

pub enum Loaded {
    Panel(PanelMatrix),
    /// The reason, already worded: "no history for `<measure>` over `<entity>`", or a
    /// refusal PR 3 worded (a measure that does not sum into a group; a `ValidationError`).
    Unavailable(String),
}

pub struct Panels {
    pub target: Loaded,
    pub sibling: Option<Loaded>,
    pub driver: Option<Loaded>,
    /// `outcome.denominator`'s panel: the target becomes a ratio of sums per unit.
    pub denominator: Option<Loaded>,
    /// A reading of past data (a retrospective run): a sparse or missing unit is
    /// dropped and listed instead of refusing, or deleting everyone's days.
    pub lenient: bool,
    /// The target units the panel builder left out, with why (`lenient` only).
    pub dropped: Vec<DroppedUnit>,
}

fn loaded(o: PanelOutcome) -> Loaded {
    match o {
        PanelOutcome::Panel(m) | PanelOutcome::Thinned { panel: m, .. } => Loaded::Panel(m),
        PanelOutcome::NoHistory { measure, entity } => {
            Loaded::Unavailable(format!("no history for `{measure}` over `{entity}`"))
        }
        // C2a: a deterministic refusal is data, not an infrastructure error.
        PanelOutcome::Refused { reason } => Loaded::Unavailable(reason),
    }
}

/// One panel read. `OxyError::ValidationError` is C2a's person-actionable refusal (unknown
/// entity, ambiguous time dimension, `assign_by` naming nothing, membership matching no row):
/// it becomes `Loaded::Unavailable` and is sealed into the report — retrying a message that
/// can never change would only leave the run `closing`. The one exception is a revision
/// refusal (a promotion raced the read, or the workspace was never promoted): that is not
/// a property of the run, so it is `Err` and the job fails, to be retried. Every other `Err`
/// is infrastructure and propagates.
async fn fetch(
    src: &dyn PanelSource,
    ctx: &PanelContext,
    req: &PanelRequest,
) -> Result<Loaded, OxyError> {
    match src.load(ctx, req).await {
        Ok(outcome) => Ok(loaded(outcome)),
        Err(e) if is_revision_refusal(&e) => Err(e),
        Err(OxyError::ValidationError(why)) => Ok(Loaded::Unavailable(why)),
        Err(e) => Err(e),
    }
}

/// [`fetch`], keeping the units a thinned panel left out.
async fn fetch_dropping(
    src: &dyn PanelSource,
    ctx: &PanelContext,
    req: &PanelRequest,
) -> Result<(Loaded, Vec<DroppedUnit>), OxyError> {
    match src.load(ctx, req).await {
        Ok(PanelOutcome::Thinned { panel, dropped }) => Ok((Loaded::Panel(panel), dropped)),
        Ok(outcome) => Ok((loaded(outcome), Vec::new())),
        Err(e) if is_revision_refusal(&e) => Err(e),
        Err(OxyError::ValidationError(why)) => Ok((Loaded::Unavailable(why), Vec::new())),
        Err(e) => Err(e),
    }
}

/// One measure's panel for this run, up to `to_exclusive`.
fn panel_request(
    cfg: &ExperimentConfig,
    measure: &str,
    filters: &[MeasureFilter],
    membership: Option<&GroupMembership>,
    restrict_to: Option<&[String]>,
    to_exclusive: NaiveDate,
    unit_floor: Option<f64>,
) -> PanelRequest {
    PanelRequest {
        entity: cfg.for_each.clone(),
        measure: measure.to_string(),
        time_dimension: time_dimension_for(cfg.design.time_dimension.as_deref(), measure)
            .map(str::to_string),
        grain: cfg.design.grain,
        timezone: Some(cfg.design.timezone.name().to_string()),
        history: HistoryBounds {
            from: None,
            to_exclusive,
        },
        membership: membership.cloned(),
        restrict_to: restrict_to.map(<[String]>::to_vec),
        unit_floor,
        filters: filters.to_vec(),
    }
}

/// The target read on past the registered end, to `carryover_end`: the carryover
/// window's own panel, so the registered estimate reads exactly what it always did.
pub async fn load_carryover(
    src: &dyn PanelSource,
    ctx: &PanelContext,
    cfg: &ExperimentConfig,
    membership: Option<&GroupMembership>,
    restrict_to: Option<&[String]>,
    carryover_end: NaiveDate,
) -> Result<Loaded, OxyError> {
    let req = panel_request(
        cfg,
        &cfg.outcome.target,
        &cfg.outcome.target_filter,
        membership,
        restrict_to,
        carryover_end,
        None,
    );
    fetch(src, ctx, &req).await
}

/// An `Err` is infrastructure (the job fails); an empty or unsummable measure, or a
/// `ValidationError` (see [`fetch`]), is data (`Loaded::Unavailable`, which the report
/// renders as a refusal). All history up to the exclusive end of the last post window —
/// or of the last switchback period — or, for an aborted run, up to `cut`
/// (`plan::measured_until`) when that is earlier: days after the levers went off are never
/// read. A switchback period past the cut is then simply absent from the panel, and the
/// engine's `retained_on_days` counts only what was read. `membership` is the run's pinned map under
/// `assign_by`, `None` otherwise. `restrict_to` is the run's pinned unit set
/// (`plan::read_units`): every read is restricted to it (`PanelRequest.restrict_to`). Each
/// measure is read on its own view's
/// calendar: `design.time_dimension` only where it belongs to that view
/// (`reads::time_dimension_for`, pricing's rule).
pub async fn load_panels(
    src: &dyn PanelSource,
    ctx: &PanelContext,
    cfg: &ExperimentConfig,
    membership: Option<&GroupMembership>,
    restrict_to: Option<&[String]>,
    cut: Option<NaiveDate>,
) -> Result<Panels, OxyError> {
    load(src, ctx, cfg, (membership, restrict_to), cut, false).await
}

/// [`load_panels`], or with `lenient` (a reading of past data) the target read
/// with a unit floor (`design.coverage_floor`), so a sparse unit drops itself
/// (`Panels.dropped`) instead of every unit's days, and the estimate drops a
/// named unit the panel lacks instead of refusing.
pub async fn load_panels_for(
    lenient: bool,
    src: &dyn PanelSource,
    ctx: &PanelContext,
    cfg: &ExperimentConfig,
    membership: Option<&GroupMembership>,
    restrict_to: Option<&[String]>,
    cut: Option<NaiveDate>,
) -> Result<Panels, OxyError> {
    load(src, ctx, cfg, (membership, restrict_to), cut, lenient).await
}

async fn load(
    src: &dyn PanelSource,
    ctx: &PanelContext,
    cfg: &ExperimentConfig,
    (membership, restrict_to): (Option<&GroupMembership>, Option<&[String]>),
    cut: Option<NaiveDate>,
    lenient: bool,
) -> Result<Panels, OxyError> {
    let to_exclusive = history_end(cfg, cut).ok_or_else(|| {
        OxyError::RuntimeError(
            "the design names no wave or switchback period, so it has no post window".to_string(),
        )
    })?;
    // A group panel sums members: thinning it by unit would change what a group is.
    let floor = (lenient && membership.is_none()).then_some(cfg.design.coverage_floor);
    let request = |measure: &str, filters: &[MeasureFilter], floor: Option<f64>| {
        panel_request(
            cfg,
            measure,
            filters,
            membership,
            restrict_to,
            to_exclusive,
            floor,
        )
    };
    let outcome = &cfg.outcome;
    let (target, mut dropped) = fetch_dropping(
        src,
        ctx,
        &request(&outcome.target, &outcome.target_filter, floor),
    )
    .await?;
    // The denominator is read like the target: a sparse unit drops itself there too,
    // and the ratio estimator reads only the units and days both panels hold.
    let denominator = match &outcome.denominator {
        Some(d) => {
            let (panel, more) = fetch_dropping(src, ctx, &request(d, &[], floor)).await?;
            for unit in more {
                if !dropped.iter().any(|known| known.unit == unit.unit) {
                    dropped.push(unit);
                }
            }
            Some(panel)
        }
        None => None,
    };
    let sibling = match &cfg.substitution_check {
        Some(s) => Some(fetch(src, ctx, &request(&s.sibling, &s.filter, None)).await?),
        None => None,
    };
    let driver = match &outcome.driver {
        Some(d) => Some(fetch(src, ctx, &request(d, &outcome.driver_filter, None)).await?),
        None => None,
    };
    Ok(Panels {
        target,
        sibling,
        driver,
        denominator,
        lenient,
        dropped,
    })
}

/// What every engine call shares: one design, built once.
struct Stage<'a> {
    cfg: &'a ExperimentConfig,
    design: ExperimentDesign,
    seeds: Seeds,
    iterations: usize,
    read: super::target_estimate::Read<'a>,
}

struct Target {
    effect: Judged<EffectIn>,
    n_untreated: usize,
    switchback: Option<SwitchbackFacts>,
    series: Option<DailySeries>,
}

pub fn run_engines(
    cfg: &ExperimentConfig,
    pool: &[String],
    panels: &Panels,
    seeds: Seeds,
    iterations: usize,
) -> Engines {
    let design = to_design(cfg, &assignment_units(cfg, pool));
    let stage = Stage {
        cfg,
        design,
        seeds,
        iterations,
        read: super::target_estimate::Read {
            lenient: panels.lenient,
            denominator: panels.denominator.as_ref(),
            dropped: &panels.dropped,
        },
    };
    let target = target_part(&stage, &panels.target);
    let (sibling, sibling_power) = match &panels.sibling {
        Some(s) => {
            let (e, p) = sibling_part(&stage, s);
            (Some(e), Some(p))
        }
        None => (None, None),
    };
    let ratio = panels
        .driver
        .as_ref()
        .map(|d| match cfg.outcome.denominator {
            // The slope is read on the numerator's levels; the headline is a share.
            Some(_) => Judged::refused(RATIO_OUTCOME_NO_DRIVER.to_string()),
            None => ratio_part(&stage, &panels.target, d),
        });
    Engines {
        carryover: None,
        effect: target.effect,
        sibling,
        sibling_power,
        ratio,
        n_untreated: target.n_untreated,
        switchback: target.switchback,
        series: target.series,
    }
}

fn target_part(st: &Stage<'_>, target: &Loaded) -> Target {
    let refused = |why: String, series: Option<DailySeries>| Target {
        effect: Judged::refused(why),
        n_untreated: 0,
        switchback: None,
        series,
    };
    match target {
        Loaded::Panel(m) => {
            // What was read, day by day — kept even when the estimate below refuses.
            let series = Some(daily_series(
                &st.cfg.outcome.target,
                st.cfg.design.grain,
                &st.design,
                m,
            ));
            let effect =
                super::target_estimate::estimate_target(m, &st.design, st.seeds.effect, st.read);
            // A switchback's totals need the fleet (from the panel) and the on-days (the
            // engine's own `retained_on_days`, C1); lacking the count is a section refusal.
            match switchback_facts(st.cfg, &effect.view, m.units().len()) {
                Ok(switchback) => Target {
                    n_untreated: never_treated(&st.design),
                    switchback,
                    effect,
                    series,
                },
                Err(why) => refused(why, series),
            }
        }
        Loaded::Unavailable(why) => refused(why.clone(), None),
    }
}

/// The sibling's own estimate, and its placebo power — bounded before the first
/// switch (see `plan::sibling_design_spec`).
fn sibling_part(st: &Stage<'_>, sibling: &Loaded) -> (Judged<EffectIn>, Judged<PowerIn>) {
    match sibling {
        Loaded::Unavailable(why) => (Judged::refused(why.clone()), Judged::refused(why.clone())),
        Loaded::Panel(m) => {
            let effect = Judged::from_engine(&estimate(m, &st.design, st.seeds.effect));
            let power = match sibling_design_spec(st.cfg, &st.design, st.iterations) {
                Ok(spec) => Judged::from_engine(&placebo_power(m, &spec, st.seeds.placebo)),
                Err(why) => Judged::refused(why),
            };
            (effect, power)
        }
    }
}

/// Why a ratio outcome reads no driver slope, so its prediction reads unavailable.
pub const RATIO_OUTCOME_NO_DRIVER: &str = "a ratio outcome has no first stage: the driver slope is \
     read on the numerator's levels, not on the share the outcome measures";

/// `estimate_ratio` does not know measure names (C2b): its refusals are completed
/// here with `outcome.driver`.
fn ratio_part(st: &Stage<'_>, target: &Loaded, driver: &Loaded) -> Judged<RatioIn> {
    let name = st.cfg.outcome.driver.as_deref().unwrap_or_default();
    let judged = match (target, driver) {
        (Loaded::Panel(t), Loaded::Panel(d)) => {
            Judged::from_engine(&estimate_ratio(t, d, &st.design, st.seeds.ratio))
        }
        (Loaded::Unavailable(why), _) | (_, Loaded::Unavailable(why)) => {
            Judged::refused(why.clone())
        }
    };
    name_the_driver(judged, name)
}
