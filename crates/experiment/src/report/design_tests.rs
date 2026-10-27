use super::tests::Case;
use super::*;
use crate::test_fixtures::{effect, grouped_config, refused, switchback_config};

fn switchback_case() -> Case {
    let mut c = Case::new();
    c.cfg = switchback_config();
    c.switchback = Some(SwitchbackFacts {
        fleet_units: 12,
        on_days: 24,
    });
    c.target = effect(12.5, 2.0);
    c
}

#[test]
fn a_unit_design_keeps_its_estimand_and_wording() {
    let v = Case::new().build();
    assert_eq!(v.design, DesignKind::Units);
    assert_eq!(v.estimand, "intention-to-treat effect per unit-day");
    assert!(v.headline.unwrap().text.contains("per unit-day"));
}

#[test]
fn a_group_design_states_dollars_per_group_day_and_totals_mean_times_groups_times_days() {
    let mut c = Case::new();
    c.cfg = grouped_config();
    c.target = effect(12.5, 2.0);
    let v = c.build();
    assert_eq!(v.design, DesignKind::Groups);
    assert!(v.estimand.contains("per group-day"), "{}", v.estimand);
    let h = v.headline.clone().expect("a headline");
    assert!(h.text.contains("per group-day"), "{}", h.text);
    let t = v.treated_total.expect("always shown");
    assert_eq!(
        (t.units, t.days),
        (6, 56),
        "six treated groups, 56 post-window days"
    );
    assert_eq!(t.total, 12.5 * 6.0 * 56.0);
    assert!(
        t.basis.contains("treated groups") && t.basis.contains("sum of its members"),
        "{}",
        t.basis
    );
}

#[test]
fn a_group_designs_floor_and_projection_speak_of_groups() {
    let mut c = Case::new();
    c.cfg = grouped_config();
    let v = c.build();
    assert!(
        v.substitution.statement.contains("per group-day"),
        "{}",
        v.substitution.statement
    );
    let p = v.projection.expect("there are untreated groups");
    assert!(
        p.assumption.contains("untreated groups"),
        "{}",
        p.assumption
    );
}

#[test]
fn a_switchbacks_assumptions_are_always_present() {
    let v = switchback_case().build();
    assert_eq!(v.design, DesignKind::Switchback);
    assert_eq!(v.assumptions.len(), 2);
    assert!(
        v.assumptions[0].contains("Carryover") && v.assumptions[0].contains("toward zero"),
        "{:?}",
        v.assumptions
    );
    assert!(
        v.assumptions[1].contains("Pull-forward") && v.assumptions[1].contains("away from zero"),
        "{:?}",
        v.assumptions
    );
}

#[test]
fn a_switchbacks_assumptions_survive_a_refused_estimate() {
    let mut c = switchback_case();
    c.target = refused("fewer than 6 pairs: the smallest attainable p exceeds alpha");
    let v = c.build();
    assert!(v.refusal.is_some() && v.headline.is_none());
    assert_eq!(
        v.assumptions.len(),
        2,
        "the design's caveats are stated whatever the estimator said"
    );
}

#[test]
fn designs_other_than_a_switchback_carry_no_assumptions() {
    assert!(Case::new().build().assumptions.is_empty());
    let mut g = Case::new();
    g.cfg = grouped_config();
    assert!(g.build().assumptions.is_empty());
}

#[test]
fn a_switchback_is_never_projected_onto_untreated_units() {
    let mut c = switchback_case();
    c.n_untreated = 6; // even if a caller says there are some: the whole fleet is on during on-periods
    assert!(c.build().projection.is_none());
}

#[test]
fn a_switchbacks_treated_total_is_over_the_on_days() {
    let t = switchback_case()
        .build()
        .treated_total
        .expect("a total over the on days");
    assert_eq!((t.units, t.days), (12, 24));
    assert_eq!(t.total, 12.5 * 12.0 * 24.0);
    assert_eq!(
        t.interval,
        Interval {
            low: Some(10.5 * 288.0),
            high: Some(14.5 * 288.0)
        }
    );
    assert!(
        t.basis.contains("on-period days") && t.basis.contains("not a projection"),
        "{}",
        t.basis
    );
}

#[test]
fn a_switchback_without_its_facts_has_no_total_but_still_a_headline() {
    let mut c = switchback_case();
    c.switchback = None;
    let v = c.build();
    assert!(
        v.treated_total.is_none(),
        "no fleet size or on-days: no number invented"
    );
    assert!(v.headline.is_some());
}

#[test]
fn a_switchbacks_estimand_is_the_short_run_effect_of_alternation() {
    let v = switchback_case().build();
    assert!(
        v.estimand
            .contains("short-run effect of on against off under alternation"),
        "{}",
        v.estimand
    );
    assert!(
        v.estimand
            .contains("not the effect of leaving the lever on"),
        "{}",
        v.estimand
    );
    assert!(v.headline.unwrap().text.contains("per unit-day"));
}
