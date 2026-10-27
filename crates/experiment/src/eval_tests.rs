use std::collections::BTreeMap;

use crate::config::{PriorCoefficient, PriorSource};
use crate::eval::*;
use crate::test_fixtures::{built_report, config, effect, ratio};

fn planted() -> Planted {
    // s01..s05 gain 20.0 per prompted store-day; s06 never offers a drink (0.0).
    let mut m: BTreeMap<String, f64> = (1..=5).map(|i| (format!("s0{i}"), 20.0)).collect();
    m.insert("s06".into(), 0.0);
    Planted {
        per_unit_day: m,
        slope: 5.0,
        slope_tolerance: 0.25,
    }
}

fn treated() -> Vec<String> {
    (1..=6).map(|i| format!("s0{i}")).collect()
}

#[test]
fn truth_of_a_unit_design_averages_over_the_treated_units_including_the_non_responder() {
    let t = planted()
        .truth(&Scope::Units { treated: treated() })
        .unwrap();
    assert!((t - 100.0 / 6.0).abs() < 1e-9, "{t}"); // five 20s and a 0, not a 20
}

#[test]
fn truth_of_a_group_design_sums_members_then_averages_groups() {
    let groups = BTreeMap::from([
        ("g1".to_string(), vec!["s01".to_string(), "s02".to_string()]), // 40
        ("g2".to_string(), vec!["s03".to_string(), "s06".to_string()]), // 20
    ]);
    assert_eq!(
        planted().truth(&Scope::Groups { treated: groups }),
        Some(30.0)
    );
}

#[test]
fn truth_of_a_fleet_is_the_mean_over_every_store() {
    let t = planted().truth(&Scope::Fleet).unwrap();
    assert!((t - 100.0 / 6.0).abs() < 1e-9);
}

#[test]
fn a_treated_unit_missing_from_the_planted_map_has_no_truth() {
    // A typo in truth.json must not read as a planted zero.
    assert_eq!(
        planted().truth(&Scope::Units {
            treated: vec!["s01".into(), "s99".into()]
        }),
        None
    );
    assert_eq!(planted().truth(&Scope::Units { treated: vec![] }), None);
}

#[test]
fn interval_covers_names_the_numbers_on_a_miss() {
    let r = built_report(&config(), &effect(16.0, 2.0), None);
    assert_eq!(interval_covers("c", &r, 16.5).verdict, Verdict::Pass);
    let miss = interval_covers("c", &r, 25.0);
    assert_eq!(miss.verdict, Verdict::Miss);
    assert!(
        miss.detail.contains("25.00")
            && miss.detail.contains("14.00")
            && miss.detail.contains("18.00"),
        "{}",
        miss.detail
    );
}

#[test]
fn an_unbounded_or_refused_report_is_inconclusive_never_a_pass() {
    let mut e = effect(16.0, 2.0);
    e.ci_high = None;
    assert_eq!(
        interval_covers("c", &built_report(&config(), &e, None), 16.0).verdict,
        Verdict::Inconclusive
    );
    let refused = built_report(
        &config(),
        &crate::test_fixtures::refused("no unit stayed untreated"),
        None,
    );
    assert_eq!(
        interval_covers("c", &refused, 16.0).verdict,
        Verdict::Inconclusive
    );
}

#[test]
fn a_null_run_that_claims_an_effect_is_a_miss() {
    let claims = built_report(&config(), &effect(12.5, 2.0), None); // significant
    let quiet = built_report(&config(), &effect(0.3, 2.0), None); // interval spans zero
    assert_eq!(claims_no_effect(&claims).verdict, Verdict::Miss);
    assert_eq!(claims_no_effect(&quiet).verdict, Verdict::Pass);
}

#[test]
fn the_non_responder_must_be_treated_for_the_dilution_to_be_exercised() {
    let r = built_report(&config(), &effect(16.5, 2.0), None);
    let in_control: Vec<String> = (1..=5).map(|i| format!("s0{i}")).collect(); // s06 drew control
    let c = non_responder_is_itt(&r, &planted(), &in_control, "s06");
    assert_eq!(c.verdict, Verdict::Inconclusive);
    assert!(c.detail.contains("control"), "{}", c.detail);
}

#[test]
fn the_non_responder_check_compares_with_the_diluted_number_not_the_responders_only_one() {
    // Diluted truth = 16.67; responders-only = 20.0.
    let diluted = built_report(&config(), &effect(16.5, 2.0), None); // [14.5, 18.5]
    assert_eq!(
        non_responder_is_itt(&diluted, &planted(), &treated(), "s06").verdict,
        Verdict::Pass
    );
    let undiluted = built_report(&config(), &effect(20.0, 1.0), None); // [19, 21]: the responders-only number
    assert_eq!(
        non_responder_is_itt(&undiluted, &planted(), &treated(), "s06").verdict,
        Verdict::Miss
    );
}

fn with_prior(value: f64) -> crate::config::ExperimentConfig {
    let mut c = config();
    c.outcome.prior_coefficient = Some(PriorCoefficient {
        value,
        source: PriorSource::Fitted,
    });
    c
}

#[test]
fn the_slope_interval_must_cover_the_planted_slope() {
    let r = built_report(&config(), &effect(25.0, 3.0), Some(&ratio(5.0))); // [4, 6]
    assert_eq!(slope_covered(&r, &planted()).verdict, Verdict::Pass);
    let off = built_report(&config(), &effect(25.0, 3.0), Some(&ratio(9.0))); // [8, 10]
    assert_eq!(slope_covered(&off, &planted()).verdict, Verdict::Miss);
}

#[test]
fn the_reports_agree_or_contradict_verdict_must_match_how_close_the_prior_is_to_the_planted_slope()
{
    // Planted 5.0, tolerance 25%: a prior of 5.2 should be agreed with, 9.0 contradicted.
    let agree = built_report(&with_prior(5.2), &effect(25.0, 3.0), Some(&ratio(5.0))); // set [4,6] holds 5.2
    assert_eq!(
        prior_verdict_matches_truth(&agree, &planted()).verdict,
        Verdict::Pass
    );
    let contradict = built_report(&with_prior(9.0), &effect(25.0, 3.0), Some(&ratio(5.0)));
    assert_eq!(
        prior_verdict_matches_truth(&contradict, &planted()).verdict,
        Verdict::Pass
    );
    // A prior near the truth that the report nevertheless contradicts (a set that missed) is a miss.
    let wrong = built_report(&with_prior(5.2), &effect(25.0, 3.0), Some(&ratio(8.0))); // set [7,9]
    assert_eq!(
        prior_verdict_matches_truth(&wrong, &planted()).verdict,
        Verdict::Miss
    );
}

#[test]
fn no_prediction_is_inconclusive_and_says_why() {
    let r = built_report(&config(), &effect(25.0, 3.0), Some(&ratio(5.0))); // no prior frozen
    let c = prior_verdict_matches_truth(&r, &planted());
    assert_eq!(c.verdict, Verdict::Inconclusive);
}
