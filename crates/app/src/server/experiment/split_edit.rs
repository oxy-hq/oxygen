//! Whether a hand edit changed the split pricing drew.
//!
//! Adopting a proposal writes its draw onto the draft and records the proposal
//! (`experiment_designs.from_proposal_id`). Editing the units, dates, strata,
//! seed or grouping afterwards leaves a split no draw produced, so the link to
//! the proposal is cleared, and the report says the split was written by hand.

use oxy_experiment::ExperimentConfig;

/// The keys that make up the drawn split.
pub fn split_changed(before: &ExperimentConfig, after: &ExperimentConfig) -> bool {
    let (b, a) = (&before.design, &after.design);
    b.waves != a.waves
        || b.switchback.as_ref().map(|s| &s.periods) != a.switchback.as_ref().map(|s| &s.periods)
        || b.strata != a.strata
        || b.seed != a.seed
        || before.assign_by != after.assign_by
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::experiment::test_design::design;

    #[test]
    fn editing_a_waves_units_changes_the_split() {
        let before = design();
        let mut after = before.clone();
        after.design.waves[0].units.push("added".into());
        assert!(split_changed(&before, &after));
    }

    #[test]
    fn editing_the_hypothesis_or_the_outcome_does_not() {
        let before = design();
        let mut after = before.clone();
        after.hypothesis = "something else".into();
        after.outcome.alpha = 0.1;
        assert!(!split_changed(&before, &after));
    }

    #[test]
    fn a_new_seed_changes_the_split() {
        let before = design();
        let mut after = before.clone();
        after.design.seed = after.design.seed.map(|s| s + 1).or(Some(1));
        assert!(split_changed(&before, &after));
    }
}
