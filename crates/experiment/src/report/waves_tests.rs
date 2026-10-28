use super::tests::Case;
use super::*;
use crate::config::Scale;
use crate::engine_in::WaveEffectIn;
use crate::test_fixtures::{effect, refused};

fn wave(day: i64, estimate: f64, half_width: f64) -> WaveEffectIn {
    WaveEffectIn {
        switch_day: day,
        n_treated: 3,
        n_control: 40,
        estimate: Some(estimate),
        ci_low: Some(estimate - half_width),
        ci_high: Some(estimate + half_width),
        p_value: Some(0.2),
        refusal: None,
    }
}

/// 2026-05-06 and 2026-05-14 as day ordinals.
const MAY_6: i64 = 739_742;
const MAY_14: i64 = 739_750;

#[test]
fn two_waves_are_listed_with_their_dates_and_own_verdicts() {
    let mut c = Case::new();
    c.target.waves = vec![wave(MAY_6, 11.0, 5.0), wave(MAY_14, 0.5, 4.0)];
    let r = c.build();
    assert_eq!(r.waves.len(), 2);
    assert_eq!(r.waves[0].switch_on.unwrap().to_string(), "2026-05-06");
    assert_eq!(r.waves[1].switch_on.unwrap().to_string(), "2026-05-14");
    assert!(r.waves[0].significant, "6..16 excludes zero");
    assert!(!r.waves[1].significant, "-3.5..4.5 holds zero");
    assert_eq!(r.waves[0].n_control, 40);
}

#[test]
fn one_wave_or_a_refused_estimate_lists_none() {
    let mut c = Case::new();
    c.target.waves = vec![wave(MAY_6, 11.0, 5.0)];
    assert!(
        c.build().waves.is_empty(),
        "one wave is the headline itself"
    );
    c.target = refused("no units");
    c.target.waves = vec![wave(MAY_6, 11.0, 5.0), wave(MAY_14, 0.5, 4.0)];
    assert!(c.build().waves.is_empty());
}

#[test]
fn a_refused_wave_is_listed_with_its_reason_and_never_significant() {
    let mut c = Case::new();
    let mut thin = wave(MAY_14, f64::NAN, 0.0);
    thin.estimate = None;
    thin.ci_low = None;
    thin.ci_high = None;
    thin.refusal = Some("fewer than 2 units in an arm".into());
    c.target.waves = vec![wave(MAY_6, 11.0, 5.0), thin];
    let r = c.build();
    assert!(!r.waves[1].significant);
    assert_eq!(r.waves[1].estimate, None);
    assert!(r.waves[1].refusal.is_some());
}

#[test]
fn a_report_without_waves_serialises_as_before() {
    let v = serde_json::to_value(Case::new().build()).unwrap();
    assert!(v.get("waves").is_none());
    assert!(v.get("proportional").is_none());
}

#[test]
fn a_proportional_outcome_reads_its_numbers_as_shares() {
    let mut c = Case::new();
    c.cfg.outcome.scale = Scale::Proportional;
    // ln(1.5) ± 0.2 on the log scale.
    let log = 1.5_f64.ln();
    c.target = effect(log, 0.2);
    c.target.waves = vec![wave(MAY_6, log, 0.2), wave(MAY_14, 0.0, 0.1)];
    let r = c.build();
    assert!(r.proportional);
    let h = r.headline.expect("a headline");
    assert!((h.estimate - 0.5).abs() < 1e-12, "{}", h.estimate);
    assert!((h.interval.low.unwrap() - (log - 0.2).exp_m1()).abs() < 1e-12);
    assert_eq!(h.half_width, None, "a share's interval is not symmetric");
    assert!(h.text.contains("+50.0%"), "{}", h.text);
    assert!(h.text.contains("+22.8% to +83.2%"), "{}", h.text);
    assert_eq!(r.estimand, PROPORTIONAL_ESTIMAND);
    assert!(r.treated_total.is_none() && r.projection.is_none());
    assert!(r.promise.is_none());
    assert!((r.waves[0].estimate.unwrap() - 0.5).abs() < 1e-12);
    assert_eq!(r.waves[1].estimate, Some(0.0));
}
