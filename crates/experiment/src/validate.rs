//! What a config is refused for.
//!
//! One [`ConfigError`] variant per refusal, and each message states only what
//! its own guard tested — not the reason behind the rule and not a
//! neighbouring guard's conclusion ("unit X appears in two waves", never "the
//! design is unidentified"). A refusal is a value; nothing here panics.

use crate::config::{ExperimentConfig, GuardrailCriteria};
use crate::expr::{ExprError, parse_comparison, parse_trigger};
use crate::grain::Grain;
use crate::moment::Moment;
use crate::suggest::MAX_EDITS;
use std::collections::HashMap;

mod design;
mod filters;
mod registrable;
mod units;

pub use registrable::{is_registrable, is_retrospective_ready};
pub use units::validate_against_units;

/// One variant per refusal; each message states only what its own guard tested.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// The YAML did not deserialize into an [`ExperimentConfig`]: malformed, a
    /// required key missing, a value of the wrong type, or a key the schema
    /// does not have.
    #[error("the file is not a valid experiment: {message}")]
    Parse { message: String },

    #[error(
        "unit {unit:?} appears in wave {first_wave} (switch_on {first_switch_on}) and in wave {second_wave} (switch_on {second_switch_on})"
    )]
    UnitInTwoWaves {
        unit: String,
        first_wave: usize,
        first_switch_on: Moment,
        second_wave: usize,
        second_switch_on: Moment,
    },

    #[error("wave {wave} (switch_on {switch_on}) lists no units")]
    EmptyWave { wave: usize, switch_on: Moment },

    #[error("waves {first_wave} and {second_wave} both have switch_on {switch_on}")]
    DuplicateSwitchDate {
        switch_on: Moment,
        first_wave: usize,
        second_wave: usize,
    },

    #[error("outcome.alpha is {value}, which is not in (0, 1)")]
    AlphaOutOfRange { value: f64 },

    #[error("outcome.power is {value}, which is not in (0, 1)")]
    PowerOutOfRange { value: f64 },

    #[error("design.coverage_floor is {value}, which is not in (0, 1]")]
    CoverageFloorOutOfRange { value: f64 },

    #[error("design.compliance_floor is {value}, which is not in (0, 1]")]
    ComplianceFloorOutOfRange { value: f64 },

    #[error("outcome.family is {value}, which is less than 1")]
    FamilyBelowOne { value: u32 },

    #[error("design.pre_days is 0")]
    PreDaysZero,

    #[error("design.post_days is 0")]
    PostDaysZero,

    #[error("guardrails[{guardrail}] anomaly significance is {value}, which is not in (0, 1)")]
    GuardrailSignificanceOutOfRange { guardrail: usize, value: f64 },

    #[error("outcome.mde_at_proposal is {value}, which is not a finite number greater than 0")]
    MdeAtProposalNotPositive { value: f64 },

    #[error("outcome.prior_coefficient.value is {value}, which is not a finite number")]
    PriorCoefficientNotFinite { value: f64 },

    #[error("design.waves lists a wave and design.switchback is set")]
    WavesAndSwitchback,

    /// `key` is the offending key; for `anticipation_days` it carries the words
    /// "(non-zero)" because only a non-zero value is refused.
    #[error("{key} is set alongside design.switchback")]
    SwitchbackForbids { key: &'static str },

    #[error(
        "design.washout_days is {washout_days}, which is not shorter than design.switchback.period_days ({period_days})"
    )]
    SwitchbackWashoutTooLong { washout_days: u32, period_days: u32 },

    #[error(
        "lever.read_back.grace_days is {grace_days}, which is not shorter than design.switchback.period_days ({period_days})"
    )]
    SwitchbackGraceTooLong { grace_days: u32, period_days: u32 },

    /// At a week or month grain every date the design names must open a period,
    /// or the panel's first post period would mix days before and after it.
    #[error("{key} is {date}, which does not start a {}: {}", grain.as_str(), period_start_hint(*grain))]
    NotAPeriodStart {
        key: String,
        date: Moment,
        grain: Grain,
    },

    #[error("design.switchback.period_days is 0")]
    SwitchbackPeriodDaysZero,

    /// `why` names the first fault found, judged on the periods in file order.
    #[error("design.switchback.periods do not form consecutive pairs of one on and one off: {why}")]
    PeriodsNotPaired { why: String },

    #[error("{key} is absent on a design that lists waves")]
    MissingWindow { key: &'static str },

    #[error("unit {unit:?} appears in more than one stratum of design.strata")]
    UnitInTwoStrata { unit: String },

    #[error("design.waves lists no wave")]
    MissingWaves,

    #[error("design.switchback.periods lists no period")]
    MissingPeriods,

    #[error("design.seed is absent")]
    MissingSeed,

    #[error("outcome.mde_at_proposal is absent")]
    MissingMdeAtProposal,

    #[error("lever is absent")]
    MissingLever,

    #[error(
        "outcome.denominator makes the outcome a ratio, which is read from past data only: pricing cannot size one yet"
    )]
    RatioNotRegistrable,

    #[error("outcome.denominator is set alongside design.switchback; a ratio outcome needs waves")]
    RatioOnSwitchback,

    #[error(
        "design.assignment is observed: an observed rollout already happened, so it is analysed from past data, not registered"
    )]
    ObservedNotRegistrable,

    #[error(
        "{:?} (wave {}, switch_on {}) matches no name in the unit pool being checked (units, or groups under assign_by){}",
        .unit, .wave, .switch_on, suggestion_suffix(.suggestions)
    )]
    UnknownUnit {
        unit: String,
        wave: usize,
        switch_on: Moment,
        suggestions: Vec<String>,
    },

    #[error(
        "{:?} (stratum {}) matches no name in the unit pool being checked (units, or groups under assign_by){}",
        .unit, .stratum, suggestion_suffix(.suggestions)
    )]
    UnknownStratumUnit {
        unit: String,
        stratum: usize,
        suggestions: Vec<String>,
    },

    #[error("unit {unit:?} of the pool is in no stratum of design.strata")]
    UnitInNoStratum { unit: String },

    #[error(
        "all {units} units in the pool (or groups, under assign_by) are switched on the same day, so none is left as a control; hold some back, or stagger them over two or more switch dates"
    )]
    NoControlUnits { units: usize },

    /// `for_each` names an entity whose key is more than one dimension. v1
    /// needs one unit name per store for a wave to list, so it is refused
    /// (`internal-docs/experiments.md`, "Platform refusals"). Raised by the
    /// host while it resolves the entity through the semantic model — the
    /// file alone cannot know.
    #[error(
        "`for_each: {entity}` has a composite key ({}); v1 needs a single key dimension so a wave can list one name per unit",
        .keys.join(", ")
    )]
    CompositeKey { entity: String, keys: Vec<String> },

    /// `assign_by` randomises groups, and a group's day is the SUM of its
    /// members' (spec, "Groups of units"). Only a sum- or count-typed measure
    /// adds up; the host raises this when it reads the measure's type.
    /// `aggregate` is the measure's SQL aggregate (`AVG`, `COUNT_DISTINCT`, …).
    #[error(
        "an `assign_by` design sums the measure `{measure}` over each group's members, but it aggregates with {aggregate}, which does not add up; v1 needs a sum- or count-typed target"
    )]
    AssignByNotAdditive { measure: String, aggregate: String },

    /// `assign_by` names no dimension of the entity's owning view, or one that
    /// cannot assign groups. `why` states what the host tested.
    #[error("`assign_by: {assign_by}` could not be resolved: {why}")]
    AssignByUnresolved { assign_by: String, why: String },

    /// Staggered waves all turn off at close, a different time after each switch,
    /// so one carryover window would read a different lag for each.
    #[error(
        "design.carryover_days needs one switch date for every treated unit; this design has {switch_dates}"
    )]
    CarryoverNeedsOneSwitchDate { switch_dates: usize },

    #[error(
        "design.carryover_days reads the days after the levers turn off, but lever.on_close is keep, so they never do"
    )]
    CarryoverWithLeversKept,

    #[error("design.carryover_days is 0; leave it out to read no carryover")]
    CarryoverZero,

    /// A measure filter names a dimension of another view. The filter would join
    /// that view in and fan the measure out, so v1 keeps it to the measure's own.
    #[error(
        "{field}: `{dimension}` is not a dimension of `{measure}`'s view; a filter narrows its measure's own view, `{}`",
        .measure.split('.').next().unwrap_or_default()
    )]
    FilterOutsideView {
        field: String,
        dimension: String,
        measure: String,
    },

    #[error("{field}: a filter needs at least one value")]
    FilterNoValues { field: String },

    #[error("{field} is set but the measure it narrows is not")]
    FilterWithoutMeasure { field: String },

    /// `field` is the path of the offending key; `error` names the expression
    /// and what `parse_comparison` expected instead.
    #[error("{field}: {error}")]
    InvalidExpression { field: String, error: ExprError },
}

/// Every refusal decidable from the file alone — run at compile time. A draft
/// passes: an absent assignment / `seed` / `mde_at_proposal` is
/// [`is_registrable`]'s business, not this function's.
pub fn validate_static(c: &ExperimentConfig) -> Vec<ConfigError> {
    let mut out = Vec::new();
    design::check_design(c, &mut out);
    check_waves(c, &mut out);
    check_rates(c, &mut out);
    check_windows(c, &mut out);
    check_ratio(c, &mut out);
    check_guardrails(c, &mut out);
    check_expressions(c, &mut out);
    filters::check_filters(c, &mut out);
    out
}

/// Strictly inside (0, 1). Written so NaN fails.
fn in_open_unit(x: f64) -> bool {
    x > 0.0 && x < 1.0
}

/// In (0, 1]. Written so NaN fails.
fn in_half_open_unit(x: f64) -> bool {
    x > 0.0 && x <= 1.0
}

fn check_waves(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let waves = &c.design.waves;
    let mut first_wave_of_unit: HashMap<&str, usize> = HashMap::new();
    let mut first_wave_on_date: HashMap<Moment, usize> = HashMap::new();
    for (i, wave) in waves.iter().enumerate() {
        let n = i + 1;
        if wave.units.is_empty() {
            out.push(ConfigError::EmptyWave {
                wave: n,
                switch_on: wave.switch_on,
            });
        }
        match first_wave_on_date.get(&wave.switch_on) {
            Some(&first) => out.push(ConfigError::DuplicateSwitchDate {
                switch_on: wave.switch_on,
                first_wave: first,
                second_wave: n,
            }),
            None => {
                first_wave_on_date.insert(wave.switch_on, n);
            }
        }
        for unit in &wave.units {
            match first_wave_of_unit.get(unit.as_str()) {
                Some(&first) if first != n => out.push(ConfigError::UnitInTwoWaves {
                    unit: unit.clone(),
                    first_wave: first,
                    first_switch_on: waves[first - 1].switch_on,
                    second_wave: n,
                    second_switch_on: wave.switch_on,
                }),
                Some(_) => {}
                None => {
                    first_wave_of_unit.insert(unit.as_str(), n);
                }
            }
        }
    }
}

fn check_rates(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let (o, d) = (&c.outcome, &c.design);
    if !in_open_unit(o.alpha) {
        out.push(ConfigError::AlphaOutOfRange { value: o.alpha });
    }
    if !in_open_unit(o.power) {
        out.push(ConfigError::PowerOutOfRange { value: o.power });
    }
    if !in_half_open_unit(d.coverage_floor) {
        out.push(ConfigError::CoverageFloorOutOfRange {
            value: d.coverage_floor,
        });
    }
    if !in_half_open_unit(d.compliance_floor) {
        out.push(ConfigError::ComplianceFloorOutOfRange {
            value: d.compliance_floor,
        });
    }
}

fn check_ratio(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    if c.outcome.denominator.is_some() && c.design.switchback.is_some() {
        out.push(ConfigError::RatioOnSwitchback);
    }
}

fn check_windows(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    if c.outcome.family < 1 {
        out.push(ConfigError::FamilyBelowOne {
            value: c.outcome.family,
        });
    }
    if c.design.pre_days == Some(0) {
        out.push(ConfigError::PreDaysZero);
    }
    if c.design.post_days == Some(0) {
        out.push(ConfigError::PostDaysZero);
    }
    if let Some(mde) = c.outcome.mde_at_proposal
        && !(mde.is_finite() && mde > 0.0)
    {
        out.push(ConfigError::MdeAtProposalNotPositive { value: mde });
    }
    if let Some(prior) = &c.outcome.prior_coefficient
        && !prior.value.is_finite()
    {
        out.push(ConfigError::PriorCoefficientNotFinite { value: prior.value });
    }
}

fn check_guardrails(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    for (i, g) in c.guardrails.iter().enumerate() {
        if let GuardrailCriteria::Anomaly { significance, .. } = &g.criteria
            && !in_open_unit(*significance)
        {
            out.push(ConfigError::GuardrailSignificanceOutOfRange {
                guardrail: i,
                value: *significance,
            });
        }
    }
}

/// `read_back` and `trigger` expressions must parse now, not on the day the
/// flip needs checking. Anomaly guardrails carry no expression.
fn check_expressions(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    if let Some(lever) = &c.lever {
        let read_back = &lever.read_back;
        for (field, source) in [
            ("lever.read_back.when_on", &read_back.when_on),
            ("lever.read_back.when_off", &read_back.when_off),
        ] {
            if let Err(error) = parse_comparison(source) {
                let field = field.to_string();
                out.push(ConfigError::InvalidExpression { field, error });
            }
        }
    }
    // A trigger may also compare a ratio of two measures (ACoS = spend / sales).
    for (i, g) in c.guardrails.iter().enumerate() {
        if let GuardrailCriteria::Trigger { when } = &g.criteria
            && let Err(error) = parse_trigger(when)
        {
            let field = format!("guardrails[{i}].criteria.when");
            out.push(ConfigError::InvalidExpression { field, error });
        }
    }
}

/// The tail of a refusal that names a unit matching nothing: the nearest names,
/// or the fact that none is near.
fn suggestion_suffix(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        format!("; no name is within {MAX_EDITS} edits of it")
    } else {
        let quoted: Vec<String> = suggestions.iter().map(|s| format!("{s:?}")).collect();
        format!("; closest: {}", quoted.join(", "))
    }
}

#[cfg(test)]
mod tests_static;

#[cfg(test)]
mod tests_design;

#[cfg(test)]
mod tests_units;

#[cfg(test)]
mod tests_registrable;

#[cfg(test)]
mod tests_strata;

#[cfg(test)]
mod tests_host;

fn period_start_hint(g: Grain) -> &'static str {
    match g {
        Grain::Hour => "at hour grain each switch is a whole local hour, such as 2026-10-26T14:00",
        Grain::Day => "at day grain each switch is a date (its midnight), with no time of day",
        Grain::Week => "at week grain each date must be a Monday",
        Grain::Month => "at month grain each date must be the 1st of a month",
    }
}

#[cfg(test)]
mod tests_observed;

#[cfg(test)]
mod tests_ratio;
