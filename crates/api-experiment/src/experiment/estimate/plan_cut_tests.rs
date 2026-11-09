//! The abort cut: an aborted run's estimate stops reading where the levers went off.

use chrono::NaiveDate;
use oxy_experiment::config::parse;
use oxy_experiment::run::LeverState::{self, Off, On};
use oxy_experiment::timeline::last_post_window_end_date;

use super::plan::{FlipDay, history_end, kept_levers_cut, measured_until};
use super::testdata::{CONFIG_YAML, switchback_config};

fn d(m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, day).unwrap()
}

fn flip(state: LeverState, due_at: NaiveDate, wave: usize) -> FlipDay {
    FlipDay {
        state,
        due_at: due_at.into(),
        wave,
        cancelled: false,
        reached: true,
    }
}

/// Cancelled by the abort: never sent, and the switch never began.
fn cancelled(mut f: FlipDay) -> FlipDay {
    f.cancelled = true;
    f.reached = false;
    f
}

/// Cancelled because its period had ended before dispatch: the period still began.
fn period_ended(mut f: FlipDay) -> FlipDay {
    f.cancelled = true;
    f
}

#[test]
fn an_aborted_waves_run_is_cut_at_its_earliest_abort_off() {
    // Ons out on 2 March; the abort's offs were decided on 19 March (a unit a later tick
    // turned off is dated 20 March). A cancelled off was never sent and decides nothing.
    let cfg = parse(CONFIG_YAML).unwrap();
    let flips = [
        flip(On, d(3, 2), 0),
        flip(On, d(3, 2), 0),
        cancelled(flip(Off, d(3, 10), 0)),
        flip(Off, d(3, 20), 0),
        flip(Off, d(3, 19), 0),
    ];
    let cut = measured_until(&cfg, true, &flips);
    assert_eq!(cut, Some(d(3, 19)));
    assert_eq!(history_end(&cfg, cut), Some(d(3, 19)));
}

#[test]
fn a_run_that_was_not_aborted_reads_to_its_registered_end() {
    let cfg = parse(CONFIG_YAML).unwrap();
    // A close's offs are not an abort.
    let flips = [flip(On, d(3, 2), 0), flip(Off, d(5, 4), 0)];
    assert_eq!(measured_until(&cfg, false, &flips), None);
    assert_eq!(history_end(&cfg, None), last_post_window_end_date(&cfg));
}

#[test]
fn a_switchbacks_scheduled_offs_are_not_the_abort() {
    // Periods: 2 Mar on, 9 Mar off, 16 Mar off, 23 Mar on. The 9 March offs are period 1's own
    // boundary; the abort turned off period 3's `on` on 25 March (its offs carry the on's index).
    let cfg = switchback_config();
    let flips = [
        flip(On, d(3, 2), 0),
        flip(Off, d(3, 9), 1),
        flip(Off, d(3, 16), 2),
        flip(On, d(3, 23), 3),
        flip(Off, d(3, 25), 3),
    ];
    assert_eq!(measured_until(&cfg, true, &flips), Some(d(3, 25)));
}

#[test]
fn a_switchback_aborted_in_an_off_period_is_cut_at_the_first_boundary_it_never_reached() {
    // Nothing was on to turn off, so there is no abort off. The levers never moved again: the
    // 23 March `on` was never recorded, so nothing from then on was treated.
    let cfg = switchback_config();
    let flips = [
        flip(On, d(3, 2), 0),
        flip(Off, d(3, 9), 1),
        flip(Off, d(3, 16), 2),
    ];
    assert_eq!(measured_until(&cfg, true, &flips), Some(d(3, 23)));
}

#[test]
fn a_boundary_cancelled_because_its_period_ended_still_counts_as_reached() {
    // An outage: the 9 March offs were cancelled with PERIOD_ENDED, but that period began
    // (PR 4's rule). Aborted in the 16 March off period: the cut is 23 March, not 9 March.
    let cfg = switchback_config();
    let flips = [
        flip(On, d(3, 2), 0),
        period_ended(flip(Off, d(3, 9), 1)),
        flip(Off, d(3, 16), 2),
    ];
    assert_eq!(measured_until(&cfg, true, &flips), Some(d(3, 23)));
}

#[test]
fn a_waves_run_aborted_before_any_on_went_out_is_cut_at_its_first_switch() {
    let cfg = parse(CONFIG_YAML).unwrap();
    // The 2 March ons were recorded but cancelled by the abort, or never recorded at all.
    let undispatched = [cancelled(flip(On, d(3, 2), 0))];
    assert_eq!(measured_until(&cfg, true, &undispatched), Some(d(3, 2)));
    assert_eq!(measured_until(&cfg, true, &[]), Some(d(3, 2)));
}

#[test]
fn an_aborted_run_with_every_switch_reached_and_no_abort_off_reads_to_its_registered_end() {
    let cfg = parse(CONFIG_YAML).unwrap();
    assert_eq!(measured_until(&cfg, true, &[flip(On, d(3, 2), 0)]), None);
}

#[test]
fn the_cut_is_never_later_than_the_registered_end() {
    let cfg = parse(CONFIG_YAML).unwrap();
    let end = last_post_window_end_date(&cfg).unwrap(); // 4 May
    let late = [flip(On, d(3, 2), 0), flip(Off, d(5, 10), 0)];
    assert_eq!(measured_until(&cfg, true, &late), None, "nothing was cut");
    let on_end = [flip(On, d(3, 2), 0), flip(Off, end, 0)];
    assert_eq!(
        measured_until(&cfg, true, &on_end),
        None,
        "exclusive: no day lost"
    );
    assert_eq!(history_end(&cfg, Some(d(5, 10))), Some(end));
    assert_eq!(history_end(&cfg, Some(d(3, 19))), Some(d(3, 19)));
}

#[test]
fn an_abort_that_kept_its_levers_is_cut_on_the_day_it_was_aborted() {
    // No abort `off` was sent: the levers stayed where they were, so the flips cannot say
    // where the run ended. The recorded abort day does; it and later are not read.
    let cfg = parse(CONFIG_YAML).unwrap();
    assert_eq!(kept_levers_cut(&cfg, d(3, 19)), Some(d(3, 19)));
    let end = last_post_window_end_date(&cfg).unwrap();
    assert_eq!(kept_levers_cut(&cfg, end), None, "exclusive: no day lost");
    assert_eq!(kept_levers_cut(&cfg, d(5, 10)), None);
}
