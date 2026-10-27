//! What differs by design: the unit of the estimate, its totals, and the assumptions a design adds.

use crate::config::ExperimentConfig;
use crate::grain::Grain;

use crate::engine_in::SizeBiasIn;

use super::build::{finite, interval};
use super::types::*;

pub const CARRYOVER: &str = "Carryover longer than the washout leaks an on period's effect into the next off period, \
                             which biases the estimate toward zero.";
pub const PULL_FORWARD: &str = "Pull-forward — a customer who buys during an on period instead of later depletes the \
                                next off period — biases the estimate away from zero; it is why a switchback over goods \
                                bought once a month needs periods longer than a month.";

pub fn design_kind(cfg: &ExperimentConfig) -> DesignKind {
    if cfg.design.switchback.is_some() {
        DesignKind::Switchback
    } else if cfg.assign_by.is_some() {
        DesignKind::Groups
    } else {
        DesignKind::Units
    }
}

/// "unit" or "group": the word before `-day`. A switchback's headline is a fleet mean per unit-day.
pub fn noun(kind: DesignKind) -> &'static str {
    match kind {
        DesignKind::Groups => "group",
        DesignKind::Units | DesignKind::Switchback => "unit",
    }
}

/// One panel cell: "unit-day", "group-week", "unit-month".
pub fn cell(kind: DesignKind, grain: Grain) -> String {
    format!("{}-{}", noun(kind), grain.as_str())
}

pub fn estimand(kind: DesignKind, grain: Grain) -> String {
    let g = grain.as_str();
    match kind {
        DesignKind::Units => format!("intention-to-treat effect per unit-{g}"),
        DesignKind::Groups => format!(
            "intention-to-treat effect per group-{g} (a group's {g} is the sum of its members')"
        ),
        DesignKind::Switchback => format!(
            "short-run effect of on against off under alternation, per unit-{g} — not the \
             effect of leaving the lever on"
        ),
    }
}

/// Stated every time, whatever the estimator said: neither is identifiable inside the design.
pub fn assumptions(kind: DesignKind) -> Vec<String> {
    match kind {
        DesignKind::Switchback => vec![CARRYOVER.to_string(), PULL_FORWARD.to_string()],
        DesignKind::Units | DesignKind::Groups => Vec::new(),
    }
}

/// `mean × units × days` over the same population — arithmetic. For a switchback the population
/// is the fleet and the days are the on-periods'; without the facts there is no number to invent.
pub fn treated_total(i: &ReportInput<'_>, est: f64, kind: DesignKind) -> Option<TreatedTotal> {
    let g = i.config.design.grain.as_str();
    let (units, days, basis) = match kind {
        DesignKind::Units => (
            i.n_treated,
            i.post_days,
            format!(
                "{est:+.2} × {} treated units × {} post-window {g}s — exact arithmetic on the treated units, not a projection",
                i.n_treated, i.post_days
            ),
        ),
        DesignKind::Groups => (
            i.n_treated,
            i.post_days,
            format!(
                "{est:+.2} × {} treated groups × {} post-window {g}s — exact arithmetic: a group's {g} is the sum of \
                 its members', so this is the sum over those groups, not a projection",
                i.n_treated, i.post_days
            ),
        ),
        DesignKind::Switchback => {
            let f = i.switchback?;
            (
                f.fleet_units,
                f.on_days,
                format!(
                    "{est:+.2} × {} fleet units × {} on-period {g}s (the post-washout {g}s of the retained pairs, \
                     as the engine counted them) — arithmetic over the {g}s the lever was on; not a projection, and not the effect \
                     of leaving it on",
                    f.fleet_units, f.on_days
                ),
            )
        }
    };
    let scale = units as f64 * days as f64;
    Some(TreatedTotal {
        units,
        days,
        total: est * scale,
        interval: interval(
            finite(i.target.ci_low).map(|l| l * scale),
            finite(i.target.ci_high).map(|h| h * scale),
        ),
        basis,
    })
}

/// Always labelled an assumption. A switchback treats the whole fleet on its on-days, so there
/// are no untreated units to project onto and none is offered.
pub fn projection(i: &ReportInput<'_>, est: f64, kind: DesignKind) -> Option<Projection> {
    if kind == DesignKind::Switchback || i.n_untreated == 0 {
        return None;
    }
    let nouns = if kind == DesignKind::Groups {
        "groups"
    } else {
        "units"
    };
    Some(Projection {
        label: PROJECTION_LABEL.to_string(),
        untreated_units: i.n_untreated,
        days: i.post_days,
        total: est * i.n_untreated as f64 * i.post_days as f64,
        assumption: format!(
            "If the treated {nouns}' mean effect held on the {} untreated {nouns} for {} {}s. This is an \
             assumption: the treated sample was not drawn to represent them, and a non-significant size \
             diagnostic does not establish that it does.",
            i.n_untreated,
            i.post_days,
            i.config.design.grain.as_str()
        ),
        warning: size_bias_warning(i.target.size_bias.as_ref()),
    })
}

fn size_bias_warning(sb: Option<&SizeBiasIn>) -> Option<String> {
    let sb = sb.filter(|s| s.significant)?;
    // A missing correlation is a number the engine could not compute, never zero: say so and
    // name no direction.
    let (Some(treated), Some(control)) = (
        finite(sb.correlation_treated),
        finite(sb.correlation_control),
    ) else {
        return Some(
            "Unit size predicts the effect, but a correlation was not computable, so the direction is unknown: \
             projecting onto units of a different size may over- or understate the effect."
                .to_string(),
        );
    };
    let numbers = format!("treated r = {treated:.2} against controls' {control:.2}");
    Some(if treated - control > 0.0 {
        format!(
            "Unit size predicts the effect ({numbers}): larger units gained more. Projecting onto units \
             smaller than the treated ones likely overstates the effect; onto larger ones, understates it."
        )
    } else {
        format!(
            "Unit size predicts the effect ({numbers}): smaller units gained more. Projecting onto units \
             smaller than the treated ones likely understates the effect; onto larger ones, overstates it."
        )
    })
}
