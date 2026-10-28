//! Each wave's own effect beside the pooled one, and the proportional scale.
//!
//! The pooled estimate is a size-weighted mean over waves, so one wave that clearly
//! moved can be averaged away by waves that did not. The report lists the parts
//! when there are at least two, each in the headline's own scale.

use crate::config::{ExperimentConfig, Scale};
use crate::engine_in::{EffectIn, WaveEffectIn};

use super::build::{finite, interval};
use super::types::WaveView;

/// What a proportional headline is a number of.
pub const PROPORTIONAL_ESTIMAND: &str = "proportional change per unit: the mean change in \
     ln(1 + outcome) between the windows, read back as a percentage (exp(x) \u{2212} 1), so \
     every unit counts alike whatever its size";

pub fn is_proportional(c: &ExperimentConfig) -> bool {
    c.outcome.scale == Scale::Proportional
}

/// `exp(x) − 1`: a change in `ln(1 + y)` read back as a share. An unbounded end
/// (`None`) stays unbounded.
fn share(x: Option<f64>) -> Option<f64> {
    x.map(f64::exp_m1)
}

/// The target as the report reads it: on a proportional scale its estimate and
/// interval (and each wave's) are shares; everything else is unchanged.
pub fn in_report_scale(c: &ExperimentConfig, t: &EffectIn) -> EffectIn {
    if !is_proportional(c) {
        return t.clone();
    }
    EffectIn {
        estimate: share(t.estimate),
        ci_low: share(t.ci_low),
        ci_high: share(t.ci_high),
        waves: t
            .waves
            .iter()
            .map(|w| WaveEffectIn {
                estimate: share(w.estimate),
                ci_low: share(w.ci_low),
                ci_high: share(w.ci_high),
                ..w.clone()
            })
            .collect(),
        ..t.clone()
    }
}

/// The waves, when the estimate has numbers and more than one wave.
pub fn waves(c: &ExperimentConfig, t: &EffectIn, est: Option<f64>) -> Vec<WaveView> {
    if est.is_none() || t.waves.len() < 2 {
        return Vec::new();
    }
    t.waves
        .iter()
        .map(|w| {
            let iv = interval(w.ci_low, w.ci_high);
            // Significant at the registered per-comparison level: the interval
            // (built at that level) excludes zero.
            let excludes_zero = matches!((iv.low, iv.high), (Some(l), _) if l > 0.0)
                || matches!((iv.low, iv.high), (_, Some(h)) if h < 0.0);
            WaveView {
                switch_on: c.design.grain.start_of(w.switch_day),
                n_treated: w.n_treated,
                n_control: w.n_control,
                estimate: finite(w.estimate),
                significant: w.refusal.is_none() && excludes_zero,
                interval: iv,
                p_value: finite(w.p_value),
                refusal: w.refusal.clone(),
            }
        })
        .collect()
}

/// `+12.3%`.
pub fn signed_pct(x: f64) -> String {
    format!("{:+.1}%", x * 100.0)
}
