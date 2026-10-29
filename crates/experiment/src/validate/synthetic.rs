//! Refusals about `design.comparison: synthetic_control` and its `exclude` list.
//!
//! v1 reads a synthetic control from past data only: one switch date, a waves
//! design, levels or a log scale of one measure. What the units are is the
//! host's (`units::validate_against_units`); this checks the file alone.

use super::ConfigError;
use crate::config::{Assignment, Comparison, ExperimentConfig};
use std::collections::BTreeSet;

pub(super) fn check_synthetic(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let d = &c.design;
    if d.comparison != Comparison::SyntheticControl {
        if !d.exclude.is_empty() {
            out.push(ConfigError::ExcludeWithoutSynthetic);
        }
        return;
    }
    if d.assignment != Assignment::Observed {
        out.push(ConfigError::SyntheticNeedsObserved);
    }
    // A synthetic control is fitted per treated unit over one pre window; the
    // pool of donors replaces both blocking and a fleet-wide alternation.
    for (set, with) in [
        (d.switchback.is_some(), "design.switchback"),
        (d.strata.is_some(), "design.strata"),
        (c.outcome.denominator.is_some(), "outcome.denominator"),
        (d.carryover_days.is_some(), "design.carryover_days"),
        // Each fit is charted by date; hourly points would collapse onto their day.
        (d.grain == crate::grain::Grain::Hour, "design.grain: hour"),
    ] {
        if set {
            out.push(ConfigError::SyntheticWith { with });
        }
    }
    let dates: BTreeSet<_> = d.waves.iter().map(|w| w.switch_on).collect();
    if dates.len() > 1 {
        out.push(ConfigError::SyntheticNeedsOneSwitchDate {
            switch_dates: dates.len(),
        });
    }
    let treated: BTreeSet<&str> = d
        .waves
        .iter()
        .flat_map(|w| w.units.iter().map(String::as_str))
        .collect();
    let mut reported = BTreeSet::new();
    for unit in &d.exclude {
        if treated.contains(unit.as_str()) && reported.insert(unit.as_str()) {
            out.push(ConfigError::ExcludedUnitTreated { unit: unit.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{Assignment, Comparison};
    use crate::test_fixtures::{full, observed};
    use crate::validate::{ConfigError, validate_static};

    fn synthetic() -> crate::config::ExperimentConfig {
        let mut c = observed();
        c.design.comparison = Comparison::SyntheticControl;
        c
    }

    #[test]
    fn an_observed_one_date_synthetic_design_is_valid_and_round_trips() {
        let mut c = synthetic();
        c.design.exclude = vec!["b9".into()];
        assert_eq!(validate_static(&c), vec![]);
        let yaml = serde_yaml::to_string(&c).unwrap();
        assert!(yaml.contains("comparison: synthetic_control"), "{yaml}");
        let back: crate::config::ExperimentConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.design.exclude, vec!["b9".to_string()]);
    }

    #[test]
    fn the_default_comparison_is_not_serialized() {
        let yaml = serde_yaml::to_string(&observed()).unwrap();
        assert!(!yaml.contains("comparison"), "{yaml}");
        assert!(!yaml.contains("exclude"), "{yaml}");
    }

    #[test]
    fn a_randomised_synthetic_design_is_refused() {
        let mut c = full();
        c.design.comparison = Comparison::SyntheticControl;
        assert_eq!(c.design.assignment, Assignment::Randomised);
        let errs = validate_static(&c);
        assert!(
            errs.contains(&ConfigError::SyntheticNeedsObserved),
            "{errs:?}"
        );
        assert!(
            errs.contains(&ConfigError::SyntheticWith {
                with: "design.strata"
            }),
            "{errs:?}"
        );
        assert!(
            errs.contains(&ConfigError::SyntheticNeedsOneSwitchDate { switch_dates: 2 }),
            "{errs:?}"
        );
    }

    #[test]
    fn exclude_needs_a_synthetic_control_and_may_not_name_a_treated_unit() {
        let mut c = observed();
        c.design.exclude = vec!["b9".into()];
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::ExcludeWithoutSynthetic]
        );
        let mut c = synthetic();
        c.design.exclude = vec!["a2".into(), "a2".into()];
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::ExcludedUnitTreated { unit: "a2".into() }]
        );
    }
}
