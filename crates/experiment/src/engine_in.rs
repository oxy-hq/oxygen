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

impl Refused for RatioIn {
    fn refused(reason: String) -> Self {
        RatioIn {
            refusal: Some(reason),
            ..Default::default()
        }
    }
}
