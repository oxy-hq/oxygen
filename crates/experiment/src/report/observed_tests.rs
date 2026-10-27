//! A declared, non-random assignment, and a design no pricing ever sized.

use super::tests::Case;
use crate::config::Assignment;
use crate::test_fixtures::effect;

#[test]
fn an_observed_design_says_it_was_not_randomised_instead_of_counting_draws() {
    let mut c = Case::new();
    c.cfg.design.assignment = Assignment::Observed;
    c.proposals = 1;
    let p = c.build().proposals;
    assert!(p.observational);
    assert_eq!(
        p.message,
        "Observational: the assignment was declared, not randomised, so treated and control units \
         may differ in ways other than the change"
    );
}

#[test]
fn a_randomised_design_is_not_observational() {
    assert!(!Case::new().build().proposals.observational);
}

#[test]
fn without_a_priced_mde_the_report_says_why_instead_of_comparing() {
    let mut c = Case::new();
    c.cfg.outcome.mde_at_proposal = None;
    c.mde_unavailable = Some("fewer than 2 non-overlapping 29-day spans fit".into());
    c.target = effect(9.0, 2.0);
    let v = c.build();
    assert!(v.promise.is_none());
    assert_eq!(
        v.mde_note.as_deref(),
        Some(
            "MDE unavailable: no smallest detectable effect was priced for this design \
             (pricing: fewer than 2 non-overlapping 29-day spans fit)"
        )
    );
}

#[test]
fn without_a_priced_mde_or_a_pricing_reason_the_note_is_plain() {
    let mut c = Case::new();
    c.cfg.outcome.mde_at_proposal = None;
    assert_eq!(
        c.build().mde_note.as_deref(),
        Some("MDE unavailable: no smallest detectable effect was priced for this design")
    );
}

#[test]
fn a_priced_design_carries_no_mde_note() {
    assert!(Case::new().build().mde_note.is_none());
}

#[test]
fn a_split_edited_by_hand_after_pricing_is_not_reported_as_a_draw() {
    let mut c = Case::new();
    c.proposals = 1;
    c.hand_assigned = true;
    let p = c.build().proposals;
    assert!(!p.observational);
    assert!(p.hand_assigned);
    assert_eq!(
        p.message,
        "the analysed split was written by hand, not drawn by the platform: none of the 1 \
         logged proposal is this split"
    );
}

#[test]
fn units_the_estimate_left_out_are_listed_with_their_reason() {
    use crate::engine_in::DroppedUnitIn;
    let mut c = Case::new();
    c.target.dropped_units = vec![DroppedUnitIn {
        unit: "a7".into(),
        reason: "a treated unit with no rows in the panel".into(),
    }];
    assert_eq!(
        c.build().checks.dropped_units,
        vec!["a7: a treated unit with no rows in the panel".to_string()]
    );
}

#[test]
fn a_ratio_outcome_is_labelled_as_one_and_carries_no_totals() {
    let mut c = Case::new();
    c.cfg.outcome.denominator = Some("store_days.covers".into());
    let v = c.build();
    assert_eq!(v.target, "store_days.beverages_revenue ÷ store_days.covers");
    assert!(v.estimand.contains("ratio"), "{}", v.estimand);
    assert!(
        v.treated_total.is_none(),
        "a share summed over unit-days means nothing"
    );
    assert!(v.projection.is_none());
}

#[test]
fn a_ratio_headline_names_the_ratio_per_unit() {
    let mut c = Case::new();
    c.cfg.outcome.denominator = Some("store_days.covers".into());
    let text = c.build().headline.unwrap().text;
    assert!(
        text.contains("per unit on store_days.beverages_revenue ÷ store_days.covers"),
        "{text}"
    );
}

#[test]
fn a_ratio_report_says_it_is_a_share_so_the_surface_does_not_print_money() {
    let mut c = Case::new();
    assert!(!c.build().share);
    c.cfg.outcome.denominator = Some("store_days.covers".into());
    assert!(c.build().share);
}
