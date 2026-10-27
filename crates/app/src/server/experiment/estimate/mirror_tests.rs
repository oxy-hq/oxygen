use oxy_airlayer_compat::engine::experiment::{
    EffectResult, PowerResult, PreTrend, RatioResult, SizeBias,
};
use oxy_experiment::engine_in::{EffectIn, PowerIn, PreTrendIn, RatioIn, SizeBiasIn};
use serde::Serialize;
use serde_json::Value;

use super::mirror::{Judged, name_the_driver};

fn engine_effect() -> EffectResult {
    EffectResult {
        estimate: 12.5,
        se: 1.0,
        t_stat: 12.5,
        df: 9.0,
        ci_low: f64::NEG_INFINITY,
        ci_high: f64::INFINITY,
        p_value: 0.001,
        n_treated: 3,
        n_control: 9,
        significant: true,
        estimand: "ATT per unit-day",
        design: "common switch date",
        dropped_waves: vec!["2026-03-16 (1 unit): fewer than 2 units".into()],
        pre_trend: Some(PreTrend {
            estimate: 0.4,
            t_stat: 0.9,
            diverged: false,
        }),
        size_bias: Some(SizeBias {
            correlation_treated: 0.7,
            correlation_control: 0.1,
            z_stat: 2.5,
            significant: true,
            size_window: (1, 2),
            n_treated: 3,
            n_control: 9,
        }),
        retained_on_days: None,
        dropped_units: Vec::new(),
        refusal: None,
    }
}

#[test]
fn the_mirror_reads_the_engines_serialised_field_names() {
    let j = Judged::<EffectIn>::from_engine(&engine_effect());
    assert_eq!(j.view.estimate, Some(12.5));
    assert_eq!((j.view.n_treated, j.view.n_control), (3, 9));
    assert!(j.view.significant);
    assert_eq!(j.view.dropped_waves.len(), 1);
    assert!(!j.view.pre_trend.as_ref().unwrap().diverged);
    assert!(j.view.size_bias.as_ref().unwrap().significant);
    assert_eq!(
        j.raw["se"], 1.0,
        "the raw JSON keeps every engine field, for audit"
    );
}

/// Every key the mirror declares must be a key the engine serialises: a renamed or
/// removed engine field would otherwise read back as the mirror's default and pass.
fn assert_mirror_keys_in_engine<M: Default + Serialize>(engine: &Value, what: &str) {
    let mirror = serde_json::to_value(M::default()).unwrap();
    for key in mirror.as_object().unwrap().keys() {
        assert!(
            engine.get(key).is_some(),
            "`{what}` mirror declares `{key}`, which the engine does not serialise: {engine}"
        );
    }
}

fn engine_power() -> PowerResult {
    PowerResult {
        mde: 86.0,
        mde_relative: 0.07,
        baseline: 1136.0,
        null_sd: 30.0,
        iterations: 400,
        distinct_windows: 4,
        independent_stretches: 3,
        refusal: None,
    }
}

fn engine_ratio() -> RatioResult {
    RatioResult {
        coefficient: 5.0,
        ci_low: 4.0,
        ci_high: 6.0,
        first_stage: engine_effect(),
        target_effect: engine_effect(),
        refusal: None,
    }
}

#[test]
fn every_effect_mirror_key_is_an_engine_key() {
    let e = serde_json::to_value(engine_effect()).unwrap();
    assert_mirror_keys_in_engine::<EffectIn>(&e, "EffectResult");
    assert_mirror_keys_in_engine::<PreTrendIn>(&e["pre_trend"], "PreTrend");
    assert_mirror_keys_in_engine::<SizeBiasIn>(&e["size_bias"], "SizeBias");
}

#[test]
fn every_power_mirror_key_is_an_engine_key() {
    let p = serde_json::to_value(engine_power()).unwrap();
    assert_mirror_keys_in_engine::<PowerIn>(&p, "PowerResult");
}

#[test]
fn every_ratio_mirror_key_is_an_engine_key() {
    let r = serde_json::to_value(engine_ratio()).unwrap();
    assert_mirror_keys_in_engine::<RatioIn>(&r, "RatioResult");
    assert_mirror_keys_in_engine::<EffectIn>(&r["first_stage"], "RatioResult.first_stage");
    assert_mirror_keys_in_engine::<EffectIn>(&r["target_effect"], "RatioResult.target_effect");
}

#[test]
fn the_mirror_carries_every_effect_value_through() {
    let v = Judged::<EffectIn>::from_engine(&engine_effect()).view;
    assert_eq!(v.p_value, Some(0.001));
    let pt = v.pre_trend.unwrap();
    assert_eq!((pt.estimate, pt.t_stat), (Some(0.4), Some(0.9)));
    let sb = v.size_bias.unwrap();
    assert_eq!(
        (sb.correlation_treated, sb.correlation_control),
        (Some(0.7), Some(0.1))
    );
}

#[test]
fn an_infinite_bound_reads_back_as_none_never_as_zero() {
    let j = Judged::<EffectIn>::from_engine(&engine_effect());
    assert_eq!((j.view.ci_low, j.view.ci_high), (None, None));
    assert!(j.raw["ci_low"].is_null() && j.raw["ci_high"].is_null());
}

#[test]
fn a_nan_estimate_reads_back_as_none() {
    let e = EffectResult {
        estimate: f64::NAN,
        ..engine_effect()
    };
    assert_eq!(Judged::<EffectIn>::from_engine(&e).view.estimate, None);
}

#[test]
fn a_switchback_result_reads_with_retained_pairs_and_pair_names() {
    let e = EffectResult {
        n_treated: 3,
        n_control: 3,
        design: "switchback",
        dropped_waves: vec!["pair 2: 3 of 7 days covered".into()],
        pre_trend: None,
        size_bias: None,
        retained_on_days: Some(15), // 3 pairs × 6 settled days, less 3 days dropped for missing data
        ..engine_effect()
    };
    let j = Judged::<EffectIn>::from_engine(&e);
    assert_eq!(
        j.view.retained_on_days,
        Some(15),
        "the engine's own count, not pairs × settled days"
    );
    assert_eq!(
        (j.view.n_treated, j.view.n_control),
        (3, 3),
        "retained pairs, not units"
    );
    assert_eq!(
        j.view.dropped_waves,
        vec!["pair 2: 3 of 7 days covered".to_string()]
    );
    assert!(j.view.pre_trend.is_none() && j.view.size_bias.is_none());
}

#[test]
fn power_and_ratio_mirrors_read_their_fields() {
    let p = PowerResult {
        mde: 86.0,
        mde_relative: 0.07,
        baseline: 1136.0,
        null_sd: 30.0,
        iterations: 400,
        distinct_windows: 4,
        independent_stretches: 3,
        refusal: None,
    };
    assert_eq!(Judged::<PowerIn>::from_engine(&p).view.mde, Some(86.0));
    let r = RatioResult {
        coefficient: 5.0,
        ci_low: 4.0,
        ci_high: f64::INFINITY,
        first_stage: engine_effect(),
        target_effect: engine_effect(),
        refusal: None,
    };
    let v = Judged::<RatioIn>::from_engine(&r).view;
    assert_eq!(
        (v.coefficient, v.ci_low, v.ci_high),
        (Some(5.0), Some(4.0), None)
    );
    assert!(v.first_stage.significant);
}

fn refused_ratio(msg: &str, first_stage_msg: Option<&str>) -> RatioResult {
    RatioResult {
        coefficient: f64::NAN,
        ci_low: f64::NEG_INFINITY,
        ci_high: f64::INFINITY,
        first_stage: EffectResult {
            refusal: first_stage_msg.map(str::to_string),
            ..engine_effect()
        },
        target_effect: engine_effect(),
        refusal: Some(msg.to_string()),
    }
}

#[test]
fn a_ratio_refusal_that_says_the_driver_names_the_driver() {
    // `estimate_ratio` does not know measure names (C2b); the host does.
    let r = refused_ratio(
        "the driver panel has no variance",
        Some("the driver never moved in any unit"),
    );
    let j = name_the_driver(
        Judged::<RatioIn>::from_engine(&r),
        "store_days.promo_redemptions",
    );
    assert_eq!(
        j.view.refusal.as_deref(),
        Some("the driver `store_days.promo_redemptions` panel has no variance")
    );
    assert_eq!(
        j.view.first_stage.refusal.as_deref(),
        Some("the driver `store_days.promo_redemptions` never moved in any unit")
    );
    assert_eq!(
        j.raw["refusal"],
        "the driver `store_days.promo_redemptions` panel has no variance"
    );
    assert_eq!(
        j.raw["first_stage"]["refusal"],
        "the driver `store_days.promo_redemptions` never moved in any unit"
    );
}

#[test]
fn naming_the_driver_is_idempotent_and_leaves_other_refusals_alone() {
    let once = name_the_driver(
        Judged::<RatioIn>::from_engine(&refused_ratio("the driver panel has no variance", None)),
        "store_days.promo_redemptions",
    );
    let twice = name_the_driver(once.clone(), "store_days.promo_redemptions");
    assert_eq!(twice.view, once.view);
    let other = name_the_driver(
        Judged::<RatioIn>::from_engine(&refused_ratio("no unit stayed untreated", None)),
        "store_days.promo_redemptions",
    );
    assert_eq!(
        other.view.refusal.as_deref(),
        Some("no unit stayed untreated")
    );
    let host_worded = name_the_driver(
        Judged::<RatioIn>::refused(
            "no history for `store_days.promo_redemptions` over `store`".to_string(),
        ),
        "store_days.promo_redemptions",
    );
    assert_eq!(
        host_worded.view.refusal.as_deref(),
        Some("no history for `store_days.promo_redemptions` over `store`")
    );
}

#[test]
fn a_refused_judgement_carries_its_reason_in_both_forms() {
    let j = Judged::<EffectIn>::refused("no history for `m` over `store`".to_string());
    assert_eq!(
        j.view.refusal.as_deref(),
        Some("no history for `m` over `store`")
    );
    assert_eq!(j.raw["refusal"], "no history for `m` over `store`");
}
