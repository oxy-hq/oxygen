//! A synthetic control's section: each treated unit's fit, its donors and its
//! chart, and what the estimate assumes because nothing was randomised.

use crate::config::{Comparison, ExperimentConfig};
use crate::engine_in::{SyntheticIn, UnitFitIn};

use super::build::finite;
use super::types::{DonorWeightView, SyntheticFitView, SyntheticPointView, SyntheticView};
use super::waves::is_proportional;

/// Stated on every synthetic-control report, beside the retrospective label.
pub const SYNTHETIC_LABEL: &str = "Not randomised \u{2014} synthetic control";

pub const NO_SPILLOVER: &str = "No spillover into the donors: a unit the change reached too \
     (a sibling product, a neighbouring store) would move its synthetic control with it and \
     shrink the estimate. Leave such units out of the donors.";
pub const DONORS_UNTOUCHED: &str = "Nothing else changed for the treated units alone after the \
     switch: a synthetic control tracks what the donors did, so a change only the treated units \
     saw is read as the effect.";

pub fn is_synthetic(c: &ExperimentConfig) -> bool {
    c.design.comparison == Comparison::SyntheticControl
}

pub fn assumptions(c: &ExperimentConfig) -> Vec<String> {
    if is_synthetic(c) {
        vec![NO_SPILLOVER.to_string(), DONORS_UNTOUCHED.to_string()]
    } else {
        Vec::new()
    }
}

/// What the headline is a number of, when it is a synthetic control's.
pub fn estimand(c: &ExperimentConfig) -> String {
    let g = c.design.grain.as_str();
    format!(
        "effect per treated unit-{g} against its own synthetic control (a weighted mix of the \
         untreated units that tracked it before the switch), averaged over the treated units"
    )
}

/// The section, in the headline's scale: on a proportional outcome the fits ran on
/// `ln(1 + y)`, so each effect reads back as a share and the chart in levels.
pub fn view(c: &ExperimentConfig, s: &SyntheticIn) -> SyntheticView {
    let proportional = is_proportional(c);
    SyntheticView {
        label: SYNTHETIC_LABEL.to_string(),
        donors: s.donors.len(),
        placebo_sets: s.placebo_sets,
        exact: s.exact,
        fits: s.fits.iter().map(|f| fit(c, f, proportional)).collect(),
    }
}

fn fit(c: &ExperimentConfig, f: &UnitFitIn, proportional: bool) -> SyntheticFitView {
    let level = |x: Option<f64>| finite(x).map(|v| if proportional { v.exp_m1() } else { v });
    SyntheticFitView {
        unit: f.unit.clone(),
        effect: level(f.effect),
        pre_fit: finite(f.pre_fit),
        rank_p: finite(f.rank_p),
        weights: f
            .weights
            .iter()
            .map(|w| DonorWeightView {
                unit: w.unit.clone(),
                weight: w.weight,
            })
            .collect(),
        series: f
            .series
            .iter()
            .map(|p| SyntheticPointView {
                date: c.design.grain.start_of(p.day),
                actual: level(p.actual),
                synthetic: level(p.synthetic),
            })
            .collect(),
    }
}
