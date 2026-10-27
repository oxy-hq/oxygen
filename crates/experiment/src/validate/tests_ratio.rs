//! A ratio outcome: `outcome.target` over `outcome.denominator`, per unit.

use super::*;
use crate::test_fixtures::{full, observed, switchback};

#[test]
fn a_ratio_outcome_parses_and_serializes_only_when_set() {
    let mut c = observed();
    c.outcome.denominator = Some("listing_days.units".into());
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["outcome"]["denominator"], "listing_days.units");
    let plain = serde_json::to_value(full()).unwrap();
    assert!(plain["outcome"].get("denominator").is_none());
}

#[test]
fn a_ratio_outcome_is_read_from_past_data_only() {
    let mut c = full();
    c.outcome.denominator = Some("store_days.revenue".into());
    assert_eq!(
        is_registrable(&c),
        Err(vec![ConfigError::RatioNotRegistrable])
    );
    assert_eq!(is_retrospective_ready(&c), Ok(()));
}

#[test]
fn a_ratio_outcome_needs_waves_not_a_switchback() {
    let mut c = switchback();
    c.outcome.denominator = Some("store_days.revenue".into());
    assert!(validate_static(&c).contains(&ConfigError::RatioOnSwitchback));
}
