//! Refusals about a wave's `switch_off`: the day a past change ended, declared on
//! an observed design so its carryover window has an off to start from. A
//! randomised design's off is the close flip the platform sends, never a date in
//! the file.

use super::ConfigError;
use crate::config::{Assignment, ExperimentConfig};

pub(super) fn check_switch_off(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let d = &c.design;
    for (i, w) in d.waves.iter().enumerate() {
        let Some(off) = w.switch_off else { continue };
        if d.assignment != Assignment::Observed {
            out.push(ConfigError::SwitchOffNeedsObserved { wave: i + 1 });
        }
        if off <= w.switch_on {
            out.push(ConfigError::SwitchOffNotAfterSwitchOn {
                wave: i + 1,
                switch_on: w.switch_on,
                switch_off: off,
            });
            continue;
        }
        // The "during" read must end by the switch_off, or the days after the change
        // are read as during it and again as after it.
        let post_end = d
            .post_days
            .and_then(|p| d.grain.later(w.switch_on, d.washout_days.checked_add(p)?));
        if let Some(post_end) = post_end.filter(|end| off < *end) {
            out.push(ConfigError::SwitchOffInsidePostWindow {
                wave: i + 1,
                switch_off: off,
                post_end,
            });
        }
    }
    if d.carryover_days.is_none() || d.assignment != Assignment::Observed || d.waves.is_empty() {
        return;
    }
    // Nothing was switched, so the only off a carryover window can start from is the
    // declared one. Carryover already needs one switch date, so this is one wave.
    if d.waves.iter().any(|w| w.switch_off.is_none()) {
        out.push(ConfigError::CarryoverNeedsSwitchOff);
    }
}

#[cfg(test)]
mod tests {
    use crate::moment::Moment;
    use crate::test_fixtures::{full, observed};
    use crate::validate::{ConfigError, validate_static};
    use chrono::NaiveDate;

    fn day(m: u32, d: u32) -> Moment {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap().into()
    }

    #[test]
    fn an_observed_design_with_a_switch_off_and_carryover_is_valid_and_round_trips() {
        let mut c = observed();
        c.design.waves[0].switch_off = Some(day(3, 15));
        c.design.carryover_days = Some(14);
        assert_eq!(validate_static(&c), vec![]);
        let yaml = serde_yaml::to_string(&c).unwrap();
        assert!(yaml.contains("switch_off: 2026-03-15"), "{yaml}");
        let back: crate::config::ExperimentConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.design.waves[0].switch_off, Some(day(3, 15)));
    }

    #[test]
    fn an_absent_switch_off_is_not_serialized() {
        let yaml = serde_yaml::to_string(&observed()).unwrap();
        assert!(!yaml.contains("switch_off"), "{yaml}");
    }

    #[test]
    fn a_randomised_design_cannot_declare_a_switch_off() {
        let mut c = full();
        c.design.waves[0].switch_off = Some(day(12, 31));
        assert!(
            validate_static(&c).contains(&ConfigError::SwitchOffNeedsObserved { wave: 1 }),
            "{:?}",
            validate_static(&c)
        );
    }

    #[test]
    fn a_switch_off_must_come_after_the_switch_on() {
        let mut c = observed();
        c.design.waves[0].switch_off = Some(day(3, 4));
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::SwitchOffNotAfterSwitchOn {
                wave: 1,
                switch_on: day(3, 4),
                switch_off: day(3, 4),
            }]
        );
    }

    #[test]
    fn a_switch_off_inside_the_post_window_is_refused() {
        // Switch 03-04, post 7: the post window ends 03-11. An end before that would put
        // days after the change into the "during" read and the "after" read both.
        let mut c = observed();
        c.design.waves[0].switch_off = Some(day(3, 8));
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::SwitchOffInsidePostWindow {
                wave: 1,
                switch_off: day(3, 8),
                post_end: day(3, 11),
            }]
        );
        c.design.waves[0].switch_off = Some(day(3, 11));
        assert_eq!(
            validate_static(&c),
            vec![],
            "the day the post window ends is fine"
        );
    }

    #[test]
    fn an_observed_carryover_needs_a_declared_off() {
        let mut c = observed();
        c.design.carryover_days = Some(14);
        assert_eq!(
            validate_static(&c),
            vec![ConfigError::CarryoverNeedsSwitchOff]
        );
    }
}
