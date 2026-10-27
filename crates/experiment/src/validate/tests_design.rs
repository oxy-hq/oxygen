use super::*;
use crate::config::{ExperimentConfig, LeverState, PeriodConfig};
use crate::test_fixtures::{date, draft, edited, full, only, switchback, switchback_draft};

/// The switchback example, edited.
fn sb(f: impl FnOnce(&mut ExperimentConfig)) -> ExperimentConfig {
    let mut c = switchback();
    f(&mut c);
    c
}

fn periods(c: &mut ExperimentConfig) -> &mut Vec<PeriodConfig> {
    &mut c.design.switchback.as_mut().expect("a switchback").periods
}

/// The `why` of the one `PeriodsNotPaired` refusal `c` earns.
fn pairing_fault(c: &ExperimentConfig) -> String {
    match only(c) {
        ConfigError::PeriodsNotPaired { why } => why,
        other => panic!("expected PeriodsNotPaired, got {other:?}"),
    }
}

#[test]
fn the_switchback_example_and_its_draft_pass_static_validation() {
    assert_eq!(validate_static(&switchback()), vec![]);
    assert_eq!(
        validate_static(&switchback_draft()),
        vec![],
        "empty periods are a draft, not a refusal"
    );
}

#[test]
fn waves_and_switchback_together_are_refused_once() {
    // Review Focus 6: one mistake, one refusal — not also a SwitchbackForbids.
    let err = only(&sb(|c| c.design.waves = full().design.waves));
    assert_eq!(err, ConfigError::WavesAndSwitchback);
    let msg = err.to_string();
    assert!(
        msg.contains("design.waves") && msg.contains("design.switchback"),
        "{msg}"
    );
}

#[test]
fn each_key_a_switchback_cannot_combine_with_is_refused_by_name() {
    // Review Focus 6.
    type Edit = fn(&mut ExperimentConfig);
    let cases: [(&'static str, Edit); 6] = [
        ("design.strata", |c| {
            c.design.strata = Some(vec![vec!["a".into()]])
        }),
        ("assign_by", |c| c.assign_by = Some("store.region".into())),
        ("design.pre_days", |c| c.design.pre_days = Some(56)),
        ("design.post_days", |c| c.design.post_days = Some(56)),
        ("design.anticipation_days (non-zero)", |c| {
            c.design.anticipation_days = 3
        }),
        ("design.carryover_days", |c| {
            c.design.carryover_days = Some(7)
        }),
    ];
    for (key, edit) in cases {
        let err = only(&sb(edit));
        assert_eq!(err, ConfigError::SwitchbackForbids { key }, "{key}");
        let msg = err.to_string();
        assert!(
            msg.contains(key) && msg.contains("design.switchback"),
            "{msg}"
        );
        for (other, _) in cases {
            let name = other.split(' ').next().unwrap();
            assert!(
                other == key || !msg.contains(name),
                "{key}: the message names {name} too: {msg}"
            );
        }
    }
    // Zero anticipation is the default and is not a key the file sets.
    assert_eq!(
        validate_static(&sb(|c| c.design.anticipation_days = 0)),
        vec![]
    );
    // The same keys on a draft switchback are refused now, not after the snippet is pasted.
    let mut d = switchback_draft();
    d.design.pre_days = Some(56);
    assert_eq!(
        only(&d),
        ConfigError::SwitchbackForbids {
            key: "design.pre_days"
        }
    );
}

#[test]
fn a_washout_not_shorter_than_the_period_is_refused() {
    for washout in [7, 8] {
        let err = only(&sb(|c| c.design.washout_days = washout));
        assert_eq!(
            err,
            ConfigError::SwitchbackWashoutTooLong {
                washout_days: washout,
                period_days: 7
            }
        );
        let msg = err.to_string();
        assert!(
            msg.contains("design.washout_days") && !msg.contains("grace"),
            "{msg}"
        );
    }
    assert_eq!(validate_static(&sb(|c| c.design.washout_days = 6)), vec![]);
}

#[test]
fn a_grace_not_shorter_than_the_period_is_refused() {
    for grace in [7, 30] {
        let err = only(&sb(|c| {
            c.lever.as_mut().unwrap().read_back.grace_days = grace
        }));
        assert_eq!(
            err,
            ConfigError::SwitchbackGraceTooLong {
                grace_days: grace,
                period_days: 7
            }
        );
        let msg = err.to_string();
        assert!(
            msg.contains("lever.read_back.grace_days") && !msg.contains("washout"),
            "{msg}"
        );
    }
    assert_eq!(
        validate_static(&sb(|c| c.lever.as_mut().unwrap().read_back.grace_days = 6)),
        vec![]
    );
}

#[test]
fn a_zero_day_period_is_refused_on_its_own() {
    // Not also a washout, a grace and a spacing fault: those are all vacuous at 0.
    let errs = validate_static(&sb(|c| {
        c.design.switchback.as_mut().unwrap().period_days = 0
    }));
    assert_eq!(errs, vec![ConfigError::SwitchbackPeriodDaysZero]);
    assert!(
        errs[0]
            .to_string()
            .contains("design.switchback.period_days is 0")
    );
}

#[test]
fn periods_must_be_consecutive_pairs_of_one_on_and_one_off() {
    // An odd count.
    let why = pairing_fault(&sb(|c| {
        periods(c).pop();
    }));
    assert!(why.contains("3 periods") && why.contains("even"), "{why}");

    // A gap that is not period_days: the second period starts a day late.
    let why = pairing_fault(&sb(|c| periods(c)[1].from = date(2026, 3, 10).into()));
    assert!(
        why.contains("2026-03-10") && why.contains("7 days"),
        "{why}"
    );

    // A pair that is on twice.
    let why = pairing_fault(&sb(|c| periods(c)[1].state = LeverState::On));
    assert!(why.contains("pair 1"), "{why}");
    // ... and the second pair, so the pair number is the pair's own.
    let why = pairing_fault(&sb(|c| periods(c)[2].state = LeverState::On));
    assert!(why.contains("pair 2"), "{why}");

    let msg = ConfigError::PeriodsNotPaired { why: "x".into() }.to_string();
    assert!(msg.contains("design.switchback.periods"), "{msg}");
}

#[test]
fn unsorted_periods_are_refused() {
    // The same four periods, listed newest first: file order is the schedule
    // order, so this is a refusal, not something to sort quietly.
    let why = pairing_fault(&sb(|c| periods(c).reverse()));
    assert!(
        why.contains("periods are not listed in date order"),
        "{why}"
    );
    // Two swapped periods are caught too, and the fault names them.
    let why = pairing_fault(&sb(|c| periods(c).swap(0, 1)));
    assert!(
        why.contains("periods are not listed in date order"),
        "{why}"
    );
    // A repeated start is not strictly increasing; a refusal, not a panic.
    let why = pairing_fault(&sb(|c| periods(c)[1].from = date(2026, 3, 2).into()));
    assert!(
        why.contains("periods are not listed in date order") && why.contains("2026-03-02"),
        "{why}"
    );
    // The example, listed in date order, is fine.
    assert_eq!(validate_static(&switchback()), vec![]);
}

#[test]
fn a_window_is_required_on_a_waves_design_and_only_there() {
    let err = only(&edited(|c| c.design.pre_days = None));
    assert_eq!(
        err,
        ConfigError::MissingWindow {
            key: "design.pre_days"
        }
    );
    assert!(!err.to_string().contains("post_days"));
    let err = only(&edited(|c| c.design.post_days = None));
    assert_eq!(
        err,
        ConfigError::MissingWindow {
            key: "design.post_days"
        }
    );

    let errs = validate_static(&edited(|c| {
        c.design.pre_days = None;
        c.design.post_days = None;
    }));
    assert_eq!(errs.len(), 2, "{errs:?}");

    // A draft of a shape not yet chosen owes no window; a switchback owes none ever.
    let mut d = draft();
    d.design.pre_days = None;
    d.design.post_days = None;
    assert_eq!(validate_static(&d), vec![]);
    assert_eq!(validate_static(&switchback()), vec![]);
}

#[test]
fn a_unit_in_two_strata_is_refused() {
    // Review Focus 7: static, so the compile fails.
    let err = only(&edited(|c| {
        c.design.strata.as_mut().unwrap()[1].push("bondi".into())
    }));
    assert_eq!(
        err,
        ConfigError::UnitInTwoStrata {
            unit: "bondi".into()
        }
    );
    let msg = err.to_string();
    assert!(
        msg.contains("\"bondi\"") && msg.contains("design.strata"),
        "{msg}"
    );
}

#[test]
fn a_unit_is_reported_once_however_many_strata_hold_it() {
    let errs = validate_static(&edited(|c| {
        let strata = c.design.strata.as_mut().unwrap();
        strata.push(vec!["bondi".into()]);
        strata[1].push("bondi".into());
    }));
    assert_eq!(
        errs,
        vec![ConfigError::UnitInTwoStrata {
            unit: "bondi".into()
        }]
    );
}

#[test]
fn a_unit_repeated_inside_one_stratum_is_not_a_two_strata_refusal() {
    assert_eq!(
        validate_static(&edited(
            |c| c.design.strata.as_mut().unwrap()[0].push("bondi".into())
        )),
        vec![]
    );
}

#[test]
fn a_pair_fault_prints_the_file_spelling_of_the_state() {
    let why = pairing_fault(&sb(|c| periods(c)[1].state = LeverState::On));
    assert!(why.contains("is on in both periods"), "{why}");
    let why = pairing_fault(&sb(|c| {
        periods(c)[0].state = LeverState::Off;
        periods(c)[1].state = LeverState::Off;
    }));
    assert!(why.contains("is off in both periods"), "{why}");
}

// ── grain ───────────────────────────────────────────────────────────────────

#[test]
fn a_week_grain_design_whose_switches_are_mondays_passes() {
    // 2026-03-02 and 2026-03-16 are Mondays.
    let c = edited(|c| c.design.grain = crate::Grain::Week);
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn a_week_grain_switch_that_is_not_a_monday_is_refused_naming_the_date() {
    let c = edited(|c| {
        c.design.grain = crate::Grain::Week;
        c.design.waves[1].switch_on = date(2026, 3, 18).into();
    });
    let err = only(&c);
    assert_eq!(
        err,
        ConfigError::NotAPeriodStart {
            key: "design.waves[2].switch_on".to_string(),
            date: date(2026, 3, 18).into(),
            grain: crate::Grain::Week,
        }
    );
    assert!(err.to_string().contains("Monday"), "{err}");
}

#[test]
fn a_month_grain_switchback_runs_on_consecutive_first_of_months() {
    let c = sb(|c| {
        c.design.grain = crate::Grain::Month;
        c.design.washout_days = 0;
        c.design.switchback.as_mut().unwrap().period_days = 1;
        let ps = periods(c);
        for (p, m) in ps.iter_mut().zip(3..) {
            p.from = date(2026, m, 1).into();
        }
    });
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn a_week_grain_switchback_counts_its_period_in_weeks() {
    // period_days: 7 at week grain is seven weeks, so periods a week apart are not paired.
    let c = sb(|c| {
        c.design.grain = crate::Grain::Week;
        c.design.washout_days = 0;
    });
    assert!(
        pairing_fault(&c).contains("7 weeks later"),
        "{}",
        pairing_fault(&c)
    );
}

#[test]
fn an_hour_grain_switchback_may_switch_every_few_hours_and_a_day_grain_one_never_mid_day() {
    let at = |s: &str| crate::Moment::parse(s).unwrap();
    let hourly = sb(|c| {
        c.design.grain = crate::Grain::Hour;
        c.design.washout_days = 1;
        c.lever.as_mut().unwrap().read_back.grace_days = 0;
        c.design.switchback.as_mut().unwrap().period_days = 4;
        for (p, h) in periods(c).iter_mut().zip(["00", "04", "08", "12"]) {
            p.from = at(&format!("2026-03-02T{h}:00"));
        }
    });
    assert_eq!(validate_static(&hourly), vec![]);
    let daily = sb(|c| periods(c)[0].from = at("2026-03-02T14:00"));
    assert!(
        validate_static(&daily)
            .iter()
            .any(|e| matches!(e, ConfigError::NotAPeriodStart { .. })),
        "{:?}",
        validate_static(&daily)
    );
}
