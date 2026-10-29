use std::collections::HashMap;

use chrono::NaiveDate;
use oxy_airlayer_compat::engine::experiment::{
    Assignment, ExperimentDesign, PanelMatrix, Period, SwitchbackSchedule, day_ordinal,
};

use super::series::{PeriodSpan, SeriesShape, daily_series};

/// A day-grain point or window edge.
fn day(s: &str) -> super::series::Moment {
    super::series::Moment {
        at: d(s).and_hms_opt(0, 0, 0).unwrap(),
        hourly: false,
    }
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// `(unit, first date, values)`: one value per consecutive day.
fn panel(rows: &[(&str, &str, &[f64])]) -> PanelMatrix {
    PanelMatrix::from_triples(rows.iter().flat_map(|(unit, from, values)| {
        let start = day_ordinal(d(from));
        values
            .iter()
            .enumerate()
            .map(move |(i, v)| (unit.to_string(), start + i as i64, *v))
    }))
}

fn waves(
    switch: &[(&str, Option<&str>)],
    pre: usize,
    post: usize,
    washout: usize,
) -> ExperimentDesign {
    ExperimentDesign::Waves(Assignment {
        switch_day: switch
            .iter()
            .map(|(u, s)| (u.to_string(), s.map(|s| day_ordinal(d(s)))))
            .collect::<HashMap<_, _>>(),
        strata: None,
        pre_days: pre,
        post_days: post,
        anticipation_days: 1,
        washout_days: washout,
        coverage_floor: 0.8,
        alpha: 0.05,
        family: 1,
        one_date_test: oxy_airlayer_compat::engine::experiment::OneDateTest::Welch,
    })
}

#[test]
fn waves_series_spans_pre_window_to_last_post_day_with_arm_means() {
    // Switch 2026-03-05, anticipation 1, pre 2 → pre window 03-02..03-04;
    // washout 1, post 2 → post window 03-06..03-08. History runs wider both ways.
    let m = panel(&[
        (
            "a",
            "2026-03-01",
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
        ),
        (
            "b",
            "2026-03-01",
            &[3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0],
        ),
        ("c", "2026-03-01", &[10.0; 9]),
    ]);
    let design = waves(
        &[
            ("a", Some("2026-03-05")),
            ("b", Some("2026-03-05")),
            ("c", None),
        ],
        2,
        2,
        1,
    );

    let s = daily_series(
        "store_days.revenue",
        oxy_experiment::Grain::Day,
        &design,
        &m,
    );

    assert_eq!(s.measure, "store_days.revenue");
    let SeriesShape::Waves {
        treated_units,
        control_units,
        waves,
        days,
    } = s.shape
    else {
        panic!("a waves design gives a waves series");
    };
    assert_eq!((treated_units, control_units), (2, 1));
    assert_eq!(waves.len(), 1);
    assert_eq!(waves[0].switch_on, d("2026-03-05"));
    assert_eq!(waves[0].units, 2);
    assert_eq!(
        (waves[0].pre_from.date(), waves[0].pre_to.date()),
        (d("2026-03-02"), d("2026-03-04"))
    );
    assert_eq!(
        (waves[0].post_from.date(), waves[0].post_to.date()),
        (d("2026-03-06"), d("2026-03-08"))
    );
    let dates: Vec<_> = days.iter().map(|r| r.date.date()).collect();
    assert_eq!(dates.first(), Some(&d("2026-03-02")));
    assert_eq!(
        dates.last(),
        Some(&d("2026-03-07")),
        "the post window's end is exclusive"
    );
    assert_eq!(days.len(), 6);
    // 03-02: a=2, b=4 → treated mean 3; control c=10; nobody on yet.
    assert_eq!(days[0].treated, Some(3.0));
    assert_eq!(days[0].control, Some(10.0));
    assert_eq!(days[0].on_units, 0);
    // 03-05 is the switch day: both treated units are on from it.
    let switch = days.iter().find(|r| r.date == d("2026-03-05")).unwrap();
    assert_eq!(switch.on_units, 2);
    assert_eq!(switch.treated, Some(6.0));
}

#[test]
fn staggered_waves_count_units_on_per_day_and_skip_days_the_panel_dropped() {
    // b is missing 03-04, so no unit keeps that day (the panel compares the same days).
    let m = PanelMatrix::from_triples([("a", 1.0), ("b", 2.0), ("c", 5.0)].into_iter().flat_map(
        |(u, v)| {
            (0..6).filter_map(move |i| {
                let date = d("2026-03-01") + chrono::Duration::days(i);
                (u != "b" || date != d("2026-03-04")).then(|| (u.to_string(), day_ordinal(date), v))
            })
        },
    ));
    let design = waves(
        &[
            ("a", Some("2026-03-03")),
            ("b", Some("2026-03-05")),
            ("c", None),
        ],
        1,
        1,
        0,
    );

    let SeriesShape::Waves { waves, days, .. } =
        daily_series("m", oxy_experiment::Grain::Day, &design, &m).shape
    else {
        panic!("waves");
    };
    assert_eq!(
        waves.iter().map(|w| w.switch_on.date()).collect::<Vec<_>>(),
        [d("2026-03-03"), d("2026-03-05")]
    );
    let on: Vec<_> = days.iter().map(|r| (r.date.date(), r.on_units)).collect();
    assert_eq!(
        on,
        [
            (d("2026-03-01"), 0),
            (d("2026-03-02"), 0),
            (d("2026-03-03"), 1),
            (d("2026-03-05"), 2),
        ]
    );
}

#[test]
fn a_unit_missing_from_the_panel_is_left_out_of_its_arm_mean() {
    let m = panel(&[
        ("a", "2026-03-01", &[4.0, 4.0, 4.0]),
        ("c", "2026-03-01", &[1.0, 1.0, 1.0]),
    ]);
    let design = waves(
        &[
            ("a", Some("2026-03-03")),
            ("b", Some("2026-03-03")),
            ("c", None),
        ],
        1,
        1,
        0,
    );
    let SeriesShape::Waves {
        treated_units,
        days,
        ..
    } = daily_series("m", oxy_experiment::Grain::Day, &design, &m).shape
    else {
        panic!("waves");
    };
    assert_eq!(treated_units, 1, "only units the panel holds are counted");
    assert!(days.iter().all(|r| r.treated == Some(4.0)));
}

#[test]
fn switchback_series_marks_on_periods_and_washout_days() {
    let m = panel(&[
        ("s1", "2026-04-01", &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        ("s2", "2026-04-01", &[3.0, 4.0, 5.0, 6.0, 7.0, 8.0]),
    ]);
    let start = day_ordinal(d("2026-04-01"));
    let design = ExperimentDesign::Switchback(SwitchbackSchedule {
        periods: vec![
            Period {
                from_day: start,
                on: true,
            },
            Period {
                from_day: start + 3,
                on: false,
            },
        ],
        period_days: 3,
        washout_days: 1,
        coverage_floor: 0.8,
        alpha: 0.05,
        family: 1,
    });

    let s = daily_series("m", oxy_experiment::Grain::Day, &design, &m);
    let SeriesShape::Switchback {
        fleet_units,
        washout_days,
        periods,
        days,
    } = s.shape
    else {
        panic!("a switchback gives a switchback series");
    };
    assert_eq!((fleet_units, washout_days), (2, 1));
    assert_eq!(
        periods,
        [
            PeriodSpan {
                from: day("2026-04-01"),
                to: day("2026-04-04"),
                on: true
            },
            PeriodSpan {
                from: day("2026-04-04"),
                to: day("2026-04-07"),
                on: false
            },
        ]
    );
    let rows: Vec<_> = days.iter().map(|r| (r.fleet, r.on, r.washout)).collect();
    assert_eq!(
        rows,
        [
            (2.0, true, true),
            (3.0, true, false),
            (4.0, true, false),
            (5.0, false, true),
            (6.0, false, false),
            (7.0, false, false),
        ]
    );
}

#[test]
fn series_serialises_with_a_design_tag_and_iso_dates() {
    let m = panel(&[
        ("a", "2026-03-01", &[1.0, 1.0]),
        ("c", "2026-03-01", &[2.0, 2.0]),
    ]);
    let design = waves(&[("a", Some("2026-03-02")), ("c", None)], 1, 1, 0);
    let mut s = daily_series("m", oxy_experiment::Grain::Day, &design, &m);
    s.measured_until = Some(d("2026-03-02"));
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["design"], "waves");
    assert_eq!(v["measured_until"], "2026-03-02");
    assert_eq!(v["days"][0]["date"], "2026-03-01");
    assert_eq!(v["waves"][0]["switch_on"], "2026-03-02");
}

#[test]
fn a_week_grain_series_dates_each_point_and_window_to_its_monday() {
    use oxy_experiment::Grain;
    let w = |s: &str| Grain::Week.ordinal(d(s));
    // Weekly cells from Monday 2026-03-02; switch Monday 03-23, anticipation 1, pre 2,
    // washout 1, post 2.
    let cells = |unit: &str, values: &[f64]| -> Vec<(String, i64, f64)> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (unit.to_string(), w("2026-03-02") + i as i64, *v))
            .collect()
    };
    let m = PanelMatrix::from_triples(
        cells("a", &[1.0; 9])
            .into_iter()
            .chain(cells("c", &[2.0; 9])),
    );
    let design = ExperimentDesign::Waves(Assignment {
        switch_day: HashMap::from([
            ("a".to_string(), Some(w("2026-03-23"))),
            ("c".to_string(), None),
        ]),
        strata: None,
        pre_days: 2,
        post_days: 2,
        anticipation_days: 1,
        washout_days: 1,
        coverage_floor: 0.8,
        alpha: 0.05,
        family: 1,
        one_date_test: oxy_airlayer_compat::engine::experiment::OneDateTest::Welch,
    });
    let s = daily_series("m", Grain::Week, &design, &m);
    assert_eq!(s.grain, Grain::Week);
    let SeriesShape::Waves { waves, days, .. } = s.shape else {
        panic!("waves");
    };
    assert_eq!(
        (
            waves[0].pre_from.date(),
            waves[0].post_from.date(),
            waves[0].post_to.date()
        ),
        (d("2026-03-02"), d("2026-03-30"), d("2026-04-13"))
    );
    let dates: Vec<NaiveDate> = days.iter().map(|p| p.date.date()).collect();
    assert_eq!(dates.first(), Some(&d("2026-03-02")));
    assert_eq!(dates.last(), Some(&d("2026-04-06")));
    assert!(dates.iter().all(|x| Grain::Week.is_start(*x)));
}

#[test]
fn an_hour_grain_series_labels_each_point_with_its_local_hour() {
    use oxy_experiment::Grain;
    let h = |s: &str| {
        Grain::Hour.ordinal_at(chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
    };
    let m = PanelMatrix::from_triples((0..6).flat_map(|i| {
        [
            ("a".to_string(), h("2026-03-04 22:00") + i, 1.0),
            ("c".to_string(), h("2026-03-04 22:00") + i, 2.0),
        ]
    }));
    let design = ExperimentDesign::Waves(Assignment {
        switch_day: HashMap::from([
            ("a".to_string(), Some(h("2026-03-05 00:00"))),
            ("c".to_string(), None),
        ]),
        strata: None,
        pre_days: 2,
        post_days: 3,
        anticipation_days: 0,
        washout_days: 1,
        coverage_floor: 0.8,
        alpha: 0.05,
        family: 1,
        one_date_test: oxy_airlayer_compat::engine::experiment::OneDateTest::Welch,
    });
    let s = daily_series("m", Grain::Hour, &design, &m);
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["grain"], "hour");
    assert_eq!(v["waves"][0]["post_from"], "2026-03-05T01:00");
    assert_eq!(v["days"][0]["date"], "2026-03-04T22:00");
    assert_eq!(v["days"][5]["date"], "2026-03-05T03:00");
}
