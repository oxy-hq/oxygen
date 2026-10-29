//! `to_design` — the file's waves or switchback, as the engine's by-name design.
//!
//! Lives in the host, not in `oxy-experiment`: the engine types it builds come
//! from airlayer, and `oxy-compile` depends on `oxy-experiment` and must not
//! link the semantic engine (index C2, C2a).

use std::collections::HashMap;

use oxy_airlayer_compat::engine::experiment::{
    Assignment, ExperimentDesign, OneDateTest, PRE_FIT_CEILING, Period, SwitchbackSchedule,
    SyntheticSpec,
};
use oxy_experiment::config::Comparison;
use oxy_experiment::{ExperimentConfig, LeverState, SwitchbackConfig};

/// The design the engine estimates, by unit **name**.
///
/// Waves: every unit in `units` appears — `None` when no wave names it (a
/// control), `Some(grain.ordinal(switch_on))` otherwise — the period ordinal of
/// `design.grain`, so every window count is a count of ordinals — and so does every unit a
/// wave names, **even if `units` does not hold it**: dropping it would leave a
/// typo out of the assignment entirely, and the engine can only refuse a name it
/// is shown ("the assignment names a unit the panel does not hold"). A unit named
/// by two waves keeps the earlier one in file order; `validate_static` refuses
/// that file, so this only decides what an invalid input does, deterministically.
/// `units` are entity units, or group names under `assign_by`.
///
/// Switchback: the periods as ordinals, in file order (which is date order:
/// PR 2's `PeriodsNotPaired` refuses unsorted periods, index C2). `units` is not read.
///
/// `alpha` and `family` come from `outcome`, the floors and windows from
/// `design` — none is a constant here.
pub fn to_design(c: &ExperimentConfig, units: &[String]) -> ExperimentDesign {
    match &c.design.switchback {
        Some(sb) => ExperimentDesign::Switchback(schedule(c, sb)),
        None => ExperimentDesign::Waves(assignment(c, units)),
    }
}

/// The design the estimates read: [`to_design`]'s, or with
/// `design.comparison: synthetic_control` a synthetic control over the same
/// units — the treated units are the waves', the donors every other unit in
/// `units` (the run's pinned pool), less `design.exclude`. Validation keeps a
/// synthetic design to one switch date, so the first treated unit's day is
/// every treated unit's. A switchback is never synthetic.
pub fn to_engine_design(c: &ExperimentConfig, units: &[String]) -> ExperimentDesign {
    let design = to_design(c, units);
    if c.design.comparison != Comparison::SyntheticControl {
        return design;
    }
    match &design {
        ExperimentDesign::Waves(a) => ExperimentDesign::Synthetic(synthetic(c, a)),
        _ => design,
    }
}

fn synthetic(c: &ExperimentConfig, a: &Assignment) -> SyntheticSpec {
    let mut treated: Vec<(String, i64)> = a
        .switch_day
        .iter()
        .filter_map(|(u, s)| s.map(|day| (u.clone(), day)))
        .collect();
    treated.sort();
    let mut donors: Vec<String> = a
        .switch_day
        .iter()
        .filter(|(_, s)| s.is_none())
        .map(|(u, _)| u.clone())
        .collect();
    donors.sort();
    SyntheticSpec {
        start_on: treated.iter().map(|(_, d)| *d).min().unwrap_or_default(),
        treated: treated.into_iter().map(|(u, _)| u).collect(),
        donors: Some(donors),
        exclude: c.design.exclude.clone(),
        pre_days: a.pre_days,
        post_days: a.post_days,
        anticipation_days: a.anticipation_days,
        washout_days: a.washout_days,
        coverage_floor: a.coverage_floor,
        alpha: a.alpha,
        family: a.family,
        // A past-data reading registers nothing, so there is no threshold priced
        // and frozen: the engine's own ceiling refuses a fit no better than a
        // flat line.
        max_pre_fit: PRE_FIT_CEILING,
    }
}

fn schedule(c: &ExperimentConfig, sb: &SwitchbackConfig) -> SwitchbackSchedule {
    SwitchbackSchedule {
        periods: sb
            .periods
            .iter()
            .map(|p| Period {
                from_day: c.design.grain.ordinal_of(p.from),
                on: p.state == LeverState::On,
            })
            .collect(),
        period_days: sb.period_days as usize,
        washout_days: c.design.washout_days as usize,
        coverage_floor: c.design.coverage_floor,
        alpha: c.outcome.alpha,
        family: c.outcome.family as usize,
    }
}

/// The file's `design.one_date_test`, as the engine's.
pub(crate) fn engine_one_date_test(t: oxy_experiment::config::OneDateTest) -> OneDateTest {
    match t {
        oxy_experiment::config::OneDateTest::Welch => OneDateTest::Welch,
        oxy_experiment::config::OneDateTest::Permutation => OneDateTest::Permutation,
    }
}

fn assignment(c: &ExperimentConfig, units: &[String]) -> Assignment {
    let mut switch_day: HashMap<String, Option<i64>> =
        units.iter().map(|u| (u.clone(), None)).collect();
    for wave in &c.design.waves {
        let day = c.design.grain.ordinal_of(wave.switch_on);
        for unit in &wave.units {
            let slot = switch_day.entry(unit.clone()).or_insert(None);
            if slot.is_none() {
                *slot = Some(day);
            }
        }
    }
    Assignment {
        switch_day,
        strata: c.design.strata.clone(),
        pre_days: c.design.pre_days.unwrap_or_default() as usize,
        post_days: c.design.post_days.unwrap_or_default() as usize,
        anticipation_days: c.design.anticipation_days as usize,
        washout_days: c.design.washout_days as usize,
        coverage_floor: c.design.coverage_floor,
        alpha: c.outcome.alpha,
        family: c.outcome.family as usize,
        one_date_test: engine_one_date_test(c.design.one_date_test),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use oxy_airlayer_compat::engine::experiment::{
        Assignment, ExperimentDesign, OneDateTest, Period, day_ordinal,
    };
    use oxy_experiment::parse;

    use super::{to_design, to_engine_design};
    use crate::server::experiment::testing::{
        FOUR_PERIODS, MDE, STRATA, TWO_WAVES, switchback_yaml, yaml,
    };

    fn units(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn config() -> oxy_experiment::ExperimentConfig {
        parse(&yaml(TWO_WAVES, MDE, 56, 42, 7)).expect("parses")
    }

    fn waves(d: ExperimentDesign) -> Assignment {
        match d {
            ExperimentDesign::Waves(a) => a,
            other => panic!("expected a waves design, got {other:?}"),
        }
    }

    #[test]
    fn a_unit_in_no_wave_maps_to_none() {
        let a = waves(to_design(
            &config(),
            &units(&["bondi", "newtown", "manly", "cronulla"]),
        ));
        assert_eq!(a.switch_day["cronulla"], None);
    }

    #[test]
    fn switch_day_is_the_day_ordinal_of_switch_on() {
        let a = waves(to_design(&config(), &units(&["bondi", "newtown", "manly"])));
        assert_eq!(a.switch_day["bondi"], Some(day_ordinal(date(2026, 3, 2))));
        assert_eq!(a.switch_day["newtown"], Some(day_ordinal(date(2026, 3, 2))));
        assert_eq!(a.switch_day["manly"], Some(day_ordinal(date(2026, 3, 16))));
    }

    #[test]
    fn every_numeric_field_is_carried() {
        let a = waves(to_design(&config(), &units(&["bondi"])));
        assert_eq!(
            (
                a.pre_days,
                a.post_days,
                a.anticipation_days,
                a.washout_days,
                a.family
            ),
            (56, 42, 3, 7, 2)
        );
        assert_eq!(a.coverage_floor, 0.85);
        assert_eq!(a.alpha, 0.1);
    }

    #[test]
    fn a_wave_unit_the_panel_lacks_is_kept_so_the_engine_refuses_it() {
        // Review Focus 2. `newtown` and `manly` are in waves but not in the panel.
        // Dropping them would leave them out of the assignment altogether — and an
        // assignment that never mentions a unit is one the engine cannot refuse.
        let a = waves(to_design(&config(), &units(&["bondi", "cronulla"])));
        assert_eq!(a.switch_day.len(), 4);
        assert_eq!(a.switch_day["manly"], Some(day_ordinal(date(2026, 3, 16))));
        assert_eq!(a.switch_day["cronulla"], None);
    }

    #[test]
    fn a_unit_named_in_two_waves_keeps_its_first_switch_day() {
        // `validate_static` refuses this file; the bridge must still not panic or
        // pick at random if it is handed one.
        let waves_yaml = "  waves:\n    - { switch_on: 2026-03-02, units: [bondi] }\n    - { switch_on: 2026-03-16, units: [bondi] }\n  seed: 1\n";
        let c = parse(&yaml(waves_yaml, MDE, 28, 28, 0)).expect("parses");
        let a = waves(to_design(&c, &units(&["bondi"])));
        assert_eq!(a.switch_day["bondi"], Some(day_ordinal(date(2026, 3, 2))));
    }

    #[test]
    fn strata_are_carried_verbatim_and_absent_means_unblocked() {
        let with = parse(&yaml(&format!("{TWO_WAVES}{STRATA}"), MDE, 28, 28, 0)).expect("parses");
        let a = waves(to_design(
            &with,
            &units(&["bondi", "manly", "newtown", "glebe"]),
        ));
        assert_eq!(
            a.strata,
            Some(vec![
                units(&["bondi", "manly"]),
                units(&["newtown", "glebe"])
            ])
        );
        assert_eq!(waves(to_design(&config(), &units(&["bondi"]))).strata, None);
    }

    #[test]
    fn a_waves_file_without_windows_maps_them_to_zero_instead_of_panicking() {
        // A draft of a shape not yet chosen owes no window (PR 2). The engine
        // refuses a zero window by name; this function must not unwrap.
        let c = parse(&yaml("", "", 28, 28, 0).replace("  pre_days: 28\n  post_days: 28\n", ""))
            .expect("parses");
        let a = waves(to_design(&c, &units(&["bondi"])));
        assert_eq!((a.pre_days, a.post_days), (0, 0));
    }

    #[test]
    fn a_switchback_maps_periods_to_ordinals_and_on_flags() {
        let c = parse(&switchback_yaml("  seed: 5\n", FOUR_PERIODS, MDE, 2)).expect("parses");
        let ExperimentDesign::Switchback(s) = to_design(&c, &units(&["ignored"])) else {
            panic!("a switchback file must map to a switchback design");
        };
        assert_eq!(
            s.periods,
            vec![
                Period {
                    from_day: day_ordinal(date(2026, 3, 2)),
                    on: true
                },
                Period {
                    from_day: day_ordinal(date(2026, 3, 9)),
                    on: false
                },
                Period {
                    from_day: day_ordinal(date(2026, 3, 16)),
                    on: false
                },
                Period {
                    from_day: day_ordinal(date(2026, 3, 23)),
                    on: true
                },
            ]
        );
        assert_eq!((s.period_days, s.washout_days, s.family), (7, 2, 2));
        assert_eq!((s.coverage_floor, s.alpha), (0.85, 0.1));
    }

    #[test]
    fn a_switchback_does_not_read_the_units() {
        let c = parse(&switchback_yaml("  seed: 5\n", FOUR_PERIODS, MDE, 2)).expect("parses");
        let (ExperimentDesign::Switchback(a), ExperimentDesign::Switchback(b)) =
            (to_design(&c, &[]), to_design(&c, &units(&["a", "b", "c"])))
        else {
            panic!("a switchback file must map to a switchback design");
        };
        assert_eq!(a.periods, b.periods);
    }

    #[test]
    fn at_week_grain_switches_are_week_ordinals_two_weeks_apart() {
        let mut c = parse(&yaml(TWO_WAVES, MDE, 4, 4, 0)).expect("parses");
        c.design.grain = oxy_experiment::Grain::Week;
        let a = waves(to_design(&c, &units(&["bondi", "manly"])));
        let (first, second) = (
            a.switch_day["bondi"].unwrap(),
            a.switch_day["manly"].unwrap(),
        );
        assert_eq!(
            second - first,
            2,
            "2026-03-02 and 2026-03-16 are two weeks apart"
        );
        assert_eq!(a.pre_days, 4, "a window count is passed through as periods");
    }

    #[test]
    fn the_one_date_test_reaches_the_engine_assignment() {
        let u = units(&["bondi", "manly", "cronulla"]);
        assert_eq!(
            waves(to_design(&config(), &u)).one_date_test,
            OneDateTest::Welch
        );
        let mut c = config();
        c.design.one_date_test = oxy_experiment::config::OneDateTest::Permutation;
        assert_eq!(
            waves(to_design(&c, &u)).one_date_test,
            OneDateTest::Permutation
        );
    }

    #[test]
    fn an_untreated_comparison_is_the_waves_design_unchanged() {
        let u = units(&["bondi", "manly", "cronulla"]);
        let (a, b) = (
            waves(to_engine_design(&config(), &u)),
            waves(to_design(&config(), &u)),
        );
        assert_eq!(a.switch_day, b.switch_day);
        assert_eq!((a.pre_days, a.post_days), (b.pre_days, b.post_days));
    }

    #[test]
    fn a_synthetic_control_takes_the_wave_units_as_treated_and_the_rest_as_donors() {
        let mut c = config();
        c.design.comparison = oxy_experiment::config::Comparison::SyntheticControl;
        c.design.exclude = vec!["coogee".into()];
        let u = units(&["manly", "bondi", "cronulla", "coogee", "newtown"]);
        let ExperimentDesign::Synthetic(s) = to_engine_design(&c, &u) else {
            panic!("expected a synthetic design");
        };
        assert_eq!(s.treated, units(&["bondi", "manly", "newtown"]));
        assert_eq!(s.donors, Some(units(&["coogee", "cronulla"])));
        assert_eq!(
            s.exclude,
            units(&["coogee"]),
            "the engine drops excluded donors"
        );
        assert_eq!(s.start_on, day_ordinal(date(2026, 3, 2)));
        assert_eq!(
            (s.pre_days, s.post_days, s.washout_days, s.family),
            (56, 42, 7, 2)
        );
        assert_eq!(
            s.max_pre_fit,
            oxy_airlayer_compat::engine::experiment::PRE_FIT_CEILING
        );
    }
}
