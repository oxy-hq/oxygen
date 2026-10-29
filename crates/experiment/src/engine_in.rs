//! Serde mirrors of the engine's result structs.
//!
//! `oxy-experiment` may not depend on airlayer — `oxy-compile` depends on this
//! crate, and compiling a YAML file must not link the semantic engine. So the
//! host serialises an engine result and reads it back as one of these.
//!
//! `serde_json` writes ±∞ and NaN as `null`; a `null` therefore reads back as
//! `None`, and `None` means *unbounded* (an interval endpoint) or *not a
//! number* (an estimate) — never zero. Everything is `#[serde(default)]` so an
//! engine field this crate does not need cannot break the read. A field it does
//! need going missing is *not* caught here — it reads back as the default — so the
//! host's `mirror_tests` assert every key declared below is one the engine
//! serialises.

use serde::{Deserialize, Serialize};

/// Builds the value a computation returns when it declined, with its reason.
pub trait Refused {
    fn refused(reason: String) -> Self;
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreTrendIn {
    pub estimate: Option<f64>,
    pub t_stat: Option<f64>,
    pub diverged: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SizeBiasIn {
    pub correlation_treated: Option<f64>,
    pub correlation_control: Option<f64>,
    pub significant: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectIn {
    pub estimate: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub p_value: Option<f64>,
    pub n_treated: usize,
    pub n_control: usize,
    pub significant: bool,
    pub dropped_waves: Vec<String>,
    pub pre_trend: Option<PreTrendIn>,
    pub size_bias: Option<SizeBiasIn>,
    /// Switchback only (C1): retained post-washout days across the retained ON periods.
    /// `None` for a waves design — and for a switchback it is a section refusal, not a
    /// number to reconstruct.
    pub retained_on_days: Option<usize>,
    /// Units left out because their data was missing or too sparse, each with
    /// the reason: the engine's own, and the host's panel builder's.
    pub dropped_units: Vec<DroppedUnitIn>,
    /// Each compared wave's own effect, in switch order (one entry on a common
    /// switch date). Not adjusted for the number of waves.
    pub waves: Vec<WaveEffectIn>,
    pub refusal: Option<String>,
}

/// One wave's own Welch effect. `switch_day` is a `num_days_from_ce` ordinal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveEffectIn {
    pub switch_day: i64,
    pub n_treated: usize,
    pub n_control: usize,
    pub estimate: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub p_value: Option<f64>,
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DroppedUnitIn {
    pub unit: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerIn {
    pub mde: Option<f64>,
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RatioIn {
    pub coefficient: Option<f64>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    pub first_stage: EffectIn,
    pub refusal: Option<String>,
}

/// A synthetic control's fits, beside its effect (`EffectIn`). One entry per
/// treated unit, in the engine's order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyntheticIn {
    pub fits: Vec<UnitFitIn>,
    /// The donor pool the fits drew on, by name.
    pub donors: Vec<String>,
    /// Placebo sets behind the p-value, and whether they were all of them.
    pub placebo_sets: usize,
    pub exact: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitFitIn {
    pub unit: String,
    pub effect: Option<f64>,
    /// Holdout pre-window error over the unit's own pre-window volatility:
    /// 0 tracks perfectly, 1 is no better than a flat line at its mean.
    pub pre_fit: Option<f64>,
    pub rmspe_ratio: Option<f64>,
    pub rank_p: Option<f64>,
    /// Non-zero weights only, largest first.
    pub weights: Vec<DonorWeightIn>,
    /// Pre window then post window; `day` is a period ordinal.
    pub series: Vec<SyntheticPointIn>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DonorWeightIn {
    pub unit: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyntheticPointIn {
    pub day: i64,
    pub actual: Option<f64>,
    pub synthetic: Option<f64>,
}

impl Refused for EffectIn {
    fn refused(reason: String) -> Self {
        EffectIn {
            refusal: Some(reason),
            ..Default::default()
        }
    }
}

impl Refused for PowerIn {
    fn refused(reason: String) -> Self {
        PowerIn {
            refusal: Some(reason),
            ..Default::default()
        }
    }
}

/// A refused synthetic control has no fits; its effect carries the reason.
impl Refused for SyntheticIn {
    fn refused(_reason: String) -> Self {
        SyntheticIn::default()
    }
}

impl Refused for RatioIn {
    fn refused(reason: String) -> Self {
        RatioIn {
            refusal: Some(reason),
            ..Default::default()
        }
    }
}
