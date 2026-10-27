//! Whether a design can be registered, and whether it can be read after the fact.

use super::ConfigError;
use crate::config::{Assignment, ExperimentConfig};

/// A design is registrable only once pricing has filled in its output: an
/// assignment — `design.waves`, **or** `design.switchback.periods` — plus
/// `design.seed` and `outcome.mde_at_proposal`, and only when it has a lever to
/// switch. Every missing key is reported; a file with a `switchback` key and no
/// periods is told `MissingPeriods`, one with neither shape `MissingWaves`. An
/// observed design is refused on that alone: what it declares already happened.
pub fn is_registrable(c: &ExperimentConfig) -> Result<(), Vec<ConfigError>> {
    if c.design.assignment == Assignment::Observed {
        return Err(vec![ConfigError::ObservedNotRegistrable]);
    }
    if c.outcome.denominator.is_some() {
        return Err(vec![ConfigError::RatioNotRegistrable]);
    }
    let mut missing = Vec::new();
    if c.lever.is_none() {
        missing.push(ConfigError::MissingLever);
    }
    push_missing_assignment(c, &mut missing);
    if c.design.seed.is_none() {
        missing.push(ConfigError::MissingSeed);
    }
    if c.outcome.mde_at_proposal.is_none() {
        missing.push(ConfigError::MissingMdeAtProposal);
    }
    done(missing)
}

/// What a retrospective — the analysis of a design whose dates are past — needs
/// from the design alone: an assignment to read, and for a randomised one the
/// `seed` that drew it (the estimate re-draws placebo splits from it). It needs
/// no lever, since nothing is switched, and no `mde_at_proposal`: the report
/// says when none was priced instead of refusing to read.
pub fn is_retrospective_ready(c: &ExperimentConfig) -> Result<(), Vec<ConfigError>> {
    let mut missing = Vec::new();
    push_missing_assignment(c, &mut missing);
    if c.design.assignment == Assignment::Randomised && c.design.seed.is_none() {
        missing.push(ConfigError::MissingSeed);
    }
    done(missing)
}

fn push_missing_assignment(c: &ExperimentConfig, missing: &mut Vec<ConfigError>) {
    let has_waves = !c.design.waves.is_empty();
    let has_periods = c
        .design
        .switchback
        .as_ref()
        .is_some_and(|s| !s.periods.is_empty());
    if !has_waves && !has_periods {
        missing.push(if c.design.switchback.is_some() {
            ConfigError::MissingPeriods
        } else {
            ConfigError::MissingWaves
        });
    }
}

fn done(missing: Vec<ConfigError>) -> Result<(), Vec<ConfigError>> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}
