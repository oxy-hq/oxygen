//! Shared unit-test data: a registered single-wave design and the view it writes back to.

use oxy_experiment::config::ExperimentConfig;

pub const CONFIG_YAML: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  read_back: { when_on: "{{store_days.promo_active}} > 0", when_off: "{{store_days.promo_active}} == 0", grace_days: 2 }
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [s01, s02, s03] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
outcome:
  target: store_days.beverages_revenue
  driver: store_days.promo_redemptions
  direction: increase
  mde_at_proposal: 17.5
substitution_check: { sibling: store_days.entree_revenue }
"#;

pub const VIEW: &str = "\
name: store_days
measures:
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
    drivers:
      - measure: store_days.promo_redemptions
        direction: positive
";

/// The host's switchback (`test_design::switchback`: on, off, off, on in weekly periods, alpha 0.2,
/// no windows) with a one-day washout.
pub fn switchback_config() -> ExperimentConfig {
    let mut cfg = crate::server::experiment::test_design::switchback();
    cfg.design.washout_days = 1;
    cfg
}
