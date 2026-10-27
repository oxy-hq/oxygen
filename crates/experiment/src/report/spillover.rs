//! What the lever did outside the target cell: substitution from the registered
//! sibling, and the carryover after the lever turned off. Pure, one rule per
//! function, like the rest of the report.

use crate::config::{Direction, measure_label};
use crate::engine_in::{EffectIn, PowerIn};

use super::build::{finite, num, pct, summary};
use super::design::{cell, design_kind};
use super::types::*;

/// The effect over the carryover window, read against the measured effect: lingering when
/// significant and the same way, pull-forward when significant the other way.
pub(super) fn carryover(
    i: &ReportInput<'_>,
    target_estimate: Option<f64>,
) -> Option<CarryoverView> {
    let days = i.config.design.carryover_days?;
    let e = i.carryover?;
    let effect = summary(e);
    let g = i.config.design.grain.as_str();
    let window = format!("the {days} {g}s after the lever turned off");
    let level = pct(1.0 - i.config.outcome.alpha);
    let ci = |est: f64| match (effect.interval.low, effect.interval.high) {
        (Some(l), Some(h)) => format!("{est:+.2}, {level} CI {l:+.2} to {h:+.2}"),
        _ => format!("{est:+.2}, interval unbounded"),
    };
    let cell = cell(design_kind(i.config), i.config.design.grain);
    let target_up = target_estimate
        .map_or(i.config.outcome.direction == Direction::Increase, |t| {
            t >= 0.0
        });
    let (lingered, statement) = match (&effect.refusal, effect.estimate) {
        (Some(why), _) => (false, format!("carryover not measured: {why}")),
        (None, None) => (
            false,
            "carryover not measured: the estimate is not a finite number".into(),
        ),
        (None, Some(est)) if !effect.significant => (
            false,
            format!("no effect was measurable over {window} ({})", ci(est)),
        ),
        (None, Some(est)) if (est >= 0.0) == target_up => (
            true,
            match (effect.interval.low, effect.interval.high) {
                (Some(l), Some(h)) => format!(
                    "the effect lingered after the lever turned off: {est:+.2} per {cell} over the next {days} {g}s ({level} CI {l:+.2} to {h:+.2})"
                ),
                _ => format!(
                    "the effect lingered after the lever turned off: {est:+.2} per {cell} over the next {days} {g}s"
                ),
            },
        ),
        (None, Some(est)) => (
            false,
            format!(
                "after the lever turned off the outcome moved the other way over {window} ({}): pull-forward, the effect brought sales earlier rather than adding them",
                ci(est)
            ),
        ),
    };
    Some(CarryoverView {
        days,
        effect,
        lingered,
        statement,
    })
}

/// Z is the sibling design's MDE and is only ever *stated*.
pub(super) fn substitution(i: &ReportInput<'_>, kind: DesignKind) -> SubstitutionView {
    let Some(check) = &i.config.substitution_check else {
        return SubstitutionView {
            sibling: None,
            floor: None,
            statement: "no substitution check was registered".to_string(),
            sibling_effect: None,
            detected: false,
        };
    };
    let name = measure_label(&check.sibling, &check.filter);
    let (floor, mut statement) = match i.sibling_power {
        Some(p) => floor_statement(p, &name, &cell(kind, i.config.design.grain)),
        None => (
            None,
            "substitution check unavailable: the sibling's power was not computed".to_string(),
        ),
    };
    let taken = i.sibling.and_then(|s| taken_from_sibling(i, s));
    if let Some((estimate, low, high)) = taken {
        statement = format!(
            "substitution detected: {name} fell {} per {} ({} CI {} to {})",
            num(estimate.abs()),
            cell(kind, i.config.design.grain),
            pct(1.0 - i.config.outcome.alpha),
            num(low),
            num(high),
        );
    }
    SubstitutionView {
        sibling: Some(name),
        floor,
        statement,
        sibling_effect: i.sibling.map(summary),
        detected: taken.is_some(),
    }
}

/// The sibling's estimate and interval when it moved significantly against the
/// target — the lift was at least partly taken from it. Against the measured
/// target when there is one, else against the registered direction. A sibling
/// that rose with the target is a halo, not substitution.
fn taken_from_sibling(i: &ReportInput<'_>, s: &EffectIn) -> Option<(f64, f64, f64)> {
    if !s.significant || s.refusal.is_some() {
        return None;
    }
    let (estimate, low, high) = (finite(s.estimate)?, finite(s.ci_low)?, finite(s.ci_high)?);
    let target_up = match finite(i.target.estimate).filter(|e| *e != 0.0) {
        Some(e) => e > 0.0,
        None => i.config.outcome.direction == Direction::Increase,
    };
    (target_up == (estimate < 0.0)).then_some((estimate, low, high))
}

fn floor_statement(p: &PowerIn, sibling: &str, cell: &str) -> (Option<f64>, String) {
    if let Some(r) = &p.refusal {
        return (
            None,
            format!(
                "substitution check unavailable: the sibling's power computation was refused: {r}"
            ),
        );
    }
    match finite(p.mde) {
        Some(mde) => (
            Some(mde),
            format!("substitution not excluded above {} per {cell} on {sibling}", num(mde)),
        ),
        None => (
            None,
            "substitution check unavailable: the sibling's minimum detectable effect is not a finite number"
                .to_string(),
        ),
    }
}
