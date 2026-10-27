use chrono::{Days, NaiveDate};

use super::*;
use crate::test_fixtures::{cfg, cfg_with, d, sb_cfg};
use crate::timeline::{last_post_window_end, last_post_window_end_date};

use LeverState::{Off, On};

/// The day the waves tests' recorded `off`s are dated: after both waves.
const OFF_DAY: &str = "2026-03-20";
/// The switchback fixture's schedule, and the fleet its tests pin.
const PERIODS: [(&str, LeverState); 4] = [
    ("2026-03-02", On),
    ("2026-03-09", Off),
    ("2026-03-16", Off),
    ("2026-03-23", On),
];
const FLEET: [&str; 2] = ["bondi", "glebe"];

fn facts(today: NaiveDate) -> RunFacts {
    RunFacts {
        now: today.into(),
        flips: vec![],
        pinned_units: None,
        last_window_coverage: None,
        closing_since: None,
        guardrail_breach: None,
        abort_requested: None,
        abort_levers: AbortLevers::Off,
        estimate_done: false,
    }
}

fn wave_day(wave: usize) -> NaiveDate {
    [d("2026-03-02"), d("2026-03-16")][wave]
}

fn wave_of(unit: &str) -> usize {
    usize::from(!["bondi", "newtown"].contains(&unit))
}

/// A recorded flip of the waves fixture: an `on` due on its wave's date, an
/// `off` on `OFF_DAY`.
fn fact(unit: &str, state: LeverState, dispatched: bool) -> FlipFact {
    let due_at = match state {
        On => wave_day(wave_of(unit)),
        Off => d(OFF_DAY),
    };
    FlipFact {
        unit: unit.to_string(),
        state,
        due_at: due_at.into(),
        dispatched,
        cancelled: false,
    }
}

fn at(unit: &str, state: LeverState, due: &str, dispatched: bool) -> FlipFact {
    FlipFact {
        unit: unit.to_string(),
        state,
        due_at: d(due).into(),
        dispatched,
        cancelled: false,
    }
}

fn all_on() -> Vec<FlipFact> {
    ["bondi", "newtown", "manly", "surry_hills"]
        .iter()
        .map(|u| fact(u, On, true))
        .collect()
}

/// A wave's `on`, due on the wave's date.
fn on(unit: &str, wave: usize) -> RunAction {
    RunAction::DispatchFlip {
        unit: unit.to_string(),
        state: On,
        wave,
        due_at: wave_day(wave).into(),
    }
}

/// A terminal `off` (abort or close), due the day it was decided.
fn off(unit: &str, wave: usize, today: NaiveDate) -> RunAction {
    RunAction::DispatchFlip {
        unit: unit.to_string(),
        state: Off,
        wave,
        due_at: today.into(),
    }
}

/// A switchback period's flip, due on the period's first day.
fn period(unit: &str, state: LeverState, k: usize, from: &str) -> RunAction {
    RunAction::DispatchFlip {
        unit: unit.to_string(),
        state,
        wave: k,
        due_at: d(from).into(),
    }
}

fn to(next: RunState, actions: Vec<RunAction>) -> Transition {
    Transition { next, actions }
}

/// The exclusive end: the first day after the last post window.
fn end() -> NaiveDate {
    last_post_window_end_date(&cfg()).expect("a registrable design has an end")
}

/// A run in `closing` whose close `off`s have all been sent already.
fn closing(today: NaiveDate, coverage: Option<f64>) -> RunFacts {
    let mut flips = all_on();
    flips.extend(
        ["bondi", "newtown", "manly", "surry_hills"]
            .iter()
            .map(|u| fact(u, Off, true)),
    );
    RunFacts {
        flips,
        closing_since: Some(end().into()),
        last_window_coverage: coverage,
        ..facts(today)
    }
}

fn sb_facts(today: &str, flips: Vec<FlipFact>) -> RunFacts {
    RunFacts {
        flips,
        pinned_units: Some(FLEET.map(String::from).to_vec()),
        ..facts(d(today))
    }
}

/// Every fleet unit's flip for periods `0..=last`, all sent.
fn sb_recorded_through(last: usize) -> Vec<FlipFact> {
    PERIODS[..=last]
        .iter()
        .flat_map(|(from, s)| FLEET.iter().map(move |u| at(u, *s, from, true)))
        .collect()
}

#[test]
fn registered_waits_before_the_first_switch_date() {
    let t = step(&cfg(), RunState::Registered, &facts(d("2026-03-01")));
    assert_eq!(t, to(RunState::Registered, vec![]));
}

#[test]
fn registered_goes_live_on_the_first_switch_date_with_that_waves_on_flips_only() {
    let t = step(&cfg(), RunState::Registered, &facts(d("2026-03-02")));
    assert_eq!(
        t,
        to(RunState::Live, vec![on("bondi", 0), on("newtown", 0)])
    );
}

#[test]
fn live_between_waves_dispatches_nothing() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("newtown", On, true)],
        ..facts(d("2026-03-09"))
    };
    assert_eq!(step(&cfg(), RunState::Live, &f), to(RunState::Live, vec![]));
}

#[test]
fn live_dispatches_a_later_wave_on_its_own_date() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("newtown", On, true)],
        ..facts(d("2026-03-16"))
    };
    assert_eq!(
        step(&cfg(), RunState::Live, &f),
        to(RunState::Live, vec![on("manly", 1), on("surry_hills", 1)])
    );
}

#[test]
fn a_flip_already_recorded_is_never_dispatched_again() {
    // Recorded but not yet enqueued is still recorded: re-enqueueing an orphan
    // row is the host's job, never a second DispatchFlip.
    let f = RunFacts {
        flips: vec![fact("bondi", On, false)],
        ..facts(d("2026-03-02"))
    };
    assert_eq!(
        step(&cfg(), RunState::Registered, &f),
        to(RunState::Live, vec![on("newtown", 0)])
    );
}

#[test]
fn a_clock_jump_over_two_waves_dispatches_both_once() {
    let t = step(&cfg(), RunState::Registered, &facts(d("2026-03-20")));
    assert_eq!(
        t,
        to(
            RunState::Live,
            vec![
                on("bondi", 0),
                on("newtown", 0),
                on("manly", 1),
                on("surry_hills", 1)
            ]
        )
    );
    let after = RunFacts {
        flips: all_on(),
        ..facts(d("2026-03-20"))
    };
    assert_eq!(
        step(&cfg(), RunState::Live, &after),
        to(RunState::Live, vec![])
    );
}

#[test]
fn live_stays_live_the_day_before_the_exclusive_end() {
    let f = RunFacts {
        flips: all_on(),
        ..facts(end() - Days::new(1))
    };
    assert_eq!(step(&cfg(), RunState::Live, &f), to(RunState::Live, vec![]));
}

#[test]
fn live_moves_to_closing_on_the_exclusive_end() {
    let f = RunFacts {
        flips: all_on(),
        ..facts(end())
    };
    // The close `off`s go with the move: the estimate may never finish.
    assert_eq!(
        step(&cfg(), RunState::Live, &f),
        to(RunState::Closing, all_close_offs(end()))
    );
}

fn all_close_offs(today: NaiveDate) -> Vec<RunAction> {
    vec![
        off("bondi", 0, today),
        off("newtown", 0, today),
        off("manly", 1, today),
        off("surry_hills", 1, today),
    ]
}

#[test]
fn a_run_stuck_in_closing_with_no_estimate_keeps_its_offs_until_sent() {
    let today = end() + Days::new(20);
    // Nothing sent yet (a crash before the dispatch): closing retries them,
    // and still waits on the estimate.
    let unsent = RunFacts {
        flips: all_on(),
        closing_since: Some(end().into()),
        last_window_coverage: None,
        ..facts(today)
    };
    let t = step(&cfg(), RunState::Closing, &unsent);
    assert_eq!(t.next, RunState::Closing);
    assert_eq!(&t.actions[..4], all_close_offs(today).as_slice());
    // Once recorded, no further offs, however long the estimate is missing.
    let sent = step(&cfg(), RunState::Closing, &closing(today, None));
    assert!(
        !sent
            .actions
            .iter()
            .any(|a| matches!(a, RunAction::DispatchFlip { .. })),
        "{:?}",
        sent.actions
    );
}

#[test]
fn closing_waits_while_coverage_is_below_the_floor() {
    let f = closing(end() + Days::new(3), Some(0.5));
    assert_eq!(
        step(&cfg(), RunState::Closing, &f),
        to(RunState::Closing, vec![])
    );
}

#[test]
fn closing_with_no_coverage_measured_waits_like_a_short_window() {
    let f = closing(end() + Days::new(3), None);
    assert_eq!(
        step(&cfg(), RunState::Closing, &f),
        to(RunState::Closing, vec![])
    );
}

#[test]
fn closing_enqueues_the_estimate_once_coverage_reaches_the_floor() {
    let f = closing(end() + Days::new(1), Some(0.9));
    assert_eq!(
        step(&cfg(), RunState::Closing, &f),
        to(RunState::Closing, vec![RunAction::EnqueueEstimate])
    );
}

#[test]
fn closing_gives_up_waiting_after_the_max_wait_and_says_why() {
    let day_13 = closing(end() + Days::new(13), Some(0.5));
    assert_eq!(
        step(&cfg(), RunState::Closing, &day_13),
        to(RunState::Closing, vec![])
    );

    let day_14 = closing(end() + Days::new(CLOSING_MAX_WAIT_DAYS as u64), Some(0.5));
    let t = step(&cfg(), RunState::Closing, &day_14);
    assert_eq!(t.next, RunState::Closing);
    assert_eq!(t.actions[0], RunAction::EnqueueEstimate);
    let RunAction::NotifyOwner { reason } = &t.actions[1] else {
        panic!("expected a notice, got {:?}", t.actions);
    };
    assert!(reason.contains("coverage_floor 0.90"), "{reason}");
    assert!(reason.contains("14 days"), "{reason}");
    // Stable text, so the host's (run, reason) dedupe sends it once.
    let day_15 = closing(end() + Days::new(15), Some(0.6));
    assert_eq!(step(&cfg(), RunState::Closing, &day_15).actions, t.actions);
}

#[test]
fn closing_reports_when_the_estimate_is_done_without_repeating_sent_offs() {
    let today = end() + Days::new(2);
    let f = RunFacts {
        estimate_done: true,
        ..closing(today, Some(1.0))
    };
    assert_eq!(
        step(&cfg(), RunState::Closing, &f),
        to(RunState::Reported, vec![])
    );
}

#[test]
fn on_close_keep_reports_without_flipping() {
    let keep = cfg_with("on_close: off", "on_close: keep");
    let f = RunFacts {
        estimate_done: true,
        ..closing(end() + Days::new(2), Some(1.0))
    };
    assert_eq!(
        step(&keep, RunState::Closing, &f),
        to(RunState::Reported, vec![])
    );
}

#[test]
fn closing_turns_off_only_units_still_missing_an_off_flip() {
    let today = end() + Days::new(5);
    let mut flips = all_on();
    flips.push(fact("bondi", Off, true));
    flips.push(fact("manly", Off, false));
    let f = RunFacts {
        flips,
        closing_since: Some(end().into()),
        ..facts(today)
    };
    assert_eq!(
        step(&cfg(), RunState::Closing, &f).actions,
        vec![off("newtown", 0, today), off("surry_hills", 1, today)]
    );
}

#[test]
fn reported_emits_nothing_even_with_offs_unsent() {
    let f = RunFacts {
        flips: all_on(),
        ..facts(end() + Days::new(5))
    };
    assert_eq!(
        step(&cfg(), RunState::Reported, &f),
        to(RunState::Reported, vec![])
    );
}

#[test]
fn a_treated_unit_whose_on_failed_still_gets_its_close_off() {
    let today = end() + Days::new(5);
    let flips = vec![
        fact("bondi", On, false),
        fact("newtown", On, true),
        fact("manly", Off, true),
        fact("surry_hills", Off, true),
    ];
    let f = RunFacts {
        flips,
        ..facts(today)
    };
    assert_eq!(
        step(&cfg(), RunState::Closing, &f).actions,
        vec![off("bondi", 0, today), off("newtown", 0, today)]
    );
}

#[test]
fn breach_while_live_aborts_and_turns_off_dispatched_units_only() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("newtown", On, false)],
        guardrail_breach: Some("refund_rate spiked".into()),
        ..facts(d("2026-03-20"))
    };
    assert_eq!(
        step(&cfg(), RunState::Live, &f),
        to(
            RunState::Aborted,
            vec![
                off("bondi", 0, d("2026-03-20")),
                RunAction::NotifyOwner {
                    reason: "guardrail breached: refund_rate spiked".into()
                },
            ]
        )
    );
}

#[test]
fn abort_request_while_registered_aborts_with_nothing_to_turn_off() {
    let f = RunFacts {
        abort_requested: Some("owner call".into()),
        ..facts(d("2026-02-20"))
    };
    assert_eq!(
        step(&cfg(), RunState::Registered, &f),
        to(
            RunState::Aborted,
            vec![RunAction::NotifyOwner {
                reason: "aborted on request: owner call".into()
            }]
        )
    );
}

#[test]
fn a_breach_on_a_switch_date_wins_over_the_switch() {
    let f = RunFacts {
        guardrail_breach: Some("x".into()),
        ..facts(d("2026-03-02"))
    };
    let t = step(&cfg(), RunState::Registered, &f);
    assert_eq!(t.next, RunState::Aborted);
    assert!(
        !t.actions
            .iter()
            .any(|a| matches!(a, RunAction::DispatchFlip { state: On, .. })),
        "{:?}",
        t.actions
    );
}

#[test]
fn aborted_is_terminal_a_later_breach_does_nothing() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("bondi", Off, true)],
        guardrail_breach: Some("again".into()),
        abort_requested: Some("again".into()),
        estimate_done: true,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![])
    );
}

#[test]
fn aborted_finishes_an_off_it_had_not_recorded_yet_without_a_second_notice() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true)],
        guardrail_breach: Some("x".into()),
        estimate_done: true,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![off("bondi", 0, d("2026-03-25"))])
    );
}

#[test]
fn a_terminal_off_recorded_on_an_earlier_day_is_not_sent_again() {
    // A terminal off is dated the day it was decided; tomorrow's step must
    // still see it, so the test is "an off since the last on", not a date match.
    let f = RunFacts {
        flips: vec![
            fact("bondi", On, true),
            at("bondi", Off, "2026-03-24", false),
        ],
        estimate_done: true,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![])
    );
}

#[test]
fn a_cancelled_scheduled_off_does_not_suppress_the_terminal_off() {
    // A switchback period's `off` that was cancelled (its period ended before
    // it was sent) never reached the lever: the unit is still on, so an abort
    // must still turn it off.
    let cancelled_off = FlipFact {
        cancelled: true,
        ..at("bondi", Off, "2026-03-24", false)
    };
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), cancelled_off],
        estimate_done: true,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![off("bondi", 0, d("2026-03-25"))])
    );
}

#[test]
fn an_aborted_run_with_no_estimate_enqueues_it_after_its_offs() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true)],
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(
            RunState::Aborted,
            vec![off("bondi", 0, d("2026-03-25")), RunAction::EnqueueEstimate]
        )
    );
}

#[test]
fn an_aborted_run_whose_estimate_is_done_enqueues_nothing() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("bondi", Off, true)],
        estimate_done: true,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![])
    );
}

#[test]
fn the_step_into_aborted_does_not_enqueue_the_estimate() {
    // The job could be claimed before the host writes `aborted`, and the
    // executor refuses a `live` run: the next step, in `aborted`, enqueues it.
    let f = RunFacts {
        flips: vec![fact("bondi", On, true)],
        abort_requested: Some("owner call".into()),
        ..facts(d("2026-03-20"))
    };
    let t = step(&cfg(), RunState::Live, &f);
    assert_eq!(t.next, RunState::Aborted);
    assert!(!t.actions.contains(&RunAction::EnqueueEstimate), "{t:?}");
}

#[test]
fn reported_is_terminal_a_later_breach_does_nothing() {
    let mut flips = all_on();
    for u in ["bondi", "newtown", "manly", "surry_hills"] {
        flips.push(fact(u, Off, true));
    }
    let f = RunFacts {
        flips,
        guardrail_breach: Some("late".into()),
        ..facts(end() + Days::new(9))
    };
    assert_eq!(
        step(&cfg(), RunState::Reported, &f),
        to(RunState::Reported, vec![])
    );
}

#[test]
fn closing_ignores_breach_and_abort() {
    let f = RunFacts {
        guardrail_breach: Some("x".into()),
        abort_requested: Some("y".into()),
        ..closing(end() + Days::new(1), None)
    };
    assert_eq!(
        step(&cfg(), RunState::Closing, &f),
        to(RunState::Closing, vec![])
    );
}

#[test]
fn a_switchback_goes_live_on_its_date_but_flips_nothing_until_the_fleet_is_pinned() {
    let t = step(&sb_cfg(), RunState::Registered, &facts(d("2026-03-02")));
    assert_eq!(t, to(RunState::Live, vec![]));
}

#[test]
fn a_switchback_flips_every_pinned_unit_with_the_period_index_as_wave() {
    let t = step(
        &sb_cfg(),
        RunState::Registered,
        &sb_facts("2026-03-02", vec![]),
    );
    assert_eq!(
        t,
        to(
            RunState::Live,
            vec![
                period("bondi", On, 0, "2026-03-02"),
                period("glebe", On, 0, "2026-03-02")
            ]
        )
    );
    let next = sb_facts("2026-03-09", sb_recorded_through(0));
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &next).actions,
        vec![
            period("bondi", Off, 1, "2026-03-09"),
            period("glebe", Off, 1, "2026-03-09")
        ]
    );
}

#[test]
fn a_switchback_boundary_that_keeps_the_state_still_sends_it() {
    // off, off: period 2 gets its own row, so read-back is per period.
    let f = sb_facts("2026-03-16", sb_recorded_through(1));
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &f).actions,
        vec![
            period("bondi", Off, 2, "2026-03-16"),
            period("glebe", Off, 2, "2026-03-16")
        ]
    );
}

#[test]
fn a_switchback_turns_a_unit_on_again_in_a_later_on_period() {
    let f = sb_facts("2026-03-23", sb_recorded_through(2));
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &f).actions,
        vec![
            period("bondi", On, 3, "2026-03-23"),
            period("glebe", On, 3, "2026-03-23")
        ],
        "an on recorded on 2 March does not stand for the on due on 23 March"
    );
    let repeated = sb_facts("2026-03-23", sb_recorded_through(3));
    assert_eq!(step(&sb_cfg(), RunState::Live, &repeated).actions, vec![]);
}

#[test]
fn a_clock_jump_over_switchback_periods_records_every_elapsed_boundary_once() {
    // Recorded, not replayed: the host cancels the ended periods (Task 9a).
    let f = RunFacts {
        pinned_units: Some(vec!["bondi".into()]),
        ..facts(d("2026-03-17"))
    };
    assert_eq!(
        step(&sb_cfg(), RunState::Registered, &f),
        to(
            RunState::Live,
            vec![
                period("bondi", On, 0, "2026-03-02"),
                period("bondi", Off, 1, "2026-03-09"),
                period("bondi", Off, 2, "2026-03-16"),
            ]
        )
    );
}

#[test]
fn a_switchback_closes_the_day_after_its_last_period() {
    let end = last_post_window_end(&sb_cfg()).expect("a registrable switchback has an end");
    assert_eq!(
        end,
        d("2026-03-30"),
        "the last period starts 23 March and lasts 7 days"
    );
    let before = sb_facts("2026-03-29", sb_recorded_through(3));
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &before),
        to(RunState::Live, vec![])
    );
    let on_end = sb_facts("2026-03-30", sb_recorded_through(3));
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &on_end),
        to(
            RunState::Closing,
            vec![
                off("bondi", 3, d("2026-03-30")),
                off("glebe", 3, d("2026-03-30"))
            ]
        )
    );
}

#[test]
fn a_reported_switchback_turns_off_only_units_its_last_period_left_on() {
    let today = d("2026-04-02");
    let f = RunFacts {
        estimate_done: true,
        ..sb_facts("2026-04-02", sb_recorded_through(3))
    };
    assert_eq!(
        step(&sb_cfg(), RunState::Closing, &f),
        to(
            RunState::Reported,
            vec![off("bondi", 3, today), off("glebe", 3, today)]
        )
    );
    let left_off = sb_facts("2026-04-02", sb_recorded_through(2));
    assert_eq!(
        step(&sb_cfg(), RunState::Closing, &left_off).actions,
        vec![],
        "an off on or after the last on means the unit is already off"
    );
}

#[test]
fn a_switchback_abort_turns_off_units_whose_latest_on_was_sent() {
    let flips = vec![
        at("bondi", On, "2026-03-02", true),
        at("glebe", On, "2026-03-02", false),
    ];
    let f = RunFacts {
        abort_requested: Some("owner call".into()),
        ..sb_facts("2026-03-04", flips)
    };
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &f),
        to(
            RunState::Aborted,
            vec![
                off("bondi", 0, d("2026-03-04")),
                RunAction::NotifyOwner {
                    reason: "aborted on request: owner call".into()
                },
            ]
        )
    );
}

#[test]
fn a_switchback_abort_ignores_a_cancelled_latest_on_and_turns_off_the_earlier_sent_one() {
    // The later period's `on` never left (cancelled): the lever is still on
    // from the earlier, sent one, so the abort must turn it off.
    let cancelled_on = FlipFact {
        cancelled: true,
        ..at("bondi", On, "2026-03-23", false)
    };
    let f = RunFacts {
        flips: vec![at("bondi", On, "2026-03-02", true), cancelled_on],
        estimate_done: true,
        ..sb_facts("2026-03-25", vec![])
    };
    assert_eq!(
        step(&sb_cfg(), RunState::Aborted, &f).actions,
        vec![off("bondi", 0, d("2026-03-25"))]
    );
}

#[test]
fn a_switchback_aborted_in_an_off_period_has_nothing_to_turn_off() {
    let f = RunFacts {
        guardrail_breach: Some("x".into()),
        ..sb_facts("2026-03-11", sb_recorded_through(1))
    };
    assert_eq!(
        step(&sb_cfg(), RunState::Live, &f).actions,
        vec![RunAction::NotifyOwner {
            reason: "guardrail breached: x".into()
        }]
    );
}

#[test]
fn switch_days_are_the_wave_dates_or_the_period_starts() {
    assert_eq!(
        switch_moments(&cfg()),
        vec![d("2026-03-02"), d("2026-03-16")]
    );
    assert_eq!(
        switch_moments(&sb_cfg()),
        PERIODS.iter().map(|(from, _)| d(from)).collect::<Vec<_>>()
    );
}

#[test]
fn states_round_trip_through_their_column_strings() {
    for s in [
        RunState::Registered,
        RunState::Live,
        RunState::Closing,
        RunState::Reported,
        RunState::Aborted,
    ] {
        assert_eq!(RunState::parse(s.as_str()), Some(s));
    }
    assert_eq!(RunState::parse("refused"), None);
    assert_eq!(LeverState::parse(On.as_str()), Some(On));
    assert_eq!(LeverState::parse(Off.as_str()), Some(Off));
    assert!(RunState::Reported.is_terminal() && RunState::Aborted.is_terminal());
    assert!(!RunState::Closing.is_terminal());
}

#[test]
fn a_retrospective_run_estimates_at_once_and_never_flips_a_lever() {
    // No flip recorded and coverage never measured: a live run in closing would
    // send every treated unit's `off` and wait on coverage. A retrospective one
    // moved no lever, so it sends none and waits on nothing.
    let f = facts(end() + Days::new(40));
    assert_eq!(
        step_retrospective(RunState::Closing, &f),
        to(RunState::Closing, vec![RunAction::EnqueueEstimate])
    );
}

#[test]
fn a_retrospective_run_is_reported_once_its_estimate_is_done() {
    let f = RunFacts {
        estimate_done: true,
        ..facts(end() + Days::new(40))
    };
    assert_eq!(
        step_retrospective(RunState::Closing, &f),
        to(RunState::Reported, vec![])
    );
    assert_eq!(
        step_retrospective(RunState::Reported, &f),
        to(RunState::Reported, vec![])
    );
}

#[test]
fn a_retrospective_run_in_any_other_state_does_nothing() {
    // Unreachable (a retrospective run is created closing), and if it ever were
    // reached, nothing it could do is a lever action.
    let f = facts(end() + Days::new(40));
    for state in [RunState::Registered, RunState::Live, RunState::Aborted] {
        assert_eq!(step_retrospective(state, &f), to(state, vec![]));
    }
}

#[test]
fn an_owner_abort_that_keeps_the_levers_sends_no_off_and_still_notifies() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true), fact("manly", On, true)],
        abort_requested: Some("owner call".into()),
        abort_levers: AbortLevers::Keep,
        ..facts(d("2026-03-20"))
    };
    assert_eq!(
        step(&cfg(), RunState::Live, &f),
        to(
            RunState::Aborted,
            vec![RunAction::NotifyOwner {
                reason: "aborted on request, levers left as they were: owner call".into()
            }]
        )
    );
    assert_eq!(abort_levers(&f), AbortLevers::Keep);
}

#[test]
fn a_breach_turns_the_levers_off_even_when_the_owner_asked_to_keep_them() {
    let f = RunFacts {
        flips: vec![fact("bondi", On, true)],
        guardrail_breach: Some("refund_rate spiked".into()),
        abort_requested: Some("owner call".into()),
        abort_levers: AbortLevers::Keep,
        ..facts(d("2026-03-20"))
    };
    assert_eq!(
        step(&cfg(), RunState::Live, &f).actions,
        vec![
            off("bondi", 0, d("2026-03-20")),
            RunAction::NotifyOwner {
                reason: "guardrail breached: refund_rate spiked".into()
            },
        ]
    );
    assert_eq!(abort_levers(&f), AbortLevers::Off, "what the host records");
}

#[test]
fn a_run_aborted_keeping_its_levers_never_turns_them_off_later() {
    // A check that was already running can record a breach after the abort: the
    // abort kept the levers, and the run no longer manages them.
    let f = RunFacts {
        flips: vec![fact("bondi", On, true)],
        guardrail_breach: Some("late".into()),
        abort_levers: AbortLevers::Keep,
        ..facts(d("2026-03-25"))
    };
    assert_eq!(
        step(&cfg(), RunState::Aborted, &f),
        to(RunState::Aborted, vec![RunAction::EnqueueEstimate])
    );
}

#[test]
fn units_on_are_the_units_an_abort_would_turn_off() {
    let flips = vec![
        fact("bondi", On, true),
        fact("newtown", On, false),
        fact("manly", On, true),
        fact("manly", Off, true),
    ];
    assert_eq!(units_on(&cfg(), &flips), vec!["bondi".to_string()]);
    let sb = vec![
        at("bondi", On, "2026-03-02", true),
        at("glebe", On, "2026-03-02", false),
    ];
    assert_eq!(units_on(&sb_cfg(), &sb), vec!["bondi".to_string()]);
}

#[test]
fn abort_levers_round_trip_and_default_to_off() {
    for l in [AbortLevers::Off, AbortLevers::Keep] {
        assert_eq!(AbortLevers::parse(l.as_str()), Some(l));
    }
    assert_eq!(AbortLevers::parse("sideways"), None);
    assert_eq!(AbortLevers::default(), AbortLevers::Off);
}

fn with_carryover(days: u32) -> ExperimentConfig {
    let mut c = cfg();
    c.design.carryover_days = Some(days);
    c
}

#[test]
fn closing_with_carryover_waits_until_the_carryover_window_has_passed() {
    // The levers went off at close; the effect that lingers is read over the next 14 days.
    let c = with_carryover(14);
    let day_1 = closing(end() + Days::new(1), Some(1.0));
    assert_eq!(
        step(&c, RunState::Closing, &day_1),
        to(RunState::Closing, vec![])
    );
    let day_13 = closing(end() + Days::new(13), Some(1.0));
    assert_eq!(
        step(&c, RunState::Closing, &day_13),
        to(RunState::Closing, vec![])
    );
    // Yesterday was the last carryover day: it has arrived.
    let day_14 = closing(end() + Days::new(14), Some(1.0));
    assert_eq!(
        step(&c, RunState::Closing, &day_14),
        to(RunState::Closing, vec![RunAction::EnqueueEstimate])
    );
}

#[test]
fn the_closing_max_wait_starts_after_the_carryover_window() {
    let c = with_carryover(14);
    let waiting = closing(end() + Days::new(14 + 13), Some(0.5));
    assert_eq!(
        step(&c, RunState::Closing, &waiting),
        to(RunState::Closing, vec![])
    );
    let gave_up = closing(
        end() + Days::new(14 + CLOSING_MAX_WAIT_DAYS as u64),
        Some(0.5),
    );
    assert_eq!(
        step(&c, RunState::Closing, &gave_up).actions[0],
        RunAction::EnqueueEstimate
    );
}

/// The switchback fixture switching every 4 hours from 2026-03-02 00:00.
fn hourly_sb() -> ExperimentConfig {
    let mut c = sb_cfg();
    c.design.grain = crate::Grain::Hour;
    c.design.washout_days = 1;
    c.lever.as_mut().unwrap().read_back.grace_days = 0;
    let sb = c.design.switchback.as_mut().unwrap();
    sb.period_days = 4;
    for (p, h) in sb.periods.iter_mut().zip(["00", "04", "08", "12"]) {
        p.from = Moment::parse(&format!("2026-03-02T{h}:00")).unwrap();
    }
    c
}

fn hour(s: &str) -> Moment {
    Moment::parse(s).unwrap()
}

#[test]
fn an_hourly_switchback_flips_at_each_hour_boundary_once_it_is_reached() {
    let c = hourly_sb();
    let f = |now: &str| RunFacts {
        pinned_units: Some(FLEET.iter().map(|u| u.to_string()).collect()),
        now: hour(now),
        ..facts(d("2026-03-02"))
    };
    let dues = |t: &Transition| -> Vec<String> {
        t.actions
            .iter()
            .filter_map(|a| match a {
                RunAction::DispatchFlip { due_at, .. } => Some(due_at.to_string()),
                _ => None,
            })
            .collect()
    };
    let t = step(&c, RunState::Live, &f("2026-03-02T03:00"));
    assert_eq!(
        dues(&t),
        ["2026-03-02", "2026-03-02"],
        "only the first period so far"
    );
    let t = step(&c, RunState::Live, &f("2026-03-02T04:00"));
    assert!(
        dues(&t).contains(&"2026-03-02T04:00".to_string()),
        "{:?}",
        dues(&t)
    );
}

#[test]
fn an_hourly_switchback_closes_at_the_hour_its_last_period_ends() {
    let c = hourly_sb();
    assert_eq!(last_post_window_end(&c), Some(hour("2026-03-02T16:00")));
    let at = |now: &str| RunFacts {
        now: hour(now),
        ..facts(d("2026-03-02"))
    };
    assert_eq!(
        step(&c, RunState::Live, &at("2026-03-02T15:00")).next,
        RunState::Live
    );
    assert_eq!(
        step(&c, RunState::Live, &at("2026-03-02T16:00")).next,
        RunState::Closing
    );
}

#[test]
fn a_wave_switching_at_two_in_the_afternoon_is_not_due_at_one() {
    let mut c = cfg();
    c.design.grain = crate::Grain::Hour;
    c.design.waves[0].switch_on = hour("2026-03-02T14:00");
    let t = |now: &str| {
        step(
            &c,
            RunState::Registered,
            &RunFacts {
                now: hour(now),
                ..facts(d("2026-03-02"))
            },
        )
    };
    assert_eq!(t("2026-03-02T13:00").next, RunState::Registered);
    assert_eq!(t("2026-03-02T14:00").next, RunState::Live);
}
