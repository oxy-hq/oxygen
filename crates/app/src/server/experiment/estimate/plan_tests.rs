use std::collections::{BTreeMap, HashMap};

use oxy_airlayer_compat::engine::experiment::{
    Assignment, DesignShape, ExperimentDesign, Period, SwitchbackSchedule,
};
use oxy_experiment::config::parse;
use oxy_experiment::engine_in::EffectIn;
use oxy_experiment::report::SwitchbackFacts;

use super::plan::*;
use super::testdata::{CONFIG_YAML, switchback_config};
use crate::server::experiment::panel::GroupMembership;

fn assignment(days: &[(&str, Option<i64>)]) -> Assignment {
    Assignment {
        switch_day: days
            .iter()
            .map(|(u, d)| (u.to_string(), *d))
            .collect::<HashMap<_, _>>(),
        strata: None,
        pre_days: 56,
        post_days: 56,
        anticipation_days: 3,
        washout_days: 7,
        coverage_floor: 0.9,
        alpha: 0.05,
        family: 1,
    }
}

fn waves(days: &[(&str, Option<i64>)]) -> ExperimentDesign {
    ExperimentDesign::Waves(assignment(days))
}

/// Four consecutive 7-day periods from day 100: (on, off), (off, on).
fn schedule() -> ExperimentDesign {
    let on = [true, false, false, true];
    ExperimentDesign::Switchback(SwitchbackSchedule {
        periods: on
            .iter()
            .enumerate()
            .map(|(i, o)| Period {
                from_day: 100 + 7 * i as i64,
                on: *o,
            })
            .collect(),
        period_days: 7,
        washout_days: 1,
        coverage_floor: 0.9,
        alpha: 0.2,
        family: 1,
    })
}

#[test]
fn sibling_design_spec_is_bounded_before_the_earliest_switch() {
    // A placebo drawn over post-switch history swallows the real effect and inflates the floor.
    let cfg = parse(CONFIG_YAML).unwrap();
    let d = waves(&[("a", Some(100)), ("b", Some(100)), ("c", None), ("d", None)]);
    let spec = sibling_design_spec(&cfg, &d, 60).unwrap();
    assert_eq!(
        spec.history_to,
        Some(100 - 3),
        "earliest switch day − anticipation_days"
    );
    assert_eq!(spec.iterations, 60);
    assert_eq!(
        (
            spec.pre_days,
            spec.post_days,
            spec.anticipation_days,
            spec.washout_days
        ),
        (56, 56, 3, 7)
    );
    assert_eq!(
        (spec.alpha, spec.family, spec.power, spec.coverage_floor),
        (0.05, 1, cfg.outcome.power, 0.9)
    );
    assert_eq!(
        spec.blocks, 0,
        "an unblocked assignment prices an unblocked placebo"
    );
    let DesignShape::CommonDate {
        n_treated,
        n_control,
    } = spec.shape
    else {
        panic!("common date")
    };
    assert_eq!((n_treated, n_control), (2, 2));
}

#[test]
fn a_staggered_design_mirrors_its_own_wave_structure() {
    let cfg = parse(CONFIG_YAML).unwrap();
    let d = waves(&[
        ("a", Some(100)),
        ("b", Some(114)),
        ("c", Some(114)),
        ("d", None),
        ("e", None),
    ]);
    let spec = sibling_design_spec(&cfg, &d, 60).unwrap();
    assert_eq!(
        spec.history_to,
        Some(97),
        "the earliest wave sets the bound, not the latest"
    );
    let DesignShape::Staggered {
        wave_sizes,
        spacing_days,
        n_never_treated,
    } = spec.shape
    else {
        panic!("staggered")
    };
    assert_eq!(
        (wave_sizes, spacing_days, n_never_treated),
        (vec![1, 2], 14, 2)
    );
}

#[test]
fn a_blocked_assignment_blocks_the_placebo_the_same_way() {
    // Anything else prices a different design (spec § Stratified assignment).
    let cfg = parse(CONFIG_YAML).unwrap();
    let mut a = assignment(&[("a", Some(100)), ("b", None), ("c", Some(100)), ("d", None)]);
    a.strata = Some(vec![
        vec!["a".into(), "b".into()],
        vec!["c".into(), "d".into()],
    ]);
    let spec = sibling_design_spec(&cfg, &ExperimentDesign::Waves(a), 60).unwrap();
    assert_eq!(spec.blocks, 2);
}

#[test]
fn a_switchback_spec_mirrors_its_schedule_and_is_bounded_before_its_first_period() {
    let cfg = switchback_config();
    let spec = sibling_design_spec(&cfg, &schedule(), 60).unwrap();
    let DesignShape::Switchback { period_days, pairs } = spec.shape else {
        panic!("switchback")
    };
    assert_eq!((period_days, pairs), (7, 2));
    // pre/post/anticipation/blocks are ignored (must be 0) for a switchback; no anticipation moves the bound
    assert_eq!(
        (
            spec.pre_days,
            spec.post_days,
            spec.anticipation_days,
            spec.blocks
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        (spec.washout_days, spec.alpha, spec.coverage_floor),
        (1, 0.2, 0.9)
    );
    assert_eq!(
        spec.history_to,
        Some(100),
        "nothing from the first period on may be a placebo draw"
    );
}

#[test]
fn a_design_with_no_treated_unit_or_period_has_no_spec() {
    let cfg = parse(CONFIG_YAML).unwrap();
    assert!(sibling_design_spec(&cfg, &waves(&[("a", None), ("b", None)]), 60).is_err());
    let empty = ExperimentDesign::Switchback(SwitchbackSchedule {
        periods: vec![],
        period_days: 7,
        washout_days: 1,
        coverage_floor: 0.9,
        alpha: 0.05,
        family: 1,
    });
    assert!(sibling_design_spec(&cfg, &empty, 60).is_err());
}

#[test]
fn never_treated_counts_the_controls_and_a_switchback_has_none() {
    assert_eq!(
        never_treated(&waves(&[("a", Some(1)), ("b", None), ("c", None)])),
        2
    );
    assert_eq!(
        never_treated(&schedule()),
        0,
        "the whole fleet is treated on its on-days"
    );
}

#[test]
fn a_switchbacks_facts_take_the_fleet_from_the_panel_and_the_on_days_from_the_engine() {
    let cfg = switchback_config(); // period_days 7, washout_days 1
    let counted = EffectIn {
        n_treated: 4,
        retained_on_days: Some(21),
        ..Default::default()
    };
    // 4 pairs × 6 settled days would be 24; the engine dropped 3 days for missing data and says 21.
    assert_eq!(
        switchback_facts(&cfg, &counted, 12),
        Ok(Some(SwitchbackFacts {
            fleet_units: 12,
            on_days: 21
        }))
    );
    let waves = parse(CONFIG_YAML).unwrap();
    assert_eq!(
        switchback_facts(&waves, &EffectIn::default(), 12),
        Ok(None),
        "not a switchback"
    );
}

#[test]
fn a_switchback_effect_without_retained_on_days_is_a_section_refusal_not_a_computed_total() {
    let cfg = switchback_config();
    let missing = EffectIn {
        n_treated: 4,
        retained_on_days: None,
        ..Default::default()
    };
    let why = switchback_facts(&cfg, &missing, 12).unwrap_err();
    assert!(why.contains("retained_on_days"), "{why}");
    // An engine refusal already says why there is no estimate; it is not double-reported.
    let refused = EffectIn {
        refusal: Some("fewer than 6 pairs".into()),
        ..Default::default()
    };
    assert_eq!(switchback_facts(&cfg, &refused, 12), Ok(None));
}

#[test]
fn what_must_be_pinned_depends_on_the_design() {
    let pool = vec!["s07".to_string()];
    let membership = GroupMembership(BTreeMap::from([(
        "north".to_string(),
        vec!["s01".to_string()],
    )]));
    let unit = parse(CONFIG_YAML).unwrap();
    let fleet = vec!["s01".to_string(), "s02".to_string()];
    assert_eq!(
        missing_pin(&unit, Some(&pool), None, None),
        None,
        "units: the pool is enough"
    );
    assert_eq!(missing_pin(&unit, None, None, None), Some(NO_POOL));

    let mut grouped = unit.clone();
    grouped.assign_by = Some("store.region".into());
    assert_eq!(
        missing_pin(&grouped, Some(&pool), None, None),
        Some(NO_MEMBERSHIP),
        "groups: the membership too"
    );
    assert_eq!(
        missing_pin(&grouped, Some(&pool), Some(&membership), None),
        None
    );
    assert_eq!(
        missing_pin(&grouped, None, Some(&membership), None),
        Some(NO_POOL)
    );

    // A switchback has no control arm, so no pool and no membership — but it pins its FLEET:
    // without it a store opened mid-run would enter a period mean.
    let sb = switchback_config();
    assert_eq!(
        missing_pin(&sb, None, None, Some(&fleet)),
        None,
        "no pool needed"
    );
    assert_eq!(
        missing_pin(&sb, None, None, None),
        Some(NO_FLEET),
        "a pin-missing refusal, like the pool's"
    );
    assert_eq!(
        missing_pin(&sb, Some(&pool), None, None),
        Some(NO_FLEET),
        "a pool is no substitute for the fleet"
    );
    assert_eq!(
        missing_pin(&unit, Some(&pool), None, Some(&fleet)),
        None,
        "a fleet means nothing to a waves design"
    );
}

#[test]
fn derived_seeds_fit_in_53_bits() {
    // C2b: seeds survive JSON and JavaScript only below 2^53. A 64-bit mix would not.
    const LIMIT: u64 = 1 << 53;
    for seed in [
        0,
        1,
        8841207,
        (1u64 << 53) - 1,
        0xDEAD_BEEF_CAFE_F00D & ((1u64 << 53) - 1),
    ] {
        let s = derive_seeds(seed);
        assert!(s.placebo < LIMIT && s.ratio < LIMIT, "seed {seed}: {s:?}");
        assert_eq!(
            s.effect, seed,
            "the registered seed is passed through, already 53-bit"
        );
    }
    // Over many seeds a 64-bit mix would exceed the limit almost surely; the mask must hold for all.
    assert!((0..2_000u64).all(|k| {
        let s = derive_seeds(k);
        s.placebo < LIMIT && s.ratio < LIMIT
    }));
}

#[test]
fn seeds_are_deterministic_distinct_and_the_effect_seed_is_the_registered_one() {
    let s = derive_seeds(8841207);
    assert_eq!(s, derive_seeds(8841207));
    assert_eq!(s.effect, 8841207);
    assert!(s.placebo != s.ratio && s.placebo != s.effect && s.ratio != s.effect);
    assert_ne!(derive_seeds(1).placebo, derive_seeds(2).placebo);
}

#[test]
fn a_switchback_has_no_assignment_universe() {
    assert!(assignment_units(&switchback_config(), &[]).is_empty());
}

#[test]
fn the_assignment_universe_is_the_treated_units_plus_the_pinned_pool() {
    // Built from the run's pinned design and pool — never from a fresh unit query,
    // so a store that opened mid-run cannot drift into the control arm.
    let cfg = parse(CONFIG_YAML).unwrap(); // wave: s01 s02 s03
    let pool = ["s09".to_string(), "s07".to_string(), "s03".to_string()]; // s03 also treated: de-duplicated
    assert_eq!(
        assignment_units(&cfg, &pool),
        vec!["s01", "s02", "s03", "s07", "s09"]
    );
    assert_eq!(assignment_units(&cfg, &[]), vec!["s01", "s02", "s03"]);
}

#[test]
fn the_refs_token_names_the_design_version() {
    assert_eq!(design_ref("3f9a0c41d2e7b8aa55"), "3f9a0c41d2e7");
}
