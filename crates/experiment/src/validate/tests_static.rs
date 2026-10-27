use super::*;
use crate::config::GuardrailCriteria;
use crate::test_fixtures::{date, draft, edited, full, only};

#[test]
fn the_specs_example_and_a_draft_pass_static_validation() {
    assert_eq!(validate_static(&full()), vec![]);
    assert_eq!(validate_static(&draft()), vec![]);
}

#[test]
fn same_unit_in_two_waves_is_refused() {
    let err = only(&edited(|c| c.design.waves[1].units.push("bondi".into())));
    assert_eq!(
        err,
        ConfigError::UnitInTwoWaves {
            unit: "bondi".into(),
            first_wave: 1,
            first_switch_on: date(2026, 3, 2).into(),
            second_wave: 2,
            second_switch_on: date(2026, 3, 16).into(),
        }
    );
    let msg = err.to_string();
    assert!(
        msg.contains("\"bondi\"") && msg.contains("wave 1") && msg.contains("wave 2"),
        "{msg}"
    );
    assert!(!msg.contains("control") && !msg.contains("alpha"), "{msg}");
}

#[test]
fn a_unit_repeated_inside_one_wave_is_not_a_two_wave_refusal() {
    // The guard tests "in two waves". One wave listing a name twice still gives
    // the unit one switch date, so it is not this refusal.
    assert_eq!(
        validate_static(&edited(|c| c.design.waves[0].units.push("bondi".into()))),
        vec![]
    );
}

#[test]
fn a_wave_with_no_units_is_refused() {
    let err = only(&edited(|c| c.design.waves[1].units.clear()));
    assert_eq!(
        err,
        ConfigError::EmptyWave {
            wave: 2,
            switch_on: date(2026, 3, 16).into()
        }
    );
    assert!(err.to_string().contains("lists no units"));
}

#[test]
fn two_waves_with_the_same_switch_on_are_refused() {
    let err = only(&edited(|c| {
        c.design.waves[1].switch_on = date(2026, 3, 2).into()
    }));
    assert_eq!(
        err,
        ConfigError::DuplicateSwitchDate {
            switch_on: date(2026, 3, 2).into(),
            first_wave: 1,
            second_wave: 2
        }
    );
    let msg = err.to_string();
    assert!(msg.contains("2026-03-02") && !msg.contains("unit"), "{msg}");
}

#[test]
fn alpha_outside_the_open_unit_interval_is_refused() {
    for bad in [0.0, 1.0, -0.5, 1.5, f64::NAN] {
        let err = only(&edited(|c| c.outcome.alpha = bad));
        assert!(
            matches!(err, ConfigError::AlphaOutOfRange { .. }),
            "alpha {bad}: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("outcome.alpha"), "{msg}");
        assert!(!msg.contains("power") && !msg.contains("floor"), "{msg}");
    }
    for ok in [0.001, 0.05, 0.5, 0.999] {
        assert_eq!(
            validate_static(&edited(|c| c.outcome.alpha = ok)),
            vec![],
            "alpha {ok}"
        );
    }
}

#[test]
fn power_outside_the_open_unit_interval_is_refused() {
    for bad in [0.0, 1.0, -0.5, 1.5, f64::NAN] {
        let err = only(&edited(|c| c.outcome.power = bad));
        assert!(
            matches!(err, ConfigError::PowerOutOfRange { .. }),
            "power {bad}: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("outcome.power"), "{msg}");
        assert!(!msg.contains("alpha") && !msg.contains("floor"), "{msg}");
    }
    assert_eq!(
        validate_static(&edited(|c| c.outcome.power = 0.999)),
        vec![]
    );
}

#[test]
fn coverage_floor_must_be_above_zero_and_at_most_one() {
    for bad in [0.0, -0.1, 1.01, f64::NAN] {
        let err = only(&edited(|c| c.design.coverage_floor = bad));
        assert!(
            matches!(err, ConfigError::CoverageFloorOutOfRange { .. }),
            "{bad}: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("design.coverage_floor") && !msg.contains("compliance"),
            "{msg}"
        );
    }
    for ok in [0.001, 0.9, 1.0] {
        assert_eq!(
            validate_static(&edited(|c| c.design.coverage_floor = ok)),
            vec![],
            "{ok}"
        );
    }
}

#[test]
fn compliance_floor_must_be_above_zero_and_at_most_one() {
    for bad in [0.0, -0.1, 1.01, f64::NAN] {
        let err = only(&edited(|c| c.design.compliance_floor = bad));
        assert!(
            matches!(err, ConfigError::ComplianceFloorOutOfRange { .. }),
            "{bad}: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("design.compliance_floor") && !msg.contains("coverage"),
            "{msg}"
        );
    }
    assert_eq!(
        validate_static(&edited(|c| c.design.compliance_floor = 1.0)),
        vec![]
    );
}

#[test]
fn family_below_one_is_refused() {
    let err = only(&edited(|c| c.outcome.family = 0));
    assert_eq!(err, ConfigError::FamilyBelowOne { value: 0 });
    assert!(err.to_string().contains("outcome.family"));
    assert_eq!(validate_static(&edited(|c| c.outcome.family = 3)), vec![]);
}

#[test]
fn zero_pre_days_is_refused() {
    let err = only(&edited(|c| c.design.pre_days = Some(0)));
    assert_eq!(err, ConfigError::PreDaysZero);
    assert!(!err.to_string().contains("post_days"));
}

#[test]
fn zero_post_days_is_refused() {
    let err = only(&edited(|c| c.design.post_days = Some(0)));
    assert_eq!(err, ConfigError::PostDaysZero);
    assert!(!err.to_string().contains("pre_days"));
}

#[test]
fn guardrail_significance_outside_the_open_unit_interval_is_refused() {
    for bad in [0.0, 1.0, 1.5, -1.0, f64::NAN] {
        let err = only(&edited(|c| {
            c.guardrails[0].criteria = GuardrailCriteria::Anomaly {
                measure: "store_days.refund_rate".into(),
                significance: bad,
            };
        }));
        assert!(
            matches!(
                err,
                ConfigError::GuardrailSignificanceOutOfRange { guardrail: 0, .. }
            ),
            "{bad}: {err:?}"
        );
        assert!(err.to_string().contains("guardrails[0]"));
    }
}

#[test]
fn a_trigger_guardrail_has_no_significance_to_refuse() {
    // guardrails[1] is a trigger; only anomaly criteria carry a significance.
    assert_eq!(validate_static(&full()), vec![]);
}

#[test]
fn a_non_positive_or_nan_mde_at_proposal_is_refused() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let err = only(&edited(|c| c.outcome.mde_at_proposal = Some(bad)));
        assert!(
            matches!(err, ConfigError::MdeAtProposalNotPositive { .. }),
            "{bad}: {err:?}"
        );
        assert!(err.to_string().contains("outcome.mde_at_proposal"));
    }
    // Absent is a draft, not a refusal here (`is_registrable` owns that).
    assert_eq!(
        validate_static(&edited(|c| c.outcome.mde_at_proposal = None)),
        vec![]
    );
}

#[test]
fn a_non_finite_prior_coefficient_is_refused_and_a_negative_one_is_not() {
    use crate::config::{PriorCoefficient, PriorSource};
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let err = only(&edited(|c| {
            c.outcome.prior_coefficient = Some(PriorCoefficient {
                value: bad,
                source: PriorSource::Declared,
            });
        }));
        assert!(
            matches!(err, ConfigError::PriorCoefficientNotFinite { .. }),
            "{bad}: {err:?}"
        );
        assert!(
            err.to_string().contains("outcome.prior_coefficient.value"),
            "{err}"
        );
    }
    let slope = |v| PriorCoefficient {
        value: v,
        source: PriorSource::Fitted,
    };
    for ok in [-2.5, 0.0, 4.2] {
        assert_eq!(
            validate_static(&edited(|c| c.outcome.prior_coefficient = Some(slope(ok)))),
            vec![],
            "{ok}"
        );
    }
    assert_eq!(
        validate_static(&edited(|c| c.outcome.prior_coefficient = None)),
        vec![]
    );
}

#[test]
fn a_malformed_when_on_is_refused_naming_the_field() {
    let err = only(&edited(|c| {
        c.lever.as_mut().unwrap().read_back.when_on = "{{promo_active}} > 0".into()
    }));
    let ConfigError::InvalidExpression { field, error } = &err else {
        panic!("expected InvalidExpression, got {err:?}");
    };
    assert_eq!(field, "lever.read_back.when_on");
    assert_eq!(error.expr, "{{promo_active}} > 0");
    let msg = err.to_string();
    assert!(
        msg.contains("lever.read_back.when_on") && msg.contains("one `view.measure`"),
        "{msg}"
    );
    assert!(!msg.contains("when_off"), "{msg}");
}

#[test]
fn a_malformed_when_off_is_refused_naming_the_field() {
    let err = only(&edited(|c| {
        c.lever.as_mut().unwrap().read_back.when_off = "store_days.promo_active == 0".into()
    }));
    let ConfigError::InvalidExpression { field, .. } = &err else {
        panic!("expected InvalidExpression, got {err:?}");
    };
    assert_eq!(field, "lever.read_back.when_off");
    let msg = err.to_string();
    assert!(
        msg.contains("must start with") && !msg.contains("when_on"),
        "{msg}"
    );
}

#[test]
fn a_malformed_trigger_when_is_refused_naming_its_guardrail() {
    let err = only(&edited(|c| {
        c.guardrails[1].criteria = GuardrailCriteria::Trigger {
            when: "{{store_days.labor_cost_pct}} > lots".into(),
        };
    }));
    let ConfigError::InvalidExpression { field, .. } = &err else {
        panic!("expected InvalidExpression, got {err:?}");
    };
    assert_eq!(field, "guardrails[1].criteria.when");
    assert!(err.to_string().contains("must be a number"), "{err}");
}

#[test]
fn every_refusal_a_file_earns_is_reported_not_just_the_first() {
    let errs = validate_static(&edited(|c| {
        c.outcome.alpha = 2.0;
        c.design.pre_days = Some(0);
    }));
    assert_eq!(errs.len(), 2, "{errs:?}");
}

#[test]
fn floor_and_significance_messages_state_the_interval_they_check() {
    let msg = only(&edited(|c| c.design.coverage_floor = 0.0)).to_string();
    assert!(msg.contains("not in (0, 1]"), "{msg}");
    let msg = only(&edited(|c| c.design.compliance_floor = 1.5)).to_string();
    assert!(msg.contains("not in (0, 1]"), "{msg}");
}

fn one_wave(c: &mut crate::config::ExperimentConfig) {
    let mut all: Vec<String> = c
        .design
        .waves
        .iter()
        .flat_map(|w| w.units.clone())
        .collect();
    all.truncate(2);
    c.design.waves.truncate(1);
    c.design.waves[0].units = all;
}

#[test]
fn a_carryover_window_on_one_switch_date_passes() {
    let c = edited(|c| {
        one_wave(c);
        c.design.carryover_days = Some(14);
    });
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn a_trigger_may_compare_a_ratio_of_two_measures() {
    let c = edited(|c| {
        c.guardrails[1].criteria = GuardrailCriteria::Trigger {
            when: "{{store_days.labor_cost}} / {{store_days.revenue}} > 0.34".into(),
        };
    });
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn a_read_back_stays_one_measure() {
    let err = only(&edited(|c| {
        c.lever.as_mut().unwrap().read_back.when_on =
            "{{store_days.a}} / {{store_days.b}} > 0".into();
    }));
    assert!(
        matches!(&err, ConfigError::InvalidExpression { field, .. } if field == "lever.read_back.when_on"),
        "{err:?}"
    );
}

fn filter(dimension: &str, values: &[&str]) -> crate::config::MeasureFilter {
    crate::config::MeasureFilter {
        dimension: dimension.into(),
        op: crate::config::FilterOp::Equals,
        values: values.iter().map(|v| v.to_string()).collect(),
    }
}

#[test]
fn a_filter_on_the_measures_own_view_passes() {
    let c = edited(|c| {
        c.outcome.target_filter = vec![filter("store_days.region", &["north"])];
        c.substitution_check.as_mut().unwrap().filter =
            vec![filter("store_days.format", &["mall"])];
    });
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn a_carryover_window_needs_one_switch_date() {
    // Staggered waves turn off together at close, a different time after each switch.
    let err = only(&edited(|c| c.design.carryover_days = Some(14)));
    assert_eq!(
        err,
        ConfigError::CarryoverNeedsOneSwitchDate { switch_dates: 2 }
    );
}

#[test]
fn a_filter_on_another_views_dimension_is_refused() {
    // Another view's dimension would join it in and fan the measure out.
    let err = only(&edited(|c| {
        c.outcome.target_filter = vec![filter("stores.region", &["north"])];
    }));
    assert_eq!(
        err,
        ConfigError::FilterOutsideView {
            field: "outcome.target_filter[0]".into(),
            dimension: "stores.region".into(),
            measure: "store_days.beverages_revenue".into(),
        }
    );
    assert!(err.to_string().contains("store_days"), "{err}");
}

#[test]
fn a_filter_with_no_values_is_refused() {
    let err = only(&edited(|c| {
        c.substitution_check.as_mut().unwrap().filter = vec![filter("store_days.region", &[])];
    }));
    assert_eq!(
        err,
        ConfigError::FilterNoValues {
            field: "substitution_check.filter[0]".into()
        }
    );
}

#[test]
fn a_carryover_window_needs_the_levers_to_turn_off() {
    let err = only(&edited(|c| {
        one_wave(c);
        c.design.carryover_days = Some(14);
        c.lever.as_mut().unwrap().on_close = crate::config::OnClose::Keep;
    }));
    assert_eq!(err, ConfigError::CarryoverWithLeversKept);
}

#[test]
fn a_zero_day_carryover_window_is_refused() {
    let err = only(&edited(|c| {
        one_wave(c);
        c.design.carryover_days = Some(0);
    }));
    assert_eq!(err, ConfigError::CarryoverZero);
}

#[test]
fn a_driver_filter_without_a_driver_is_refused() {
    let err = only(&edited(|c| {
        c.outcome.driver = None;
        c.outcome.prior_coefficient = None;
        c.outcome.driver_filter = vec![filter("store_days.region", &["north"])];
    }));
    assert_eq!(
        err,
        ConfigError::FilterWithoutMeasure {
            field: "outcome.driver_filter".into()
        }
    );
}
