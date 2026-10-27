//! Pricing a switchback against the synthetic panel of `price_tests`.

use chrono::{NaiveDate, TimeZone, Utc};
use oxy_airlayer_compat::engine::experiment::{day_ordinal, propose_switchback};
use oxy_experiment::LeverState;

use super::candidate::{Candidate, SwitchbackCandidate, WavesCandidate};
use super::price::*;
use super::price_switchback::price_switchback;
use super::price_tests::{panel, run};
use super::snippet::plan_periods;
use super::testing::{TWO_WAVES, draft, registered, switchback_draft, with_assign_by, yaml};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn sb(period_days: u32, pairs: usize) -> SwitchbackCandidate {
    SwitchbackCandidate {
        first_period_on: date(2026, 3, 9).into(),
        period_days,
        pairs,
        alpha: None,
    }
}

fn priced(c: PriceComputation) -> Priced {
    match c {
        PriceComputation::Priced(p) => p,
        PriceComputation::Refused { reason, problems } => panic!("refused: {reason} {problems:?}"),
    }
}

fn periods(p: &Priced) -> (u32, &[super::snippet::PeriodPlan]) {
    match &p.plan {
        SchedulePlan::Switchback {
            period_days,
            periods,
        } => (*period_days, periods.as_slice()),
        other => panic!("expected a switchback, got {other:?}"),
    }
}

fn refused(c: PriceComputation) -> (String, Vec<String>) {
    match c {
        PriceComputation::Refused { reason, problems } => (reason, problems),
        PriceComputation::Priced(_) => panic!("must refuse"),
    }
}

#[test]
fn a_switchback_prices_pairs_with_exactly_one_on_and_one_off_each() {
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    let (period_days, plan) = periods(&p);
    assert_eq!((period_days, plan.len()), (7, 12));
    assert_eq!(plan[0].from, date(2026, 3, 9));
    assert_eq!(
        plan[1].from,
        date(2026, 3, 16),
        "periods follow each other, period_days apart"
    );
    for pair in plan.chunks(2) {
        let on = pair.iter().filter(|p| p.state == LeverState::On).count();
        assert_eq!(on, 1, "one half of every pair is on: {pair:?}");
    }
    assert!(
        p.power.mde > 0.0 && p.power.mde.is_finite(),
        "mde {}",
        p.power.mde
    );
    assert_eq!(
        (p.assign_by.clone(), p.strata.clone(), p.alpha),
        (None, None, 0.1)
    );
    assert_eq!(
        p.mde_unit_day, p.power.mde,
        "a switchback's MDE is already a fleet mean per unit-day"
    );
}

#[test]
fn the_periods_are_what_the_engine_proposes_for_that_seed() {
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    let engine = propose_switchback(day_ordinal(date(2026, 3, 9)), 7, 6, 8841207);
    assert_eq!(
        periods(&p).1,
        plan_periods(oxy_experiment::Grain::Day, &engine)
            .unwrap()
            .as_slice()
    );
    let again = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert_eq!(periods(&p).1, periods(&again).1);
    assert_eq!(p.power.mde, again.power.mde);
}

#[test]
fn four_pairs_are_refused_at_the_files_alpha_and_priced_at_a_looser_one() {
    // Review Focus 9. Four pairs have 2^4 = 16 relabellings, so the smallest
    // attainable p is 2/16 = 0.125. The engine applies it to the Sidak per-comparison
    // alpha (family 2 in the fixture): 0.1 -> 0.051 and 0.2 -> 0.1056 are refused,
    // 0.3 -> 0.163 is attainable.
    let (reason, _) = refused(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 4),
        &run(),
    ));
    assert!(
        reason.contains("the design could not be priced"),
        "{reason}"
    );

    let mut loose = sb(7, 4);
    loose.alpha = Some(0.3);
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &loose,
        &run(),
    ));
    assert_eq!(p.alpha, 0.3);
    assert_eq!(periods(&p).1.len(), 8);
}

#[test]
fn a_washout_that_does_not_fit_inside_the_candidates_period_is_refused() {
    // The draft's washout (7) against the candidate's period (7): no day of a
    // period would be settled. The period exists only in the candidate.
    let (reason, problems) = refused(price_switchback(
        &switchback_draft(7),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert!(reason.contains("problem"), "{reason}");
    assert!(
        problems.iter().any(|p| p.contains("washout_days")),
        "{problems:?}"
    );
}

#[test]
fn a_grace_window_that_does_not_fit_inside_the_candidates_period_is_refused() {
    // grace_days is 2 in the fixture.
    let (_, problems) = refused(price_switchback(
        &switchback_draft(0),
        &panel(),
        &sb(2, 6),
        &run(),
    ));
    assert!(
        problems.iter().any(|p| p.contains("grace_days")),
        "{problems:?}"
    );
}

#[test]
fn a_draft_with_waves_keys_prices_as_a_switchback_and_names_them_to_delete() {
    // Review Focus 10. `draft(2)` is a waves draft: pre_days, post_days and a
    // non-zero anticipation_days. Pasting a switchback snippet into it would
    // fail compile with SwitchbackForbids (PR 2).
    let p = priced(price_switchback(&draft(2), &panel(), &sb(7, 6), &run()));
    let w = p
        .warnings
        .iter()
        .find(|w| w.contains("delete"))
        .expect("a delete warning");
    for key in [
        "`design.pre_days`",
        "`design.post_days`",
        "`design.anticipation_days (non-zero)`",
    ] {
        assert!(w.contains(key), "{key} missing from {w}");
    }

    let with_waves = registered(TWO_WAVES, 2);
    let p = priced(price_switchback(&with_waves, &panel(), &sb(7, 6), &run()));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("`design.waves`") && w.contains("delete")),
        "{:?}",
        p.warnings
    );

    let grouped =
        oxy_experiment::parse(&with_assign_by(&yaml("", "", 28, 28, 2), "store.region")).unwrap();
    let p = priced(price_switchback(&grouped, &panel(), &sb(7, 6), &run()));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("`assign_by`") && w.contains("delete")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_switchback_draft_has_nothing_to_delete() {
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert!(
        p.warnings.iter().all(|w| !w.contains("delete")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn periods_that_are_not_whole_weeks_warn() {
    // 14 days x 6 pairs needs more history than the 273-day panel holds, so 14 is 4 pairs at alpha 0.3.
    let fourteen = SwitchbackCandidate {
        alpha: Some(0.3),
        ..sb(14, 4)
    };
    for c in [sb(7, 6), fourteen] {
        let p = priced(price_switchback(&switchback_draft(2), &panel(), &c, &run()));
        assert!(
            p.warnings.iter().all(|w| !w.contains("whole weeks")),
            "{}: {:?}",
            c.period_days,
            p.warnings
        );
    }
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(10, 6),
        &run(),
    ));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("10 days") && w.contains("whole weeks")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn the_standing_assumptions_are_stated_on_every_switchback() {
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("carryover") && w.contains("pull-forward")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn zero_washout_warns_that_every_period_starts_with_a_partial_day() {
    let p = priced(price_switchback(
        &switchback_draft(0),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert!(
        p.warnings.iter().any(|w| w.contains("partial first day")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn the_deadline_warning_uses_the_first_periods_start() {
    // Anticipation is 0 for a switchback, so the deadline is the start of the
    // first period's local day: 2026-03-09 in Sydney (UTC+11) = 2026-03-08T13:00Z.
    let mut late = run();
    late.now = Utc.with_ymd_and_hms(2026, 3, 9, 0, 0, 0).unwrap();
    let p = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &late,
    ));
    let w = p
        .warnings
        .iter()
        .find(|w| w.contains("registration deadline"))
        .expect("a deadline warning");
    assert!(w.contains("2026-03-08 13:00:00 UTC"), "{w}");
    let early = priced(price_switchback(
        &switchback_draft(2),
        &panel(),
        &sb(7, 6),
        &run(),
    ));
    assert!(
        early
            .warnings
            .iter()
            .all(|w| !w.contains("registration deadline"))
    );
}

#[test]
fn the_dispatcher_prices_each_shape_with_its_own_core() {
    let waves = Candidate::Waves(WavesCandidate {
        wave_sizes: vec![4],
        first_switch_on: date(2026, 3, 9).into(),
        spacing_days: 0,
        blocks: None,
        pairs: false,
        assign_by: None,
        alpha: None,
    });
    let w = priced(price_design(&draft(7), &panel(), &waves, &run()));
    assert!(matches!(w.plan, SchedulePlan::Waves(_)));
    let s = priced(price_design(
        &switchback_draft(2),
        &panel(),
        &Candidate::Switchback(sb(7, 6)),
        &run(),
    ));
    assert!(matches!(s.plan, SchedulePlan::Switchback { .. }));
}

#[test]
fn at_hour_grain_a_switchback_may_start_mid_day_but_only_on_a_whole_hour() {
    let mut hourly = switchback_draft(2);
    hourly.design.grain = oxy_experiment::Grain::Hour;
    hourly.lever.as_mut().unwrap().read_back.grace_days = 0;
    hourly.design.washout_days = 1;
    let at = |s: &str| s.parse::<oxy_experiment::Moment>().unwrap();
    let mut four_hourly = sb(4, 6);
    four_hourly.first_period_on = at("2026-03-02T14:00");
    if let PriceComputation::Refused { reason, problems } =
        price_switchback(&hourly, &panel(), &four_hourly, &run())
    {
        assert!(!reason.contains("whole hour"), "{reason}");
        assert!(
            !problems.iter().any(|p| p.contains("whole")),
            "{problems:?}"
        );
    }
    four_hourly.first_period_on = at("2026-03-02T14:30");
    let (reason, _) = refused(price_switchback(&hourly, &panel(), &four_hourly, &run()));
    assert!(reason.contains("is not a whole hour"), "{reason}");
}
