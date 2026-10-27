//! Experiment files for this module's unit tests.
//!
//! One head and one tail with the design in between, so a test that wants a
//! different `washout_days` says so and nothing else. The numbers are
//! deliberately not the defaults (`coverage_floor 0.85`, `alpha 0.1`,
//! `family 2`) so a test that says "every field is carried" can tell a carried
//! value from a defaulted one.

use oxy_experiment::{ExperimentConfig, parse};

const HEAD: &str = r##"experiment: promo_pilot
hypothesis: "A promo lifts beverage revenue per store-day"
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
"##;

const TAIL: &str = r##"
outcome:
  target: store_days.beverages_revenue
  direction: increase
  family: 2
  alpha: 0.1
"##;

/// `design.waves` + `design.seed`, as pricing would have written them.
pub(super) const TWO_WAVES: &str = "  waves:\n    - { switch_on: 2026-03-02, units: [bondi, newtown] }\n    - { switch_on: 2026-03-16, units: [manly] }\n  seed: 8841207\n";

/// `design.switchback.periods`: two consecutive pairs, `on, off` then `off, on`.
pub(super) const FOUR_PERIODS: &str = "    periods:\n      - { from: 2026-03-02, state: on }\n      - { from: 2026-03-09, state: off }\n      - { from: 2026-03-16, state: off }\n      - { from: 2026-03-23, state: on }\n";

/// `design.strata`, as pricing would have written it.
pub(super) const STRATA: &str = "  strata:\n    - [bondi, manly]\n    - [newtown, glebe]\n";

/// `outcome.mde_at_proposal`, as pricing would have written it.
pub(super) const MDE: &str = "  mde_at_proposal: 17.5\n";

/// A waves file. `waves` is whatever sits at two-space indent before the
/// windows (`design.waves`, `design.seed`, `design.strata`, or nothing).
pub(super) fn yaml(waves: &str, mde: &str, pre: u32, post: u32, washout: u32) -> String {
    format!(
        "{HEAD}{waves}  pre_days: {pre}\n  post_days: {post}\n  anticipation_days: 3\n  washout_days: {washout}\n  coverage_floor: 0.85\n{TAIL}{mde}"
    )
}

/// A switchback file: no windows, no anticipation — a switchback refuses them.
/// `seed` sits at two-space indent; `periods` at four (empty = a draft).
pub(super) fn switchback_yaml(seed: &str, periods: &str, mde: &str, washout: u32) -> String {
    format!(
        "{HEAD}{seed}  switchback:\n    period_days: 7\n{periods}  washout_days: {washout}\n  coverage_floor: 0.85\n{TAIL}{mde}"
    )
}

/// `assign_by: <by>` under `for_each`.
pub(super) fn with_assign_by(yaml: &str, by: &str) -> String {
    yaml.replacen(
        "for_each: store\n",
        &format!("for_each: store\nassign_by: {by}\n"),
        1,
    )
}

/// A waves draft: no waves, no seed, no `mde_at_proposal` — what pricing is asked about.
pub(super) fn draft(washout_days: u32) -> ExperimentConfig {
    parse(&yaml("", "", 28, 28, washout_days)).expect("the draft parses")
}

/// A file carrying `waves` (and the seed) and an MDE.
pub(super) fn registered(waves: &str, washout_days: u32) -> ExperimentConfig {
    parse(&yaml(waves, MDE, 28, 28, washout_days)).expect("the registered file parses")
}

/// A switchback draft: `period_days: 7`, no periods, no seed, no MDE.
pub(super) fn switchback_draft(washout_days: u32) -> ExperimentConfig {
    parse(&switchback_yaml("", "", "", washout_days)).expect("the switchback draft parses")
}

/// A `guardrails:` block appended to a file; `items` sit at two-space indent.
pub(super) fn with_guardrails(yaml: &str, items: &str) -> String {
    format!("{yaml}\nguardrails:\n{items}")
}

/// An anomaly guardrail and a trigger guardrail, on different measures.
pub(super) const ANOMALY_AND_TRIGGER: &str = "  - criteria: { type: anomaly, measure: store_days.refund_rate, significance: 0.95 }\n    action: abort\n  - criteria: { type: trigger, when: \"{{store_days.labor_pct}} > 0.34\" }\n    action: abort\n";
