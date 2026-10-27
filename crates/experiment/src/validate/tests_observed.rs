//! A declared, non-random assignment: a past rollout read after the fact.

use super::*;
use crate::config::Assignment;
use crate::test_fixtures::{full, observed};

#[test]
fn an_observed_design_without_lever_seed_or_mde_is_statically_valid() {
    let c = observed();
    assert_eq!(c.design.assignment, Assignment::Observed);
    assert!(c.lever.is_none());
    assert_eq!(validate_static(&c), vec![]);
}

#[test]
fn an_observed_design_is_ready_for_a_retrospective() {
    assert_eq!(is_retrospective_ready(&observed()), Ok(()));
}

#[test]
fn an_observed_design_is_never_registrable() {
    assert_eq!(
        is_registrable(&observed()),
        Err(vec![ConfigError::ObservedNotRegistrable])
    );
}

#[test]
fn a_randomised_design_without_a_lever_is_not_registrable() {
    let mut c = full();
    c.lever = None;
    assert_eq!(is_registrable(&c), Err(vec![ConfigError::MissingLever]));
}

#[test]
fn a_retrospective_needs_waves_even_when_observed() {
    let mut c = observed();
    c.design.waves.clear();
    assert_eq!(
        is_retrospective_ready(&c),
        Err(vec![ConfigError::MissingWaves])
    );
}

#[test]
fn a_randomised_retrospective_needs_its_seed_but_not_an_mde() {
    let mut c = full();
    c.outcome.mde_at_proposal = None;
    c.lever = None;
    assert_eq!(is_retrospective_ready(&c), Ok(()));
    c.design.seed = None;
    assert_eq!(
        is_retrospective_ready(&c),
        Err(vec![ConfigError::MissingSeed])
    );
}

#[test]
fn an_observed_design_serializes_its_assignment_and_a_randomised_one_does_not() {
    let observed = serde_json::to_value(observed()).unwrap();
    assert_eq!(observed["design"]["assignment"], "observed");
    assert!(observed.get("lever").is_none());
    let full = serde_json::to_value(full()).unwrap();
    assert!(full["design"].get("assignment").is_none());
    assert!(full.get("lever").is_some());
}
