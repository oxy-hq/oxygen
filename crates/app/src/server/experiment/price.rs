//! Pricing a proposed design — everything after the panel exists.
//!
//! Pure: no database, no warehouse, and no clock but the `now` it is handed.
//! This file holds the shared types, the guards every core shares and the
//! dispatcher; the waves core is [`super::price_waves`] (re-exported here) and
//! the switchback core [`super::price_switchback`].
//! Every message states only what its own guard tested.

use chrono::{DateTime, Utc};
use oxy_airlayer_compat::engine::experiment::{
    DesignSpec, PanelMatrix, PowerResult, placebo_power,
};
use oxy_experiment::Moment;
use oxy_experiment::timeline::{deadline_before, moment_utc, today_in};
use oxy_experiment::{ExperimentConfig, Grain, validate_against_units, validate_static};

use super::candidate::Candidate;
use super::panel::GroupMembership;
use super::price_switchback::price_switchback;
pub use super::price_waves::price_waves;
use super::snippet::{PeriodPlan, WavePlan};

/// Placebo draws for a common-date design. Host tuning, recorded in
/// `power.iterations` — not a registered parameter.
pub const ITERATIONS_COMMON_DATE: usize = 2000;
/// Placebo draws for a staggered design, fewer because each draw runs a
/// permutation of its own (`internal-docs/experiments.md:608-611`).
pub const ITERATIONS_STAGGERED: usize = 300;
/// Placebo draws for a switchback: a sign-flip over the pairs, cheap per draw.
pub const ITERATIONS_SWITCHBACK: usize = 1000;

#[derive(Debug, Clone, Copy)]
pub struct Iterations {
    pub common_date: usize,
    pub staggered: usize,
    pub switchback: usize,
}

impl Default for Iterations {
    fn default() -> Self {
        Self {
            common_date: ITERATIONS_COMMON_DATE,
            staggered: ITERATIONS_STAGGERED,
            switchback: ITERATIONS_SWITCHBACK,
        }
    }
}

/// What the job supplies besides the candidate: the seed it drew, the clock,
/// the tuning, and — under `assign_by` — the membership the panel was summed by.
pub struct RunParams {
    pub seed: u64,
    pub now: DateTime<Utc>,
    pub iterations: Iterations,
    pub membership: Option<GroupMembership>,
}

/// The assignment, dated.
#[derive(Debug, Clone)]
pub enum SchedulePlan {
    Waves(Vec<WavePlan>),
    Switchback {
        period_days: u32,
        periods: Vec<PeriodPlan>,
    },
}

#[derive(Debug, Clone)]
pub struct Priced {
    pub seed: u64,
    /// The α this candidate was priced at.
    pub alpha: f64,
    pub assign_by: Option<String>,
    pub plan: SchedulePlan,
    pub strata: Option<Vec<Vec<String>>>,
    pub power: PowerResult,
    /// `power.mde` in $ per unit-day (index C2b).
    pub mde_unit_day: f64,
    pub warnings: Vec<String>,
}

pub enum PriceComputation {
    Priced(Priced),
    /// A design that was not priced, and why. `problems` lists one line per
    /// validation error when the file itself was the reason.
    Refused {
        reason: String,
        problems: Vec<String>,
    },
}

impl PriceComputation {
    pub(super) fn refused(reason: impl Into<String>) -> Self {
        Self::Refused {
            reason: reason.into(),
            problems: Vec::new(),
        }
    }
}

pub(super) fn refused_with_problems(problems: Vec<String>) -> PriceComputation {
    PriceComputation::Refused {
        reason: format!(
            "the experiment file has {} problem(s) against this workspace's data",
            problems.len()
        ),
        problems,
    }
}

/// `validate_static` plus `validate_against_units`, one line per distinct message.
pub(super) fn config_problems(cfg: &ExperimentConfig, units: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in validate_static(cfg)
        .into_iter()
        .chain(validate_against_units(cfg, units))
    {
        let message = e.to_string();
        if !out.contains(&message) {
            out.push(message);
        }
    }
    out
}

/// The placebo's answer, or why there is none (a refusal, or a non-finite MDE).
pub(super) fn priced_power(
    panel: &PanelMatrix,
    spec: &DesignSpec,
    seed: u64,
) -> Result<PowerResult, String> {
    let power = placebo_power(panel, spec, seed);
    if let Some(reason) = &power.refusal {
        return Err(format!("the design could not be priced: {reason}"));
    }
    if !power.mde.is_finite() {
        return Err(
            "the placebo returned a minimum detectable effect that is not a finite number"
                .to_string(),
        );
    }
    Ok(power)
}

/// PR 2's `registration_deadline` convention applied to a candidate's first
/// date: the start of the local day `first_day − anticipation_days` periods.
pub(super) fn commit_before(cfg: &ExperimentConfig, first: Moment) -> DateTime<Utc> {
    deadline_before(cfg, first).unwrap_or_else(|| moment_utc(first, cfg.design.timezone))
}

/// A candidate's first date must open a period of `design.grain`, as every date
/// in the file must (`ConfigError::NotAPeriodStart`).
pub(super) fn first_date_refusal(cfg: &ExperimentConfig, first: Moment) -> Option<String> {
    let grain = cfg.design.grain;
    if grain.starts(first) {
        return None;
    }
    Some(match grain {
        Grain::Hour => format!(
            "the first switch {first} is not a whole hour: at hour grain it must be one, such as {}",
            Moment(
                first
                    .at()
                    .date()
                    .and_hms_opt(chrono::Timelike::hour(&first.at()), 0, 0)
                    .unwrap_or(first.at())
            )
        ),
        Grain::Day => format!(
            "the first switch {first} has a time of day: at day grain a switch is a date (its midnight), such as {}",
            first.ceil_date()
        ),
        _ => format!(
            "the first date {first} does not start a {g}: at {g} grain it must be {}, such as {}",
            if grain == Grain::Week {
                "a Monday"
            } else {
                "the 1st of a month"
            },
            grain
                .add(grain.floor(first.date()), 1)
                .unwrap_or(first.date()),
            g = grain.as_str(),
        ),
    })
}

/// The registration deadline warning every candidate shares: set when `now` is
/// already at or past it.
pub(super) fn deadline_warning(
    cfg: &ExperimentConfig,
    first_day: Moment,
    now: DateTime<Utc>,
) -> Option<String> {
    let deadline = commit_before(cfg, first_day);
    (now >= deadline).then(|| {
        format!("the registration deadline {deadline} has already passed: a file committed now would be dated after it")
    })
}

/// The exclusive end of the pricing history: the period today falls in, in
/// `design.timezone` — it has not finished, and the panel stops before it.
pub(super) fn history_to(eff: &ExperimentConfig, run: &RunParams) -> i64 {
    eff.design
        .grain
        .ordinal(today_in(eff.design.timezone, run.now))
}

/// The one entry point the job calls: price a candidate with its own core.
pub fn price_design(
    cfg: &ExperimentConfig,
    panel: &PanelMatrix,
    candidate: &Candidate,
    run: &RunParams,
) -> PriceComputation {
    // Placebo power prices one measure; a ratio of two per unit needs its own.
    if cfg.outcome.denominator.is_some() {
        return PriceComputation::refused(
            "outcome.denominator makes the outcome a ratio, which pricing cannot size yet; \
             analyse it from past data instead",
        );
    }
    match candidate {
        Candidate::Waves(c) => price_waves(cfg, panel, c, run),
        Candidate::Switchback(c) => price_switchback(cfg, panel, c, run),
    }
}
