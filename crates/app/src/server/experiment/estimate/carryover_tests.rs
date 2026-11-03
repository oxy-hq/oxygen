//! The carryover window: the days after the close `off`s, read when the design asks.

use chrono::NaiveDate;
use oxy_airlayer_compat::engine::experiment::{PanelMatrix, day_ordinal};
use oxy_experiment::config::parse;
use oxy_experiment::run::LeverState::{Off, On};

use super::carryover::{carryover_effect, carryover_window};
use super::plan::FlipDay;
use super::testdata::CONFIG_YAML;
use crate::server::experiment::assignment::to_design;

fn d(m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, day).unwrap()
}

fn flip(state: oxy_experiment::run::LeverState, due_at: NaiveDate, cancelled: bool) -> FlipDay {
    FlipDay {
        state,
        due_at: due_at.into(),
        wave: 0,
        cancelled,
        reached: !cancelled,
    }
}

fn cfg(carryover: Option<u32>) -> oxy_experiment::ExperimentConfig {
    let mut c = parse(CONFIG_YAML).unwrap();
    c.design.carryover_days = carryover;
    c
}

#[test]
fn the_window_starts_at_the_earliest_close_off_and_runs_the_registered_days() {
    let flips = [
        flip(On, d(3, 2), false),
        flip(Off, d(3, 10), true), // cancelled: never sent
        flip(Off, d(5, 6), false),
        flip(Off, d(5, 5), false),
    ];
    assert_eq!(
        carryover_window(&cfg(Some(14)), false, &flips),
        Some((d(5, 5).into(), d(5, 19)))
    );
}

#[test]
fn no_window_without_carryover_days_an_off_or_when_aborted() {
    let flips = [flip(On, d(3, 2), false), flip(Off, d(5, 5), false)];
    assert_eq!(carryover_window(&cfg(None), false, &flips), None);
    assert_eq!(
        carryover_window(&cfg(Some(14)), false, &flips[..1]),
        None,
        "the levers never went off"
    );
    assert_eq!(
        carryover_window(&cfg(Some(14)), true, &flips),
        None,
        "an abort reads no carryover"
    );
}

#[test]
fn a_retrospective_starts_the_window_at_the_declared_switch_off() {
    // Nothing was switched, so no flip went out: the off is the one the design declares.
    let mut c = cfg(Some(14));
    c.design.assignment = oxy_experiment::config::Assignment::Observed;
    c.design.waves[0].switch_off = Some(d(5, 5).into());
    assert_eq!(
        carryover_window(&c, false, &[]),
        Some((d(5, 5).into(), d(5, 19)))
    );
    c.design.waves[0].switch_off = None;
    assert_eq!(carryover_window(&c, false, &[]), None, "no off declared");
}

/// Six stores, switched on 2 March (s01..s03 treated), off on 5 May. Treated stores sit +20
/// while on and +8 for the carryover window after the off, controls flat at 100.
fn panel(c: &oxy_experiment::ExperimentConfig, off: NaiveDate, carry: u32) -> PanelMatrix {
    let switch = c.design.waves[0].switch_on.date();
    let mut triples = Vec::new();
    for (i, unit) in ["s01", "s02", "s03", "s07", "s08", "s09"]
        .iter()
        .enumerate()
    {
        let treated = i < 3;
        let mut day = switch - chrono::Days::new(120);
        while day < off + chrono::Days::new(u64::from(carry)) {
            let lift = match (treated, day >= switch, day >= off) {
                (true, _, true) => 8.0,
                (true, true, false) => 20.0,
                _ => 0.0,
            };
            let noise = ((day_ordinal(day) * 7 + i as i64 * 13) % 5) as f64;
            triples.push((unit.to_string(), day_ordinal(day), 100.0 + lift + noise));
            day = day + chrono::Days::new(1);
        }
    }
    PanelMatrix::from_triples(triples)
}

#[test]
fn the_carryover_effect_reads_the_days_after_the_off_against_the_same_baseline() {
    let c = cfg(Some(14));
    let pool: Vec<String> = ["s07", "s08", "s09"].map(String::from).to_vec();
    let design = to_design(&c, &super::plan::assignment_units(&c, &pool));
    let off = d(5, 5);
    let e = carryover_effect(&c, &design, &panel(&c, off, 14), off.into(), 7, false);
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    let est = e.estimate.expect("an estimate");
    assert!(
        (est - 8.0).abs() < 1.5,
        "the carryover lift, not the on-lift: {est}"
    );
}

#[test]
fn a_retrospective_carryover_drops_a_pinned_unit_the_panel_lacks_instead_of_refusing() {
    // s99 is in the control pool but has no rows: the registered read drops it, so must this.
    let c = cfg(Some(14));
    let pool: Vec<String> = ["s07", "s08", "s09", "s99"].map(String::from).to_vec();
    let design = to_design(&c, &super::plan::assignment_units(&c, &pool));
    let off = d(5, 5);
    let m = panel(&c, off, 14);
    let strict = carryover_effect(&c, &design, &m, off.into(), 7, false);
    assert!(strict.refusal.is_some(), "a registered run still refuses");
    let e = carryover_effect(&c, &design, &m, off.into(), 7, true);
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    assert!((e.estimate.expect("an estimate") - 8.0).abs() < 1.5);
}

#[test]
fn a_proportional_outcomes_carryover_is_read_on_the_log_scale() {
    // +8 on ~102 is ln(110/102) ≈ 0.076 on the log scale; the raw read would be ~8.
    let mut c = cfg(Some(14));
    c.outcome.scale = oxy_experiment::config::Scale::Proportional;
    let pool: Vec<String> = ["s07", "s08", "s09"].map(String::from).to_vec();
    let design = to_design(&c, &super::plan::assignment_units(&c, &pool));
    let off = d(5, 5);
    let e = carryover_effect(&c, &design, &panel(&c, off, 14), off.into(), 7, false);
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    let est = e.estimate.expect("an estimate");
    assert!((est - 0.076).abs() < 0.02, "the log-scale lift: {est}");
}

#[test]
fn a_ratio_outcome_reads_no_carryover() {
    let mut c = cfg(Some(14));
    c.outcome.denominator = Some("store_days.orders".into());
    let pool: Vec<String> = ["s07", "s08", "s09"].map(String::from).to_vec();
    let design = to_design(&c, &super::plan::assignment_units(&c, &pool));
    let off = d(5, 5);
    let e = carryover_effect(&c, &design, &panel(&c, off, 14), off.into(), 7, false);
    assert!(
        e.refusal.as_deref().is_some_and(|r| r.contains("ratio")),
        "never the numerator alone: {:?}",
        e.refusal
    );
}

fn at(s: &str) -> oxy_experiment::Moment {
    oxy_experiment::Moment::parse(s).unwrap()
}

/// Hour grain, switched on at 16 March 00:00 (s01..s03 treated): 6 washout hours,
/// 30 post hours, so the close `off` goes out at 17 March 12:00, mid-day.
fn hour_cfg(carryover: u32) -> oxy_experiment::ExperimentConfig {
    let mut c = cfg(Some(carryover));
    c.design.grain = oxy_experiment::Grain::Hour;
    c.design.waves[0].switch_on = at("2026-03-16T00:00");
    c.design.pre_days = Some(48);
    c.design.anticipation_days = 0;
    c.design.washout_days = 6;
    c.design.post_days = Some(30);
    c
}

#[test]
fn at_hour_grain_the_window_starts_at_the_hour_the_levers_went_off() {
    let flips = [
        FlipDay {
            due_at: at("2026-03-16T00:00"),
            ..flip(On, d(3, 16), false)
        },
        FlipDay {
            due_at: at("2026-03-17T12:00"),
            ..flip(Off, d(3, 17), false)
        },
    ];
    assert_eq!(
        carryover_window(&hour_cfg(24), false, &flips),
        Some((at("2026-03-17T12:00"), d(3, 19)))
    );
}

/// Treated stores sit +20 from the switch to the off at 17 March 12:00, then +8 for
/// the 24 carryover hours; controls flat at 100.
fn hour_panel(off: oxy_experiment::Moment) -> PanelMatrix {
    let grain = oxy_experiment::Grain::Hour;
    let switch = grain.ordinal_of(at("2026-03-16T00:00"));
    let off = grain.ordinal_of(off);
    let mut triples = Vec::new();
    for (i, unit) in ["s01", "s02", "s03", "s07", "s08", "s09"]
        .iter()
        .enumerate()
    {
        let treated = i < 3;
        for hour in switch - 96..off + 24 {
            let lift = match (treated, hour >= switch, hour >= off) {
                (true, _, true) => 8.0,
                (true, true, false) => 20.0,
                _ => 0.0,
            };
            let noise = ((hour * 7 + i as i64 * 13) % 5) as f64;
            triples.push((unit.to_string(), hour, 100.0 + lift + noise));
        }
    }
    PanelMatrix::from_triples(triples)
}

#[test]
fn at_hour_grain_the_carryover_effect_reads_the_hours_after_a_mid_day_off() {
    let c = hour_cfg(24);
    let pool: Vec<String> = ["s07", "s08", "s09"].map(String::from).to_vec();
    let design = to_design(&c, &super::plan::assignment_units(&c, &pool));
    let off = at("2026-03-17T12:00");
    let e = carryover_effect(&c, &design, &hour_panel(off), off, 7, false);
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    let est = e.estimate.expect("an estimate");
    assert!(
        (est - 8.0).abs() < 1.5,
        "the carryover lift, not hours still under the on-lift: {est}"
    );
}
