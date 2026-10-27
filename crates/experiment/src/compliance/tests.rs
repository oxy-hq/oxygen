use std::collections::BTreeMap;

use super::*;
use crate::test_fixtures::{cfg, cfg_with};

/// A day as read-back samples it: its midnight.
fn d(s: &str) -> Moment {
    Moment::midnight(crate::test_fixtures::d(s))
}

fn rec(pairs: &[(&str, FlipOutcome)]) -> Vec<(String, FlipOutcome)> {
    pairs
        .iter()
        .map(|(u, o)| (u.to_string(), o.clone()))
        .collect()
}

fn landed() -> FlipOutcome {
    FlipOutcome::Landed {
        on_day: d("2026-03-03"),
    }
}

fn failed() -> FlipOutcome {
    FlipOutcome::Failed {
        reason: "lever automation missing".into(),
    }
}

fn leaked() -> FlipOutcome {
    FlipOutcome::LeakedToControl {
        on_day: d("2026-03-04"),
    }
}

#[test]
fn c_treated_is_landed_over_treated_units_recorded() {
    let s = summarize(
        &cfg(),
        &rec(&[
            ("bondi", landed()),
            ("newtown", landed()),
            ("manly", landed()),
            ("surry_hills", FlipOutcome::NeverLanded),
        ]),
    );
    assert_eq!(s.c_treated, 0.75);
}

#[test]
fn a_failed_flip_counts_as_not_landed() {
    let s = summarize(&cfg(), &rec(&[("bondi", landed()), ("newtown", failed())]));
    assert_eq!(s.c_treated, 0.5);
}

#[test]
fn control_leak_is_leaked_over_every_control_recorded() {
    let s = summarize(
        &cfg(),
        &rec(&[
            ("bondi", landed()),
            ("glebe", leaked()),
            ("paddington", FlipOutcome::Clean),
            ("redfern", FlipOutcome::Clean),
            ("marrickville", FlipOutcome::Clean),
        ]),
    );
    assert_eq!(s.c_control_leak, 0.25, "clean controls are the denominator");
    assert_eq!(s.c_treated, 1.0);
}

#[test]
fn restated_mde_divides_the_promise_by_the_compliance_gap() {
    let s = summarize(
        &cfg(),
        &rec(&[
            ("bondi", landed()),
            ("newtown", landed()),
            ("manly", landed()),
            ("surry_hills", failed()),
            ("glebe", leaked()),
            ("paddington", FlipOutcome::Clean),
            ("redfern", FlipOutcome::Clean),
            ("marrickville", FlipOutcome::Clean),
        ]),
    );
    // 17.5 / (0.75 − 0.25)
    assert_eq!(s.restated_mde, Some(35.0));
}

#[test]
fn restated_mde_is_none_when_the_gap_is_not_positive() {
    let even = summarize(
        &cfg(),
        &rec(&[
            ("bondi", landed()),
            ("newtown", failed()),
            ("glebe", leaked()),
            ("paddington", FlipOutcome::Clean),
        ]),
    );
    assert_eq!((even.c_treated, even.c_control_leak), (0.5, 0.5));
    assert_eq!(even.restated_mde, None);

    let draft = cfg_with("  mde_at_proposal: 17.5\n", "");
    let no_promise = summarize(&draft, &rec(&[("bondi", landed())]));
    assert_eq!(no_promise.restated_mde, None);
}

#[test]
fn summarize_with_no_treated_entries_reports_not_measured_counts() {
    // Only controls recorded (e.g. a switchback aborted before its first on period):
    // compliance was not measured. The counts say so; the floor and the restatement stay silent.
    let s = summarize(
        &cfg(),
        &rec(&[
            ("glebe", FlipOutcome::Clean),
            ("redfern", FlipOutcome::Clean),
        ]),
    );
    assert_eq!((s.n_treated, s.n_control), (0, 2));
    assert!(!s.below_floor, "no treated entry is not a failed floor");
    assert_eq!(s.restated_mde, None);

    let none = summarize(&cfg(), &[]);
    assert_eq!((none.n_treated, none.n_control), (0, 0));
    assert!(!none.below_floor);
    assert_eq!(none.restated_mde, None);

    let measured = summarize(&cfg(), &rec(&[("bondi", landed()), ("glebe", leaked())]));
    assert_eq!((measured.n_treated, measured.n_control), (1, 1));
}

#[test]
fn below_floor_compares_treated_compliance_to_compliance_floor() {
    let three_of_four = rec(&[
        ("bondi", landed()),
        ("newtown", landed()),
        ("manly", landed()),
        ("surry_hills", failed()),
    ]);
    assert!(summarize(&cfg(), &three_of_four).below_floor, "0.75 < 0.8");
    let at_floor = cfg_with("compliance_floor: 0.8", "compliance_floor: 0.75");
    assert!(
        !summarize(&at_floor, &three_of_four).below_floor,
        "0.75 is not below 0.75"
    );
}

#[test]
fn outcomes_round_trip_through_json_with_their_days() {
    for o in [
        landed(),
        FlipOutcome::NeverLanded,
        failed(),
        FlipOutcome::Clean,
        leaked(),
    ] {
        let json = serde_json::to_value(&o).unwrap();
        assert_eq!(serde_json::from_value::<FlipOutcome>(json).unwrap(), o);
    }
    assert_eq!(
        serde_json::to_value(leaked()).unwrap(),
        serde_json::json!({ "outcome": "leaked_to_control", "on_day": "2026-03-04" })
    );
}

fn held(days: &[(&str, bool)]) -> BTreeMap<Moment, bool> {
    days.iter().map(|(day, h)| (d(day), *h)).collect()
}

#[test]
fn classify_treated_lands_on_the_first_day_when_on_held() {
    let h = held(&[
        ("2026-03-02", false),
        ("2026-03-03", true),
        ("2026-03-04", true),
    ]);
    assert_eq!(
        classify_treated(d("2026-03-02"), 2, Sampling::Daily, d("2026-03-03"), &h),
        Some(FlipOutcome::Landed {
            on_day: d("2026-03-03")
        })
    );
}

#[test]
fn classify_treated_never_landed_needs_the_whole_grace_window_seen_and_over() {
    let h = held(&[
        ("2026-03-02", false),
        ("2026-03-03", false),
        ("2026-03-04", false),
    ]);
    assert_eq!(
        classify_treated(d("2026-03-02"), 2, Sampling::Daily, d("2026-03-04"), &h),
        None,
        "not over yet"
    );
    assert_eq!(
        classify_treated(d("2026-03-02"), 2, Sampling::Daily, d("2026-03-05"), &h),
        Some(FlipOutcome::NeverLanded)
    );
}

#[test]
fn classify_treated_is_undecided_while_a_grace_day_is_missing() {
    let h = held(&[("2026-03-02", false), ("2026-03-04", false)]);
    assert_eq!(
        classify_treated(d("2026-03-02"), 2, Sampling::Daily, d("2026-03-09"), &h),
        None
    );
}

#[test]
fn a_day_after_grace_does_not_count_as_landing() {
    let h = held(&[
        ("2026-03-02", false),
        ("2026-03-03", false),
        ("2026-03-04", false),
        ("2026-03-05", true),
    ]);
    assert_eq!(
        classify_treated(d("2026-03-02"), 2, Sampling::Daily, d("2026-03-06"), &h),
        Some(FlipOutcome::NeverLanded)
    );
}

#[test]
fn first_leak_is_the_first_day_when_off_failed() {
    let h = held(&[
        ("2026-03-02", true),
        ("2026-03-05", false),
        ("2026-03-09", false),
    ]);
    assert_eq!(first_leak(&h), Some(d("2026-03-05")));
    assert_eq!(first_leak(&held(&[("2026-03-02", true)])), None);
}

#[test]
fn summarize_classifies_by_outcome_so_a_switchback_with_no_waves_counts() {
    // A switchback has no waves: its on periods are treated and its off
    // periods control because of their variants, keyed unit@period.
    let s = summarize(
        &cfg(),
        &rec(&[
            (period_key("bondi", d("2026-03-02")).as_str(), landed()),
            (
                period_key("bondi", d("2026-03-09")).as_str(),
                FlipOutcome::Clean,
            ),
            (
                period_key("glebe", d("2026-03-02")).as_str(),
                FlipOutcome::NeverLanded,
            ),
            (period_key("glebe", d("2026-03-09")).as_str(), leaked()),
        ]),
    );
    assert_eq!((s.c_treated, s.c_control_leak), (0.5, 0.5));
}

#[test]
fn a_period_key_is_the_unit_at_the_periods_first_day() {
    assert_eq!(period_key("bondi", d("2026-03-09")), "bondi@2026-03-09");
}

#[test]
fn summarize_pools_entries_never_group_shares() {
    // 2 of 3 members of east landed and 1 of 1 of west: 3 of 4 — never
    // the mean of group shares, never a per-group threshold.
    let s = summarize(
        &cfg(),
        &rec(&[
            ("bondi", landed()),
            ("manly", landed()),
            ("coogee", failed()),
            ("parramatta", landed()),
            ("glebe", FlipOutcome::Clean),
            ("redfern", leaked()),
        ]),
    );
    assert_eq!((s.c_treated, s.c_control_leak), (0.75, 0.5));
}

#[test]
fn a_groups_share_counts_members_in_their_assigned_state_for_display() {
    assert_eq!(
        share_complied(&[landed(), landed(), FlipOutcome::NeverLanded]),
        Some(2.0 / 3.0)
    );
    assert_eq!(share_complied(&[FlipOutcome::Clean, leaked()]), Some(0.5));
    assert_eq!(share_complied(&[]), None, "no member decided, no share");
}

fn span(from: &str, state: LeverState) -> PeriodSpan {
    let from = d(from);
    PeriodSpan {
        from,
        until: from.plus_hours(7 * 24).unwrap(),
        state,
    }
}

#[test]
fn an_on_period_lands_within_grace_like_a_wave() {
    let h = held(&[("2026-03-02", false), ("2026-03-03", true)]);
    assert_eq!(
        classify_period(
            &span("2026-03-02", LeverState::On),
            2,
            Sampling::Daily,
            d("2026-03-03"),
            &h
        ),
        Some(FlipOutcome::Landed {
            on_day: d("2026-03-03")
        })
    );
}

#[test]
fn an_off_period_leaks_on_the_first_day_from_the_end_of_grace_that_when_off_fails() {
    // `held` is when_off. 9-10 March still on is the off landing within
    // grace; 13 March is the lever back on mid-period.
    let h = held(&[
        ("2026-03-09", false),
        ("2026-03-10", false),
        ("2026-03-11", true),
        ("2026-03-12", true),
        ("2026-03-13", false),
    ]);
    assert_eq!(
        classify_period(
            &span("2026-03-09", LeverState::Off),
            2,
            Sampling::Daily,
            d("2026-03-14"),
            &h
        ),
        Some(FlipOutcome::LeakedToControl {
            on_day: d("2026-03-13")
        })
    );
}

#[test]
fn an_off_period_still_on_at_the_end_of_grace_leaks_that_day() {
    let h = held(&[
        ("2026-03-09", false),
        ("2026-03-10", false),
        ("2026-03-11", false),
    ]);
    assert_eq!(
        classify_period(
            &span("2026-03-09", LeverState::Off),
            2,
            Sampling::Daily,
            d("2026-03-12"),
            &h
        ),
        Some(FlipOutcome::LeakedToControl {
            on_day: d("2026-03-11")
        })
    );
}

#[test]
fn an_off_period_is_clean_once_over_with_every_day_from_grace_seen() {
    let days: Vec<String> = (11..=15).map(|n| format!("2026-03-{n}")).collect();
    let h = held(
        &days
            .iter()
            .map(|day| (day.as_str(), true))
            .collect::<Vec<_>>(),
    );
    let p = span("2026-03-09", LeverState::Off);
    assert_eq!(
        classify_period(&p, 2, Sampling::Daily, d("2026-03-15"), &h),
        None,
        "the period is not over"
    );
    assert_eq!(
        classify_period(&p, 2, Sampling::Daily, d("2026-03-16"), &h),
        Some(FlipOutcome::Clean)
    );
}

#[test]
fn an_off_period_is_undecided_while_a_day_is_missing() {
    // 13 March never arrived: not a leak, and not clean either.
    let h = held(&[
        ("2026-03-11", true),
        ("2026-03-12", true),
        ("2026-03-14", true),
        ("2026-03-15", true),
    ]);
    assert_eq!(
        classify_period(
            &span("2026-03-09", LeverState::Off),
            2,
            Sampling::Daily,
            d("2026-03-20"),
            &h
        ),
        None
    );
}

#[test]
fn hourly_read_back_tells_a_four_hour_off_period_from_the_on_hours_around_it() {
    let h = |s: &str| Moment::parse(s).unwrap();
    // On 00:00–04:00, off 04:00–08:00; when_off holds every hour of the off period.
    let off = PeriodSpan {
        from: h("2026-03-02T04:00"),
        until: h("2026-03-02T08:00"),
        state: LeverState::Off,
    };
    let held: BTreeMap<Moment, bool> = ["04", "05", "06", "07"]
        .iter()
        .map(|hr| (h(&format!("2026-03-02T{hr}:00")), true))
        .collect();
    assert_eq!(
        classify_period(
            &off,
            0,
            Sampling::Hourly(chrono_tz::UTC),
            h("2026-03-02T07:00"),
            &held
        ),
        None,
        "the off period is not over yet"
    );
    assert_eq!(
        classify_period(
            &off,
            0,
            Sampling::Hourly(chrono_tz::UTC),
            h("2026-03-02T08:00"),
            &held
        ),
        Some(FlipOutcome::Clean)
    );
    let mut leaked = held.clone();
    leaked.insert(h("2026-03-02T06:00"), false);
    assert_eq!(
        classify_period(
            &off,
            0,
            Sampling::Hourly(chrono_tz::UTC),
            h("2026-03-02T08:00"),
            &leaked
        ),
        Some(FlipOutcome::LeakedToControl {
            on_day: h("2026-03-02T06:00")
        })
    );
}

/// 2027-03-14 in New York: 02:00 never happens (01:59 is followed by 03:00), so
/// the warehouse returns no reading for it.
fn spring_forward_held(value: bool) -> BTreeMap<Moment, bool> {
    let h = |s: &str| Moment::parse(s).unwrap();
    let start = h("2027-03-14T00:00");
    (0..30)
        .filter_map(|i| start.plus_hours(i))
        .filter(|m| *m != h("2027-03-14T02:00"))
        .map(|m| (m, value))
        .collect()
}

#[test]
fn hourly_read_back_settles_a_flip_whose_grace_crosses_the_spring_forward_hour() {
    let h = |s: &str| Moment::parse(s).unwrap();
    let ny = Sampling::Hourly(chrono_tz::America::New_York);
    assert_eq!(
        classify_treated(
            h("2027-03-14T00:00"),
            1,
            ny,
            h("2027-03-15T06:00"),
            &spring_forward_held(false)
        ),
        Some(FlipOutcome::NeverLanded),
        "02:00 does not exist that night, so no reading is missing"
    );
}

#[test]
fn hourly_read_back_clears_an_off_period_across_the_spring_forward_hour() {
    let h = |s: &str| Moment::parse(s).unwrap();
    let off = PeriodSpan {
        from: h("2027-03-14T00:00"),
        until: h("2027-03-14T06:00"),
        state: LeverState::Off,
    };
    assert_eq!(
        classify_period(
            &off,
            0,
            Sampling::Hourly(chrono_tz::America::New_York),
            h("2027-03-14T06:00"),
            &spring_forward_held(true)
        ),
        Some(FlipOutcome::Clean)
    );
}
