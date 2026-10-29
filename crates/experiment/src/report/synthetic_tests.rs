//! A synthetic control's report: its label, its assumptions, no projection, and
//! each fit read back in the headline's scale.

use super::synthetic::{DONORS_UNTOUCHED, NO_SPILLOVER, SYNTHETIC_LABEL};
use super::tests::Case;
use crate::config::{Assignment, Comparison, Scale};
use crate::engine_in::{DonorWeightIn, SyntheticIn, SyntheticPointIn, UnitFitIn};

fn fits(effect: f64, actual: f64) -> SyntheticIn {
    let day = chrono::NaiveDate::from_ymd_opt(2026, 5, 1)
        .unwrap()
        .num_days_from_ce() as i64;
    SyntheticIn {
        fits: vec![UnitFitIn {
            unit: "a1".into(),
            effect: Some(effect),
            pre_fit: Some(0.31),
            rmspe_ratio: Some(4.2),
            rank_p: Some(1.0 / 21.0),
            weights: vec![
                DonorWeightIn {
                    unit: "b3".into(),
                    weight: 0.7,
                },
                DonorWeightIn {
                    unit: "b9".into(),
                    weight: 0.3,
                },
            ],
            series: vec![SyntheticPointIn {
                day,
                actual: Some(actual),
                synthetic: None,
            }],
        }],
        donors: (0..20).map(|i| format!("b{i}")).collect(),
        placebo_sets: 20,
        exact: true,
    }
}

fn synthetic_case() -> Case {
    let mut c = Case::new();
    c.cfg.design.assignment = Assignment::Observed;
    c.cfg.design.comparison = Comparison::SyntheticControl;
    c.retrospective = true;
    c.synthetic = Some(fits(4.0, 12.0));
    c
}

use chrono::Datelike;

#[test]
fn a_synthetic_report_says_so_states_its_assumptions_and_projects_nothing() {
    let v = synthetic_case().build();
    let s = v.synthetic.as_ref().expect("a synthetic section");
    assert_eq!(s.label, SYNTHETIC_LABEL);
    assert_eq!((s.donors, s.placebo_sets, s.exact), (20, 20, true));
    assert!(v.assumptions.contains(&NO_SPILLOVER.to_string()));
    assert!(v.assumptions.contains(&DONORS_UNTOUCHED.to_string()));
    assert!(v.estimand.contains("synthetic control"), "{}", v.estimand);
    assert!(v.projection.is_none());
    assert!(v.treated_total.is_some());
    let f = &s.fits[0];
    assert_eq!(f.weights[0].unit, "b3");
    assert_eq!(
        f.series[0].date,
        chrono::NaiveDate::from_ymd_opt(2026, 5, 1)
    );
    assert_eq!(
        (f.series[0].actual, f.series[0].synthetic),
        (Some(12.0), None)
    );
}

#[test]
fn a_report_without_a_synthetic_control_has_no_section_and_keeps_its_projection() {
    let v = Case::new().build();
    assert!(v.synthetic.is_none());
    assert!(v.projection.is_some());
    assert!(!v.assumptions.contains(&NO_SPILLOVER.to_string()));
}

#[test]
fn a_proportional_synthetic_reads_each_fit_back_in_levels_and_shares() {
    let mut c = synthetic_case();
    c.cfg.outcome.scale = Scale::Proportional;
    c.synthetic = Some(fits(0.1_f64.ln_1p(), 12.0_f64.ln_1p()));
    let v = c.build();
    let f = &v.synthetic.as_ref().unwrap().fits[0];
    assert!((f.effect.unwrap() - 0.1).abs() < 1e-12);
    assert!((f.series[0].actual.unwrap() - 12.0).abs() < 1e-9);
}
