//! The spec's example, parsed once for every module's tests.
//!
//! This is the one fixture module for the crate (C2): PR 4 extends it with its
//! own fixtures rather than adding another. It is declared `#[cfg(test)] mod
//! test_fixtures;` and its items are `pub(crate)`, which is all PR 4 needs, since
//! `run.rs` and `compliance.rs` are in this same crate (`oxy-experiment`); widen
//! to `pub` only if a test outside the crate ever needs them.

use crate::compliance::ComplianceSummary;
use crate::config::{ExperimentConfig, LeverState, PeriodConfig, SwitchbackConfig, parse};
use crate::engine_in::{EffectIn, PowerIn, RatioIn, Refused};
use crate::validate::{ConfigError, validate_static};
use chrono::NaiveDate;

/// `internal-docs/experiments.md` "The artifact", verbatim — including the
/// keys the amended spec added (`lever.per`, `design.strata`,
/// `outcome.prior_coefficient`).
pub(crate) const FULL_YAML: &str = r##"experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store                        # a World Model entity; its key is the unit

lever:
  on:  { via: promo_on }               # automation, referenced by name
  off: { via: promo_off }
  per: unit                            # or wave: one run per wave, all its units
  read_back:
    when_on:  "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off                        # or keep

design:
  timezone: Australia/Sydney           # IANA; the calendar every date here is in
  time_dimension: store_days.business_date   # optional; required when ambiguous
  waves:                               # platform-generated, human-committed
    - { switch_on: 2026-03-02, units: [bondi, newtown] }
    - { switch_on: 2026-03-16, units: [manly] }
  strata:                              # platform-generated: units blocked by baseline volume
    - [bondi, manly, parramatta, chatswood]
    - [newtown, glebe, surry_hills, balmain]
  seed: 8841207                        # reproduces the waves exactly, given the strata
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
  coverage_floor: 0.9
  compliance_floor: 0.8

outcome:
  target: store_days.beverages_revenue
  driver: store_days.promo_redemptions # optional: what the lever moves
  family: 1
  direction: increase
  alpha: 0.05
  power: 0.80
  mde_at_proposal: 17.5                # $/store-day, frozen at pricing
  prior_coefficient: { value: 4.2, source: fitted }   # driver edge's slope at pricing; omitted = none

substitution_check: { sibling: store_days.entree_revenue }

guardrails:
  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }
    action: abort
  - criteria: { type: trigger, when: "{{store_days.labor_cost_pct}} > 0.34" }
    action: abort
"##;

/// The same design as a draft: no `waves`, no `strata`, no `seed`, no
/// `mde_at_proposal`, no `prior_coefficient`, no `time_dimension`, and no
/// `per` or `on_close` (so their defaults are observable).
pub(crate) const DRAFT_YAML: &str = r##"experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store

lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  read_back:
    when_on:  "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2

design:
  timezone: Australia/Sydney
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
  coverage_floor: 0.9
  compliance_floor: 0.8

outcome:
  target: store_days.beverages_revenue
  driver: store_days.promo_redemptions
  family: 1
  direction: increase
  alpha: 0.05
  power: 0.80

substitution_check: { sibling: store_days.entree_revenue }

guardrails:
  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }
    action: abort
"##;

/// A registrable switchback: two pairs of seven-day periods, `per: wave` (a
/// period is a wave of the whole fleet), no `waves`, `strata`, `assign_by`,
/// `pre_days`, `post_days` or `anticipation_days` — none of which means anything
/// to a switchback. A directional `alpha`, since two pairs cannot reach 0.05.
pub(crate) const SWITCHBACK_YAML: &str = r##"experiment: promo_switchback
hypothesis: "Alternating the promo fleet-wide on and off lifts beverage revenue per store-day"
owner: ops
for_each: store

lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back:
    when_on:  "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2

design:
  timezone: Australia/Sydney
  switchback:
    period_days: 7
    periods:
      - { from: 2026-03-02, state: on }
      - { from: 2026-03-09, state: off }
      - { from: 2026-03-16, state: off }
      - { from: 2026-03-23, state: on }
  seed: 8841207
  washout_days: 2

outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
  mde_at_proposal: 12.5
"##;

/// The same switchback before pricing: the period length is the owner's input,
/// the periods, `seed` and `mde_at_proposal` are pricing's output.
pub(crate) const SWITCHBACK_DRAFT_YAML: &str = r##"experiment: promo_switchback
hypothesis: "Alternating the promo fleet-wide on and off lifts beverage revenue per store-day"
owner: ops
for_each: store

lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back:
    when_on:  "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2

design:
  timezone: Australia/Sydney
  switchback: { period_days: 7 }
  washout_days: 2

outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
"##;

pub(crate) fn full() -> ExperimentConfig {
    parse(FULL_YAML).expect("the spec's example parses")
}

pub(crate) fn draft() -> ExperimentConfig {
    parse(DRAFT_YAML).expect("a draft parses")
}

pub(crate) fn switchback() -> ExperimentConfig {
    parse(SWITCHBACK_YAML).expect("the switchback example parses")
}

pub(crate) fn switchback_draft() -> ExperimentConfig {
    parse(SWITCHBACK_DRAFT_YAML).expect("a switchback draft parses")
}

pub(crate) fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

pub(crate) fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The spec's waves example, edited.
pub(crate) fn edited(f: impl FnOnce(&mut ExperimentConfig)) -> ExperimentConfig {
    let mut c = full();
    f(&mut c);
    c
}

/// The one static refusal `c` earns; fails if there are none or several.
pub(crate) fn only(c: &ExperimentConfig) -> ConfigError {
    let mut errs = validate_static(c);
    assert_eq!(errs.len(), 1, "expected exactly one refusal, got {errs:?}");
    errs.remove(0)
}

// Appended by PR 4. `YAML`: four treated units in two waves, so compliance
// ratios like 3/4 are expressible. `SWITCHBACK`: the spec's own schedule —
// on, off, off, on — in weekly periods.

pub(crate) const YAML: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  read_back:
    when_on: "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [bondi, newtown] }
    - { switch_on: 2026-03-16, units: [manly, surry_hills] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
  coverage_floor: 0.9
  compliance_floor: 0.8
outcome:
  target: store_days.beverages_revenue
  family: 1
  direction: increase
  alpha: 0.05
  power: 0.80
  mde_at_proposal: 17.5
guardrails:
  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }
    action: abort
  - criteria: { type: trigger, when: "{{store_days.labor_cost_pct}} > 0.34" }
    action: abort
"#;

pub(crate) const SWITCHBACK: &str = r#"
experiment: beverage_promo_switchback
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back:
    when_on: "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  switchback:
    period_days: 7
    periods:
      - { from: 2026-03-02, state: on }
      - { from: 2026-03-09, state: off }
      - { from: 2026-03-16, state: off }
      - { from: 2026-03-23, state: on }
  seed: 8841207
  washout_days: 2
outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
  mde_at_proposal: 9.0
"#;

pub(crate) fn cfg() -> ExperimentConfig {
    parse(YAML).expect("the fixture design parses")
}

pub(crate) fn sb_cfg() -> ExperimentConfig {
    parse(SWITCHBACK).expect("the switchback fixture parses")
}

/// The fixture with one textual edit. Panics if `from` matches nothing, so a
/// fixture drift cannot silently turn a test into a copy of another.
pub(crate) fn cfg_with(from: &str, to: &str) -> ExperimentConfig {
    let edited = YAML.replace(from, to);
    assert_ne!(edited, YAML, "fixture edit `{from}` matched nothing");
    parse(&edited).expect("the edited fixture parses")
}

pub(crate) fn d(s: &str) -> NaiveDate {
    s.parse().expect("a YYYY-MM-DD date")
}

// --- PR 5: report builders ---

/// The spec's registered example (`internal-docs/experiments.md`, "The artifact").
pub(crate) const REGISTERED_YAML: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  read_back:
    when_on:  "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [bondi, newtown] }
    - { switch_on: 2026-03-16, units: [manly] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
  coverage_floor: 0.9
  compliance_floor: 0.8
outcome:
  target: store_days.beverages_revenue
  driver: store_days.promo_redemptions
  family: 1
  direction: increase
  alpha: 0.05
  power: 0.80
  mde_at_proposal: 17.5
substitution_check: { sibling: store_days.entree_revenue }
guardrails:
  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }
    action: abort
"#;

pub(crate) fn config() -> ExperimentConfig {
    parse(REGISTERED_YAML).expect("the spec's registered example must parse")
}

/// A finite, non-refused effect with a symmetric interval.
pub(crate) fn effect(estimate: f64, half_width: f64) -> EffectIn {
    EffectIn {
        estimate: Some(estimate),
        ci_low: Some(estimate - half_width),
        ci_high: Some(estimate + half_width),
        p_value: Some(0.01),
        n_treated: 6,
        n_control: 6,
        significant: half_width < estimate.abs(),
        ..Default::default()
    }
}

pub(crate) fn refused(reason: &str) -> EffectIn {
    EffectIn::refused(reason.to_string())
}

pub(crate) fn power(mde: f64) -> PowerIn {
    PowerIn {
        mde: Some(mde),
        refusal: None,
    }
}

/// A Wald ratio whose first stage is strong (5.0 ± 1.0).
pub(crate) fn ratio(coefficient: f64) -> RatioIn {
    RatioIn {
        coefficient: Some(coefficient),
        ci_low: Some(coefficient - 1.0),
        ci_high: Some(coefficient + 1.0),
        first_stage: effect(5.0, 1.0),
        refusal: None,
    }
}

/// `ratio(coefficient)` with an Anderson–Rubin set unbounded on the `None` side(s), as the
/// engine emits it: a finite coefficient and first stage, and the unbounded side named in
/// `refusal` (airlayer `estimate_ratio` sets one whenever an endpoint is infinite).
pub(crate) fn unbounded_ratio(coefficient: f64, low: Option<f64>, high: Option<f64>) -> RatioIn {
    let side = match (low, high) {
        (None, None) => "on both sides",
        (None, Some(_)) => "below",
        _ => "above",
    };
    RatioIn {
        ci_low: low,
        ci_high: high,
        refusal: Some(format!(
            "the Anderson-Rubin set is unbounded {side} at the registered alpha 0.05 and family 1"
        )),
        ..ratio(coefficient)
    }
}

pub(crate) fn compliance(
    c_treated: f64,
    c_control_leak: f64,
    restated_mde: Option<f64>,
    below_floor: bool,
) -> ComplianceSummary {
    // Measured by default (6 treated, 6 control); a test for "not measured" sets `n_treated: 0` itself.
    ComplianceSummary {
        c_treated,
        c_control_leak,
        restated_mde,
        below_floor,
        n_treated: 6,
        n_control: 6,
    }
}

/// The registered example, randomised by group of stores.
pub(crate) fn grouped_config() -> ExperimentConfig {
    let mut c = config();
    c.assign_by = Some("store.region".to_string());
    c
}

/// The registered example as a four-period (two-pair) switchback at registered alpha 0.2:
/// no waves, no windows (`pre_days` / `post_days` are `None`), a one-day washout.
pub(crate) fn switchback_config() -> ExperimentConfig {
    let mut c = config();
    c.design.waves.clear();
    c.design.pre_days = None;
    c.design.post_days = None;
    c.design.anticipation_days = 0;
    c.design.washout_days = 1;
    let period = |day: u32, state| PeriodConfig {
        from: NaiveDate::from_ymd_opt(2026, 3, day).unwrap().into(),
        state,
    };
    c.design.switchback = Some(SwitchbackConfig {
        period_days: 7,
        periods: vec![
            period(2, LeverState::On),
            period(9, LeverState::Off),
            period(16, LeverState::Off),
            period(23, LeverState::On),
        ],
    });
    c.outcome.alpha = 0.2;
    c
}

/// A report built by the real rules from a target effect, an optional ratio and a config.
pub(crate) fn built_report(
    cfg: &ExperimentConfig,
    target: &EffectIn,
    ratio: Option<&RatioIn>,
) -> crate::report::ReportView {
    use crate::report::{ReportInput, Seeds, build_report};
    let c = compliance(0.95, 0.0, None, false);
    build_report(ReportInput {
        config: cfg,
        target,
        sibling: None,
        sibling_power: None,
        ratio,
        write_back: None,
        compliance: &c,
        proposal_count: 1,
        aborted: false,
        retrospective: false,
        abort_reason: None,
        measured_until: None,
        levers_kept: false,
        n_treated: 6,
        n_untreated: 6,
        distinct_lever_revisions: 1,
        switchback: None,
        carryover: None,
        post_days: 56,
        seeds: Seeds {
            effect: 1,
            placebo: 2,
            ratio: 3,
        },
        mde_unavailable: None,
        hand_assigned: false,
        synthetic: None,
    })
}

/// A past rollout analysed after the fact: the waves are what happened, not a
/// draw. No lever (nothing will ever be switched), no `seed`, no
/// `mde_at_proposal`.
pub(crate) const OBSERVED_YAML: &str = r##"experiment: listing_backup_rollout
hypothesis: "Backup listings lower revenue lost to stockouts per listing-day"
owner: ops
for_each: listing

design:
  timezone: America/Vancouver
  assignment: observed
  waves:
    - { switch_on: 2026-03-04, units: [a1, a2, a3] }
  pre_days: 28
  post_days: 7
  anticipation_days: 1

outcome:
  target: listing_days.lost_revenue
  direction: decrease
"##;

pub(crate) fn observed() -> ExperimentConfig {
    parse(OBSERVED_YAML).expect("an observed design parses")
}
