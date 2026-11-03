//! `guardrails`' tests.
use chrono::Utc;
use std::collections::BTreeMap;
use uuid::Uuid;

use super::*;
use crate::server::experiment::test_design::{design, switchback};

fn d(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

fn flip(unit: &str, state: &str, due: &str, status: &str) -> experiment_flips::Model {
    experiment_flips::Model {
        id: Uuid::new_v4(),
        run_id: Uuid::nil(),
        unit: unit.into(),
        target_state: state.into(),
        due_at: d(due).and_hms_opt(0, 0, 0).unwrap(),
        wave: 0,
        task_id: Some(Uuid::nil()),
        agentic_run_id: Some(Uuid::nil()),
        lever_revision_id: None,
        status: status.into(),
        confirmed_on: None,
        reason: None,
        created_at: Utc::now().fixed_offset(),
        attempts: 1,
        failed_at: None,
    }
}

fn watched_from(day: &str) -> Watched {
    Watched {
        unit: "bondi".into(),
        members: vec!["bondi".into()],
        treated: vec![(d(day), NaiveDate::MAX)],
    }
}

fn flags(days: &[&str]) -> Vec<(NaiveDate, String)> {
    days.iter()
        .map(|s| (d(s), format!("observed on {s}")))
        .collect()
}

#[test]
fn one_flagged_treated_day_does_not_abort() {
    let w = watched_from("2026-03-02");
    let u = anomaly_unit(&w, "m", &flags(&["2026-03-03", "2026-03-05"]));
    assert_eq!(u.status, GuardrailState::Ok);
    assert!(
        u.detail.unwrap().contains("2026-03-05"),
        "names the latest flag"
    );
}

#[test]
fn two_consecutive_flagged_treated_days_abort() {
    let w = watched_from("2026-03-02");
    let u = anomaly_unit(&w, "m", &flags(&["2026-03-03", "2026-03-05", "2026-03-06"]));
    assert_eq!(u.status, GuardrailState::Breached);
    let detail = u.detail.unwrap();
    assert!(
        detail.contains("on 2026-03-05 through 2026-03-06"),
        "{detail}"
    );
}

#[test]
fn a_flag_before_the_switch_does_not_count_toward_the_run() {
    let w = watched_from("2026-03-02");
    let u = anomaly_unit(&w, "m", &flags(&["2026-03-01", "2026-03-02"]));
    assert_eq!(u.status, GuardrailState::Ok);
}

#[test]
fn significance_selects_the_most_sensitive_cutoff_it_does_not_undercut() {
    assert_eq!(sensitivity_for(0.95).unwrap(), "high");
    assert_eq!(sensitivity_for(0.99).unwrap(), "high");
    assert_eq!(sensitivity_for(0.9973).unwrap(), "medium");
    assert_eq!(sensitivity_for(0.999).unwrap(), "medium");
    assert_eq!(sensitivity_for(0.999_999_9).unwrap(), "low");
}

#[test]
fn significance_outside_the_open_unit_interval_is_refused() {
    for bad in [0.0, 1.0, -0.5, f64::NAN] {
        let err = sensitivity_for(bad).unwrap_err();
        assert!(err.contains("not in (0, 1)"), "{bad}: {err}");
    }
}

#[test]
fn a_waves_unit_is_watched_from_its_switch_day_while_its_on_stands() {
    let flips = [
        flip("bondi", "on", "2026-03-02", "confirmed"),
        flip("newtown", "on", "2026-03-02", "failed"),
        flip("manly", "on", "2026-03-16", "dispatched"),
        flip("manly", "off", "2026-03-20", "pending"),
    ];
    let w = watched(&design(), &flips, None, None, d("2026-03-21"));
    assert_eq!(
        w.iter().map(|w| w.unit.as_str()).collect::<Vec<_>>(),
        vec!["bondi"]
    );
    assert_eq!(w[0].members, vec!["bondi"]);
    assert!(w[0].treats(d("2026-03-02")) && !w[0].treats(d("2026-03-01")));
}

#[test]
fn a_group_is_watched_over_its_pinned_members() {
    let flips = [flip("east", "on", "2026-03-02", "confirmed")];
    let pinned = GroupMembership(BTreeMap::from([(
        "east".to_string(),
        vec!["bondi".to_string(), "coogee".to_string()],
    )]));
    let w = watched(&design(), &flips, Some(&pinned), None, d("2026-03-05"));
    assert_eq!(
        (w[0].unit.as_str(), w[0].members.clone()),
        ("east", vec!["bondi".to_string(), "coogee".to_string()])
    );
}

#[test]
fn a_switchback_unit_is_treated_on_its_begun_on_periods_only() {
    let fleet = ["bondi".to_string(), "glebe".to_string()];
    assert!(
        watched(&switchback(), &[], None, Some(&fleet), d("2026-03-01")).is_empty(),
        "nothing has begun"
    );
    let flips: Vec<_> = ["bondi", "glebe"]
        .into_iter()
        .flat_map(|u| {
            [
                flip(u, "on", "2026-03-02", "confirmed"),
                flip(u, "on", "2026-03-23", "dispatched"),
            ]
        })
        .collect();
    let w = watched(&switchback(), &flips, None, Some(&fleet), d("2026-03-24"));
    assert_eq!(w.len(), 2, "every fleet unit");
    assert!(w[0].treats(d("2026-03-08")), "period 0 is on");
    assert!(
        !w[0].treats(d("2026-03-09")) && !w[0].treats(d("2026-03-22")),
        "periods 1 and 2 are off"
    );
    assert!(w[0].treats(d("2026-03-23")), "period 3 is on and has begun");
}

#[test]
fn a_switchback_on_period_whose_on_never_went_out_is_not_watched() {
    // glebe's first `on` was cancelled (its period ended before dispatch) and bondi's
    // second failed: neither is a treated day, so a trigger there cannot abort the run.
    let fleet = ["bondi".to_string(), "glebe".to_string()];
    let mut cancelled = flip("glebe", "on", "2026-03-02", "cancelled");
    cancelled.task_id = None;
    cancelled.reason = Some(crate::server::experiment::send::PERIOD_ENDED.into());
    let flips = [
        flip("bondi", "on", "2026-03-02", "confirmed"),
        cancelled,
        flip("bondi", "on", "2026-03-23", "failed"),
        flip("glebe", "on", "2026-03-23", "dispatched"),
    ];
    let w = watched(&switchback(), &flips, None, Some(&fleet), d("2026-03-24"));
    let bondi = w.iter().find(|w| w.unit == "bondi").expect("bondi");
    assert!(bondi.treats(d("2026-03-02")));
    assert!(!bondi.treats(d("2026-03-23")), "its on failed");
    let glebe = w.iter().find(|w| w.unit == "glebe").expect("glebe");
    assert!(!glebe.treats(d("2026-03-02")), "its on was never sent");
    assert!(glebe.treats(d("2026-03-23")));
}
