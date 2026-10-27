//! The experiment design schema, exactly as `internal-docs/experiments.md` "The
//! artifact" writes it.
//!
//! `#[serde(deny_unknown_fields)]` on every struct: a typo is an error, never a
//! default. That matters most on the optional keys — without it `sead: 1` would
//! quietly turn a registrable design into a draft.
//!
//! Serialize is derived too: a design version stores this type as JSON and
//! every reader decodes it with `serde_json::from_value`.

use crate::grain::Grain;
use crate::moment::Moment;
use crate::validate::ConfigError;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentConfig {
    pub experiment: String,
    pub hypothesis: String,
    pub owner: String,
    pub for_each: String,
    /// A dimension of the `for_each` entity: its values, not single units, are
    /// randomised. Platform-written like `waves`; `None` = single units.
    pub assign_by: Option<String>,
    /// What switches the change on and off. Required to register; a design that
    /// is only ever analysed after the fact (`design.assignment: observed`, or a
    /// retrospective) switches nothing and may leave it out. Skipped when absent,
    /// so a design that has one serializes, and hashes, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lever: Option<LeverConfig>,
    pub design: DesignConfig,
    pub outcome: OutcomeConfig,
    pub substitution_check: Option<SubstitutionCheck>,
    #[serde(default)]
    pub guardrails: Vec<Guardrail>,
}

impl ExperimentConfig {
    /// What the levers do at close: `off` when there is no lever, since then
    /// nothing was ever switched on.
    pub fn on_close(&self) -> OnClose {
        self.lever.as_ref().map_or(OnClose::Off, |l| l.on_close)
    }

    /// What a flip is checked against; `None` with no lever, which flips nothing.
    pub fn read_back(&self) -> Option<&ReadBack> {
        self.lever.as_ref().map(|l| &l.read_back)
    }

    /// How flips are batched; `unit` with no lever, which flips nothing.
    pub fn lever_per(&self) -> LeverPer {
        self.lever.as_ref().map_or(LeverPer::Unit, |l| l.per)
    }

    /// The read-back grace in days; 0 with no lever, which reads nothing back.
    pub fn grace_days(&self) -> u32 {
        self.lever.as_ref().map_or(0, |l| l.read_back.grace_days)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeverConfig {
    /// YAML 1.1 readers (PyYAML among them) read an unquoted `on:` key as the
    /// boolean `true` and `off:` as `false`, and the JSON they write then names
    /// the keys "true" and "false"; both are taken for what was meant.
    #[serde(alias = "true")]
    pub on: ViaRef,
    #[serde(alias = "false")]
    pub off: ViaRef,
    pub read_back: ReadBack,
    #[serde(default)]
    pub on_close: OnClose,
    /// `unit` (default): one automation run per unit per flip. `wave`: one run
    /// per wave and state carrying the wave's units (a rate-limited vendor API).
    #[serde(default)]
    pub per: LeverPer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeverPer {
    #[default]
    Unit,
    Wave,
}

/// A lever's two positions. Lives here (not in the run state machine) because a
/// switchback period names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeverState {
    On,
    Off,
}

/// An automation, by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViaRef {
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadBack {
    pub when_on: String,
    pub when_off: String,
    pub grace_days: u32,
}

/// Whether the change stays live after the run reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnClose {
    #[default]
    Off,
    Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignConfig {
    /// IANA zone; every date and window boundary in the file is a calendar date here.
    pub timezone: Tz,
    /// The date dimension the panel's calendar runs on. Optional here: PR 3's
    /// host refuses when it is absent and the target's view has several date
    /// dimensions (that needs the semantic layer, which this crate never sees).
    pub time_dimension: Option<String>,
    /// The period one panel cell covers; every `_days` count below and
    /// `switchback.period_days` count periods of it, and every date names a
    /// period's first day. Skipped when `day`, so a day-grain design
    /// serializes, and hashes, as before.
    #[serde(default, skip_serializing_if = "Grain::is_day")]
    pub grain: Grain,
    /// Who chose the treated units: pricing's random draw (the default), or the
    /// world (`observed`), a rollout that already happened and is declared, not
    /// drawn. Skipped when randomised, so such a design serializes, and hashes,
    /// as before.
    #[serde(default, skip_serializing_if = "Assignment::is_randomised")]
    pub assignment: Assignment,
    /// Empty (and no `switchback.periods`) = draft.
    #[serde(default)]
    pub waves: Vec<WaveConfig>,
    /// The fleet alternates on and off instead of waves. Exclusive with `waves`.
    pub switchback: Option<SwitchbackConfig>,
    /// Units (or groups) blocked by baseline volume, from pricing. A waves
    /// design only; `None` = unblocked.
    pub strata: Option<Vec<Vec<String>>>,
    /// `None` = draft.
    pub seed: Option<u64>,
    /// Required for a waves design, refused on a switchback — `validate_static`
    /// owns both, because the type cannot say "required unless".
    pub pre_days: Option<u32>,
    pub post_days: Option<u32>,
    #[serde(default)]
    pub anticipation_days: u32,
    #[serde(default)]
    pub washout_days: u32,
    /// Days to keep reading after the levers turn off at close: the effect that
    /// lingers (an ad seen this week, bought next). `None` = none read. A waves
    /// design with one switch date and `on_close: off` only; skipped when absent,
    /// so a design without it serializes, and hashes, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carryover_days: Option<u32>,
    #[serde(default = "d_coverage")]
    pub coverage_floor: f64,
    #[serde(default = "d_compliance")]
    pub compliance_floor: f64,
}

/// How the waves came to be.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assignment {
    /// Drawn by pricing from `seed`: the split is random, so a randomisation
    /// test stands and the design can be registered.
    #[default]
    Randomised,
    /// Declared by the author: the units that were in fact changed, on the dates
    /// they were changed. Nothing was drawn, so it can only be analysed after the
    /// fact, and its report says the comparison is observational.
    Observed,
}

impl Assignment {
    pub fn is_randomised(&self) -> bool {
        matches!(self, Assignment::Randomised)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveConfig {
    /// A date (its midnight), or at hour grain a local whole hour (`2026-10-26T14:00`).
    pub switch_on: Moment,
    /// Unit names, matched by exact string equality — never by position.
    pub units: Vec<String>,
}

/// A paired switchback: consecutive `period_days` periods, in pairs of one on and
/// one off. `periods` empty = the draft, the owner's `period_days` awaiting
/// pricing's schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchbackConfig {
    pub period_days: u32,
    #[serde(default)]
    pub periods: Vec<PeriodConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodConfig {
    /// As a wave's `switch_on`: a date, or at hour grain a local whole hour.
    pub from: Moment,
    pub state: LeverState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeConfig {
    pub target: String,
    /// Narrows `target` to some rows of its view (one product's sales), so a
    /// design needs no hand-made measure per filter. Empty = the whole measure.
    /// Skipped when empty, so a design without one serializes, and hashes, as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_filter: Vec<MeasureFilter>,
    /// Makes the outcome a ratio per unit, `Σ target / Σ denominator` over each
    /// window (a share such as merchant-fulfilled units over all units). Read
    /// from past data only; skipped when absent, so a design without one
    /// serializes, and hashes, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<String>,
    pub driver: Option<String>,
    /// Narrows `driver`, like `target_filter`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub driver_filter: Vec<MeasureFilter>,
    #[serde(default = "one")]
    pub family: u32,
    pub direction: Direction,
    #[serde(default = "d_alpha")]
    pub alpha: f64,
    #[serde(default = "d_power")]
    pub power: f64,
    /// `None` = draft.
    pub mde_at_proposal: Option<f64>,
    /// The slope the World Model held on the `driver` → `target` edge at
    /// pricing. Platform-written; `None` = no slope existed.
    pub prior_coefficient: Option<PriorCoefficient>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorCoefficient {
    pub value: f64,
    pub source: PriorSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorSource {
    Declared,
    Fitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Increase,
    Decrease,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstitutionCheck {
    pub sibling: String,
    /// Narrows `sibling`, like `outcome.target_filter` (the other products).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter: Vec<MeasureFilter>,
}

/// Keeps the rows of a measure's view whose `dimension` is (`equals`) or is not
/// (`not_equals`) one of `values`. Several filters on one measure all apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureFilter {
    /// `<view>.<dimension>`, of the filtered measure's own view.
    pub dimension: String,
    #[serde(default)]
    pub op: FilterOp,
    pub values: Vec<String>,
}

/// `measure (dim = a, other ≠ b)`, the measure as a report names it; just the
/// measure when nothing narrows it. A dimension is named without its view.
pub fn measure_label(measure: &str, filters: &[MeasureFilter]) -> String {
    if filters.is_empty() {
        return measure.to_string();
    }
    let clauses: Vec<String> = filters
        .iter()
        .map(|f| {
            let dim = f
                .dimension
                .split_once('.')
                .map_or(f.dimension.as_str(), |(_, d)| d);
            let op = match f.op {
                FilterOp::Equals => "=",
                FilterOp::NotEquals => "≠",
            };
            format!("{dim} {op} {}", f.values.join(" or "))
        })
        .collect();
    format!("{measure} ({})", clauses.join(", "))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    #[default]
    Equals,
    NotEquals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guardrail {
    pub criteria: GuardrailCriteria,
    pub action: GuardrailAction,
}

/// `anomaly` and `trigger` only — `opportunity` is meaningless as a stop, so an
/// `opportunity` criterion is a parse error, not a variant to ignore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuardrailCriteria {
    Anomaly { measure: String, significance: f64 },
    Trigger { when: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailAction {
    Abort,
}

fn d_coverage() -> f64 {
    0.9
}
fn d_compliance() -> f64 {
    0.8
}
fn one() -> u32 {
    1
}
fn d_alpha() -> f64 {
    0.05
}
fn d_power() -> f64 {
    0.80
}

/// Parse a design written as YAML. Strict: an unknown key, a missing required key or
/// a wrongly-typed value is an error. Parsing does not judge the design —
/// that is [`crate::validate_static`].
pub fn parse(yaml: &str) -> Result<ExperimentConfig, ConfigError> {
    serde_yaml::from_str(yaml).map_err(|e| ConfigError::Parse {
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests;
