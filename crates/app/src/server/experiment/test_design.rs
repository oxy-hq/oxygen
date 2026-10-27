//! The designs the host's unit tests reason about (the same shapes as the
//! `oxy-experiment` fixtures), parsed through PR 2's public `parse`.
//!
//! Shared by later tasks' tests; until they land a helper may be unused.

use oxy_experiment::ExperimentConfig;

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
outcome:
  target: store_days.beverages_revenue
  family: 1
  direction: increase
  mde_at_proposal: 17.5
"#;

/// The switchback the host's schedule and status tests reason about: the
/// spec's on, off, off, on in weekly periods.
pub(crate) const SWITCHBACK_YAML: &str = r#"
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

pub(crate) fn design() -> ExperimentConfig {
    oxy_experiment::config::parse(YAML).expect("the host test design parses")
}

pub(crate) fn switchback() -> ExperimentConfig {
    oxy_experiment::config::parse(SWITCHBACK_YAML).expect("the host switchback parses")
}

pub(crate) fn design_with(from: &str, to: &str) -> ExperimentConfig {
    let edited = YAML.replace(from, to);
    assert_ne!(edited, YAML, "design edit `{from}` matched nothing");
    oxy_experiment::config::parse(&edited).expect("the edited design parses")
}
