//! Refusals that need the entity's units — or, under `assign_by`, its group
//! names. The caller passes whichever the design is randomised over; nothing
//! here depends on which.

use super::ConfigError;
use crate::config::ExperimentConfig;
use crate::suggest::closest;
use std::collections::HashSet;

/// Refusals that need the entity's units (`units`, as the warehouse rendered
/// them). A wave names a unit **by exact, case-sensitive name** or is refused
/// with the nearest names; and if every unit is placed in a wave there is no
/// control pool. A draft (no waves) and a switchback (which names no unit) have
/// nothing to refuse here. `units` is the pool about to be pinned: pricing's
/// panel units, or the entity's current units at registration. `design.strata`,
/// when present, must hold exactly these names (see `check_strata`), so call
/// this with the pool about to be pinned, never with units that appeared after.
pub fn validate_against_units(c: &ExperimentConfig, units: &[String]) -> Vec<ConfigError> {
    let known: HashSet<&str> = units.iter().map(String::as_str).collect();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for (i, wave) in c.design.waves.iter().enumerate() {
        for unit in &wave.units {
            if known.contains(unit.as_str()) {
                placed.insert(unit.as_str());
            } else {
                out.push(ConfigError::UnknownUnit {
                    unit: unit.clone(),
                    wave: i + 1,
                    switch_on: wave.switch_on,
                    suggestions: closest(unit, units),
                });
            }
        }
    }
    // Every unit placed over two or more switch dates is a fully staggered rollout:
    // the later waves control the earlier ones and the estimator drops the last.
    // On one switch date nothing is ever a control.
    let switch_dates: HashSet<_> = c.design.waves.iter().map(|w| w.switch_on).collect();
    if !known.is_empty() && placed.len() == known.len() && switch_dates.len() < 2 {
        out.push(ConfigError::NoControlUnits { units: known.len() });
    }
    check_strata(c, units, &known, &mut out);
    out
}

/// `design.strata`, when present, must hold exactly the names in `units`: a
/// stratum name that matches none is `UnknownStratumUnit` (with suggestions);
/// a name in `units` that no stratum holds is `UnitInNoStratum`, once, in
/// `units` order. A name in two strata is the static `UnitInTwoStrata` refusal.
fn check_strata(
    c: &ExperimentConfig,
    units: &[String],
    known: &HashSet<&str>,
    out: &mut Vec<ConfigError>,
) {
    let Some(strata) = &c.design.strata else {
        return;
    };
    let mut stratified: HashSet<&str> = HashSet::new();
    for (i, stratum) in strata.iter().enumerate() {
        for unit in stratum {
            if known.contains(unit.as_str()) {
                stratified.insert(unit.as_str());
            } else {
                out.push(ConfigError::UnknownStratumUnit {
                    unit: unit.clone(),
                    stratum: i + 1,
                    suggestions: closest(unit, units),
                });
            }
        }
    }
    let mut reported: HashSet<&str> = HashSet::new();
    for unit in units {
        if !stratified.contains(unit.as_str()) && reported.insert(unit.as_str()) {
            out.push(ConfigError::UnitInNoStratum { unit: unit.clone() });
        }
    }
}
