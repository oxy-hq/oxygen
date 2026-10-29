//! Refusals about `design.one_date_test`: it picks the test for a waves design
//! with one switch date, so it means nothing anywhere else.

use super::ConfigError;
use crate::config::{Comparison, ExperimentConfig, OneDateTest};
use std::collections::BTreeSet;

pub(super) fn check_one_date_test(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let d = &c.design;
    if d.one_date_test != OneDateTest::Permutation {
        return;
    }
    for (set, with) in [
        (d.switchback.is_some(), "design.switchback"),
        (
            d.comparison == Comparison::SyntheticControl,
            "design.comparison: synthetic_control",
        ),
    ] {
        if set {
            out.push(ConfigError::OneDateTestWith { with });
        }
    }
    let dates: BTreeSet<_> = d.waves.iter().map(|w| w.switch_on).collect();
    if dates.len() > 1 {
        out.push(ConfigError::OneDateTestNeedsOneSwitchDate {
            switch_dates: dates.len(),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Comparison, OneDateTest};
    use crate::test_fixtures::{full, observed, switchback};
    use crate::validate::{ConfigError, validate_static};

    #[test]
    fn a_one_date_permutation_design_is_valid_and_round_trips() {
        let mut c = observed();
        c.design.one_date_test = OneDateTest::Permutation;
        assert_eq!(validate_static(&c), vec![]);
        let yaml = serde_yaml::to_string(&c).unwrap();
        assert!(yaml.contains("one_date_test: permutation"), "{yaml}");
        let back: crate::config::ExperimentConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.design.one_date_test, OneDateTest::Permutation);
    }

    #[test]
    fn the_default_test_is_not_serialized() {
        let yaml = serde_yaml::to_string(&observed()).unwrap();
        assert!(!yaml.contains("one_date_test"), "{yaml}");
    }

    #[test]
    fn permutation_is_refused_where_it_chooses_nothing() {
        let mut c = full();
        c.design.one_date_test = OneDateTest::Permutation;
        assert!(
            validate_static(&c)
                .contains(&ConfigError::OneDateTestNeedsOneSwitchDate { switch_dates: 2 }),
            "{:?}",
            validate_static(&c)
        );
        let mut c = switchback();
        c.design.one_date_test = OneDateTest::Permutation;
        assert!(
            validate_static(&c).contains(&ConfigError::OneDateTestWith {
                with: "design.switchback"
            }),
            "{:?}",
            validate_static(&c)
        );
        let mut c = observed();
        c.design.one_date_test = OneDateTest::Permutation;
        c.design.comparison = Comparison::SyntheticControl;
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::OneDateTestWith {
                with: "design.comparison: synthetic_control"
            }]
        );
    }
}
