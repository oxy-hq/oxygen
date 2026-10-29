//! The report as data.
//!
//! Every view here is **owned and serde-round-trippable**: the spec requires that
//! "a re-read years later renders the same page". An infinite bound is stored as
//! `None` and serialises as `null`; every consumer renders `null` as *unbounded*.

use serde::{Deserialize, Serialize};

use crate::compliance::ComplianceSummary;
use crate::config::{ExperimentConfig, PriorSource};
use crate::engine_in::{EffectIn, PowerIn, RatioIn, SyntheticIn};
use crate::patch::{CoefficientEdit, PatchRefusal, ReplacedPrior};

/// Shown on every projection, whatever the size diagnostic says.
pub const PROJECTION_LABEL: &str = "Assumption, not a measurement";

/// The unit the estimate is stated in: stores, groups of stores, or the whole fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignKind {
    Units,
    Groups,
    Switchback,
}

/// What a switchback's treated total multiplies by: `n_treated` on the effect is retained
/// *pairs*, so the fleet size comes from the panel and the days from the engine's own
/// `EffectResult.retained_on_days` (C1) — exact, not reconstructed from the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchbackFacts {
    pub fleet_units: usize,
    pub on_days: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriorView {
    pub value: f64,
    pub source: PriorSource,
}

/// The World Model's prediction against the measurement. Rendered from `statement`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredictionView {
    /// `prior.value × ITT(driver)`.
    pub predicted: f64,
    /// `ITT(target)`.
    pub measured: f64,
    pub measured_interval: Interval,
    pub prior: PriorView,
    /// Whether the prior lies inside the Anderson–Rubin set; `None` when that set is unbounded.
    pub prior_in_interval: Option<bool>,
    pub statement: String,
}

/// The seed each random step used, so the whole computation reproduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seeds {
    pub effect: u64,
    pub placebo: u64,
    pub ratio: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportStatus {
    Registered,
    Aborted {
        reason: Option<String>,
        /// The owner aborted keeping the levers as they were: no `off` was sent, and the
        /// units were left in whatever state the run had put them. Absent before it was
        /// recorded, which reads as turned off.
        #[serde(default)]
        levers_kept: bool,
    },
    Retrospective,
    /// A reading of a run still in progress, over the days read so far (`crate::interim`).
    /// Never stored as a result.
    Interim {
        through: chrono::NaiveDate,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub low: Option<f64>,
    pub high: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Headline {
    pub estimate: f64,
    /// `None` when either endpoint is unbounded.
    pub half_width: Option<f64>,
    pub interval: Interval,
    pub p_value: Option<f64>,
    pub significant: bool,
    /// "significant at α = 0.05" | "directional at α = 0.2" | "not distinguishable from zero at α = …"
    pub verdict: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreatedTotal {
    pub units: usize,
    pub days: usize,
    pub total: f64,
    pub interval: Interval,
    pub basis: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub label: String,
    pub untreated_units: usize,
    pub days: usize,
    pub total: f64,
    pub assumption: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSummary {
    pub estimate: Option<f64>,
    pub interval: Interval,
    pub significant: bool,
    pub refusal: Option<String>,
}

/// The effect over the days after the levers turned off at close.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarryoverView {
    pub days: u32,
    pub effect: EffectSummary,
    /// Significant and in the direction of the measured effect.
    pub lingered: bool,
    pub statement: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstitutionView {
    pub sibling: Option<String>,
    /// The sibling design's MDE — the smallest substitution this experiment
    /// could have read. Stated, never subtracted.
    pub floor: Option<f64>,
    pub statement: String,
    pub sibling_effect: Option<EffectSummary>,
    /// The sibling moved significantly against the target: the lift was at least
    /// partly taken from it. Defaulted so reports sealed before it existed still read.
    #[serde(default)]
    pub detected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectionView {
    pub predicted: String,
    pub observed: String,
    pub mismatch: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromiseView {
    pub mde_at_proposal: f64,
    pub abs_estimate: f64,
    pub below_promise: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComplianceView {
    pub c_treated: f64,
    pub c_control_leak: f64,
    /// `c_treated − c_control_leak` — the dilution factor on the lever's own effect.
    pub net: f64,
    pub floor: f64,
    pub below_floor: bool,
    pub restated_mde: Option<f64>,
    pub warning: Option<String>,
    pub restated: Option<String>,
    /// Entries pooled into `c_treated` / `c_control_leak` (C2). `n_treated == 0` means
    /// compliance was not measured.
    pub n_treated: usize,
    pub n_control: usize,
    /// Some only when `n_treated == 0`: the sentence the section shows instead of a share.
    pub not_measured: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreTrendView {
    pub estimate: Option<f64>,
    pub t_stat: Option<f64>,
    pub diverged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChecksView {
    pub pre_trend: Option<PreTrendView>,
    pub pre_trend_note: String,
    /// Each entry is the engine's own line: the wave and why it was dropped.
    pub dropped_waves: Vec<String>,
    /// `unit: reason` for each unit the estimate left out (missing or too
    /// sparse data). Absent on a report sealed before units could be dropped.
    #[serde(default)]
    pub dropped_units: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalsView {
    pub count: usize,
    pub message: String,
    /// The assignment was declared (`design.assignment: observed`), not drawn:
    /// `message` says so and `count` is not a count of draws.
    #[serde(default)]
    pub observational: bool,
    /// Proposals were logged, but the analysed split is none of them: it was
    /// written by hand after pricing.
    #[serde(default)]
    pub hand_assigned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatioView {
    pub coefficient: Option<f64>,
    pub interval: Interval,
    /// True when either endpoint is unbounded — reported as such, never as a number.
    pub unbounded: bool,
    pub first_stage: EffectSummary,
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WriteBackView {
    Proposed {
        coefficient: f64,
        target: String,
        driver: String,
        file_path: String,
        refs_entry: String,
        /// The prior this edit would replace, when the run froze one.
        replaces: Option<ReplacedPrior>,
    },
    Refused {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportView {
    pub status: ReportStatus,
    /// One line that says what this is. An abort is never labelled "registered".
    pub label: String,
    pub experiment: String,
    pub hypothesis: String,
    pub target: String,
    pub unit: String,
    pub design: DesignKind,
    /// `design.grain`: every per-unit figure is per unit-period of it. A report sealed
    /// before grains existed reads back as day.
    #[serde(default)]
    pub grain: crate::grain::Grain,
    /// What the headline is a number *of*: per unit-day, per group-day, or a switchback's short-run effect.
    pub estimand: String,
    /// Registered alpha above 0.05: the headline says "directional", never "significant".
    pub directional: bool,
    pub alpha: f64,
    /// A design's own assumptions, stated every time — a switchback's carryover and pull-forward.
    pub assumptions: Vec<String>,
    /// Set when the target estimate was refused: the page renders this, not numbers.
    pub refusal: Option<String>,
    pub headline: Option<Headline>,
    pub treated_total: Option<TreatedTotal>,
    pub projection: Option<Projection>,
    pub substitution: SubstitutionView,
    pub direction: DirectionView,
    pub promise: Option<PromiseView>,
    pub compliance: ComplianceView,
    pub checks: ChecksView,
    pub proposals: ProposalsView,
    pub ratio: Option<RatioView>,
    pub prediction: Option<PredictionView>,
    /// Why a frozen prior could not be compared (no first stage, refused, aborted).
    pub prediction_note: Option<String>,
    pub write_back: Option<WriteBackView>,
    /// Set when the lever's automation resolved to more than one revision across
    /// the run's flips: what "on" meant changed during the experiment.
    pub lever_note: Option<String>,
    /// `None` when the design reads no carryover window. Defaulted so reports sealed
    /// before it existed still read.
    #[serde(default)]
    pub carryover: Option<CarryoverView>,
    pub seeds: Seeds,
    /// Set when the design carries no `mde_at_proposal` (an observed rollout, or
    /// one pricing could not size): says so, with pricing's reason when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mde_note: Option<String>,
    /// The outcome is a ratio per unit (`outcome.denominator`): its numbers are a
    /// share, not money per unit-day.
    #[serde(default)]
    pub share: bool,
    /// The outcome is read on a proportional scale (`outcome.scale`): the
    /// headline, its interval and each wave's are shares (0.12 = +12%).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proportional: bool,
    /// Each wave's own effect, when the estimate compared more than one. Empty
    /// on a report sealed before it existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waves: Vec<WaveView>,
    /// Set when the treated units were compared with a synthetic control
    /// (`design.comparison`): each treated unit's fit, its donors and its chart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<SyntheticView>,
}

/// How a synthetic control was built, unit by unit, in the headline's scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticView {
    /// "Not randomised — synthetic control": stated on every such report.
    pub label: String,
    /// Units the fits could draw on.
    pub donors: usize,
    /// Placebo sets behind the p-value, and whether they were every one.
    pub placebo_sets: usize,
    pub exact: bool,
    pub fits: Vec<SyntheticFitView>,
}

/// One treated unit against its synthetic control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticFitView {
    pub unit: String,
    /// Its own mean effect per period over the post window.
    pub effect: Option<f64>,
    /// Pre-window tracking error over the unit's own volatility: 0 is perfect,
    /// 1 is no better than a flat line at its mean.
    pub pre_fit: Option<f64>,
    /// How its post/pre error ratio ranks among the donors' placebos, as a p-value.
    pub rank_p: Option<f64>,
    /// Donors with a non-zero weight, largest first.
    pub weights: Vec<DonorWeightView>,
    pub series: Vec<SyntheticPointView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DonorWeightView {
    pub unit: String,
    pub weight: f64,
}

/// One period of the treated-versus-synthetic chart, in the measure's own units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticPointView {
    /// The first day of the period.
    pub date: Option<chrono::NaiveDate>,
    pub actual: Option<f64>,
    pub synthetic: Option<f64>,
}

/// One wave's own effect, in the headline's scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveView {
    /// The first day of the period its units switched in.
    pub switch_on: Option<chrono::NaiveDate>,
    pub n_treated: usize,
    pub n_control: usize,
    pub estimate: Option<f64>,
    pub interval: Interval,
    pub p_value: Option<f64>,
    /// Its interval, at the registered per-comparison level, excludes zero.
    /// Not adjusted for the number of waves.
    pub significant: bool,
    pub refusal: Option<String>,
}

/// Everything `build_report` reads. Plain values: the rules are a pure function.
pub struct ReportInput<'a> {
    pub config: &'a ExperimentConfig,
    pub target: &'a EffectIn,
    pub sibling: Option<&'a EffectIn>,
    pub sibling_power: Option<&'a PowerIn>,
    pub ratio: Option<&'a RatioIn>,
    pub write_back: Option<&'a Result<CoefficientEdit, PatchRefusal>>,
    pub compliance: &'a ComplianceSummary,
    pub proposal_count: usize,
    pub aborted: bool,
    pub retrospective: bool,
    pub abort_reason: Option<&'a str>,
    /// An aborted run's measurement cut: the exclusive end of the history read — the day
    /// the levers went off. `None` when the run read to its registered end.
    pub measured_until: Option<chrono::NaiveDate>,
    /// The abort left the levers as they were (`AbortLevers::Keep`): the cut is the day it
    /// was aborted, not the day the levers went off.
    pub levers_kept: bool,
    pub n_treated: usize,
    pub n_untreated: usize,
    pub distinct_lever_revisions: usize,
    pub switchback: Option<SwitchbackFacts>,
    /// The effect over `design.carryover_days` after the close `off`s, when the design
    /// reads one.
    pub carryover: Option<&'a EffectIn>,
    pub post_days: usize,
    pub seeds: Seeds,
    /// Why no MDE was priced, when pricing ran and refused (its own words).
    pub mde_unavailable: Option<&'a str>,
    /// The analysed split is not one pricing drew: the design's assignment was
    /// edited by hand after a proposal was adopted, or never adopted from one.
    pub hand_assigned: bool,
    /// The synthetic control's fits, when `design.comparison` asked for one.
    pub synthetic: Option<&'a SyntheticIn>,
}
