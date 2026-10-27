use super::*;
use crate::test_fixtures::{draft, full, switchback, switchback_draft};

#[test]
fn a_priced_file_is_registrable() {
    assert_eq!(is_registrable(&full()), Ok(()));
}

#[test]
fn a_priced_switchback_is_registrable_with_periods_instead_of_waves() {
    assert_eq!(is_registrable(&switchback()), Ok(()));
}

#[test]
fn a_switchback_draft_says_periods_not_waves_are_missing() {
    assert_eq!(
        is_registrable(&switchback_draft()),
        Err(vec![
            ConfigError::MissingPeriods,
            ConfigError::MissingSeed,
            ConfigError::MissingMdeAtProposal
        ])
    );
}

#[test]
fn a_switchback_missing_only_its_seed_is_refused_for_the_seed() {
    let mut c = switchback();
    c.design.seed = None;
    assert_eq!(is_registrable(&c), Err(vec![ConfigError::MissingSeed]));
}

#[test]
fn a_draft_is_not_registrable_and_says_what_is_missing() {
    assert_eq!(
        is_registrable(&draft()),
        Err(vec![
            ConfigError::MissingWaves,
            ConfigError::MissingSeed,
            ConfigError::MissingMdeAtProposal
        ])
    );
}

#[test]
fn each_missing_key_is_refused_on_its_own() {
    let mut c = full();
    c.design.waves.clear();
    assert_eq!(is_registrable(&c), Err(vec![ConfigError::MissingWaves]));

    let mut c = full();
    c.design.seed = None;
    assert_eq!(is_registrable(&c), Err(vec![ConfigError::MissingSeed]));

    let mut c = full();
    c.outcome.mde_at_proposal = None;
    assert_eq!(
        is_registrable(&c),
        Err(vec![ConfigError::MissingMdeAtProposal])
    );
}

#[test]
fn registrable_messages_state_only_their_own_guard() {
    assert_eq!(
        ConfigError::MissingSeed.to_string(),
        "design.seed is absent"
    );
    assert_eq!(
        ConfigError::MissingWaves.to_string(),
        "design.waves lists no wave"
    );
    assert_eq!(
        ConfigError::MissingPeriods.to_string(),
        "design.switchback.periods lists no period"
    );
    assert_eq!(
        ConfigError::MissingMdeAtProposal.to_string(),
        "outcome.mde_at_proposal is absent"
    );
}
