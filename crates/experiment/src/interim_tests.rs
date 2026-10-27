use crate::interim::{as_interim, interim_config};
use crate::report::ReportStatus;
use crate::test_fixtures::{built_report, date, effect, full, names, switchback_config};

// `full()`: waves 2026-03-02 [bondi, newtown] and 2026-03-16 [manly]; washout 7, post 56.
// A post window is [switch + 7, switch + 7 + post), so wave 1 reads from 03-09, wave 2
// from 03-23, and the registered end is 05-18 (exclusive).

#[test]
fn a_wave_without_a_whole_post_day_is_left_out_and_its_units_leave_the_strata() {
    let w = interim_config(&full(), date(2026, 3, 20).into()).unwrap();
    assert_eq!(w.config.design.waves.len(), 1);
    assert_eq!(w.config.design.waves[0].units, names(&["bondi", "newtown"]));
    assert_eq!(w.left_out, 1);
    // 03-09 .. 03-19: eleven whole post days.
    assert_eq!(w.config.design.post_days, Some(11));
    assert_eq!(w.through, date(2026, 3, 19));
    let strata = w.config.design.strata.unwrap();
    assert!(
        strata.iter().all(|s| !s.contains(&"manly".to_string())),
        "{strata:?}"
    );
    assert_eq!(strata[1].len(), 4, "the other stratum is untouched");
}

#[test]
fn every_post_window_shortens_to_the_shortest_one_read_so_far() {
    let w = interim_config(&full(), date(2026, 3, 30).into()).unwrap();
    assert_eq!(w.config.design.waves.len(), 2);
    assert_eq!(w.left_out, 0);
    // Wave 2 has read 03-23 .. 03-29: seven days. Wave 1 has read more, but one window
    // length serves every wave.
    assert_eq!(w.config.design.post_days, Some(7));
    assert_eq!(w.through, date(2026, 3, 29));
}

#[test]
fn a_cut_past_the_registered_end_reads_the_registered_design() {
    let w = interim_config(&full(), date(2026, 6, 1).into()).unwrap();
    assert_eq!(w.config, full());
    assert_eq!(w.through, date(2026, 5, 17));
}

#[test]
fn before_any_whole_post_day_there_is_nothing_to_read_and_it_says_when_there_will_be() {
    let why = interim_config(&full(), date(2026, 3, 9).into()).unwrap_err();
    assert!(why.contains("2026-03-09"), "{why}");
    assert!(why.contains("2026-03-10"), "{why}");
}

#[test]
fn a_switchback_keeps_only_its_finished_pairs() {
    // Periods of 7 days from 03-02 (on), 03-09 (off), 03-16 (off), 03-23 (on).
    let w = interim_config(&switchback_config(), date(2026, 3, 20).into()).unwrap();
    let sb = w.config.design.switchback.unwrap();
    assert_eq!(sb.periods.len(), 2);
    assert_eq!(w.left_out, 1);
    assert_eq!(w.through, date(2026, 3, 15));
}

#[test]
fn a_switchback_with_no_finished_pair_says_when_the_first_one_finishes() {
    let why = interim_config(&switchback_config(), date(2026, 3, 12).into()).unwrap_err();
    assert!(why.contains("2026-03-15"), "{why}");
    assert!(why.contains("2026-03-16"), "{why}");
}

#[test]
fn an_interim_report_is_labelled_and_drops_what_would_extrapolate_or_write_back() {
    let report = built_report(&full(), &effect(20.0, 5.0), None);
    assert!(report.treated_total.is_some() && report.projection.is_some());
    let v = as_interim(report, date(2026, 3, 19));
    assert_eq!(
        v.status,
        ReportStatus::Interim {
            through: date(2026, 3, 19)
        }
    );
    assert!(v.label.starts_with("INTERIM"), "{}", v.label);
    assert!(v.label.contains("2026-03-19"), "{}", v.label);
    assert!(v.treated_total.is_none());
    assert!(v.projection.is_none());
    assert!(v.write_back.is_none());
    assert!(v.prediction.is_none());
    assert!(v.headline.is_some(), "the estimate itself is the point");
    let json = serde_json::to_value(&v.status).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "kind": "interim", "through": "2026-03-19" })
    );
}

#[test]
fn at_week_grain_only_whole_weeks_are_read_and_the_week_in_progress_is_not() {
    let mut c = full();
    c.design.grain = crate::Grain::Week;
    c.design.washout_days = 1;
    c.design.post_days = Some(8);
    // Wave 1 reads from Monday 03-09, wave 2 from 03-23. Thursday 04-02 is inside
    // the week of 03-30, which has not finished: wave 2 has read one week.
    let w = interim_config(&c, date(2026, 4, 2).into()).unwrap();
    assert_eq!((w.config.design.waves.len(), w.left_out), (2, 0));
    assert_eq!(w.config.design.post_days, Some(1));
    assert_eq!(w.through, date(2026, 3, 29));
}
