//! An observed design: a rollout that already happened, declared rather than
//! drawn. No lever, no seed, no MDE. It can be analysed from past data, and
//! never registered.

use oxy_app::server::experiment::register::RegisterError;

use super::fixture::*;

const OBSERVED: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "The promo the stores ran in March lifted beverage revenue per store-day"
owner: owner@acme.test
for_each: store
design:
  timezone: Australia/Sydney
  assignment: observed
  waves:
    - { switch_on: 2026-03-02, units: [bondi, newtown] }
  pre_days: 56
  post_days: 56
  anticipation_days: 3
outcome:
  target: store_days.beverages_revenue
  direction: increase
"#;

#[tokio::test]
async fn an_observed_design_is_analysed_without_a_lever_seed_or_mde() {
    let fx = Fx::seed_with_levers(OBSERVED, &[]).await;
    let run = fx
        .retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("an observed design is read from past data");
    assert!(run.retrospective);
    assert_eq!(run.state, "closing");
}

#[tokio::test]
async fn an_observed_design_is_never_registered() {
    let fx = Fx::seed_with_levers(OBSERVED, &[]).await;
    let Err(RegisterError::Draft(problems)) = fx.register_at(utc("2026-02-10T00:00:00Z")).await
    else {
        panic!("an observed design has nothing to switch");
    };
    assert!(
        problems.iter().any(|p| p.contains("observed")),
        "{problems:?}"
    );
}

#[tokio::test]
async fn the_status_offers_past_data_for_an_observed_design() {
    let fx = Fx::seed_with_levers(OBSERVED, &[]).await;
    let s = fx.status_at(utc("2026-06-01T00:00:00Z")).await;
    assert!(s.retrospective_ready, "{:?}", s.retrospective_refusal);
}
