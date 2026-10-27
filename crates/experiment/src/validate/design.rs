//! Refusals about the shape of the `design` block: waves versus switchback, the
//! windows a waves design needs, and whether strata overlap.
//!
//! Everything here is decidable from the file alone, so it runs at compile. What
//! needs the entity's units (strata that fail to partition them) is
//! `units::check_strata`.

use super::ConfigError;
use crate::config::{ExperimentConfig, LeverState, PeriodConfig, SwitchbackConfig};
use crate::grain::Grain;
use std::collections::{HashMap, HashSet};

pub(super) fn check_design(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    match &c.design.switchback {
        Some(sb) => check_switchback(c, sb, out),
        None => {
            check_waves_window(c, out);
            check_carryover(c, out);
        }
    }
    check_strata_disjoint(c, out);
    check_period_starts(c, out);
}

/// Every wave's `switch_on` and every switchback period's `from` opens a period
/// of `design.grain`. Always true at day grain.
fn check_period_starts(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let grain = c.design.grain;
    let waves = c
        .design
        .waves
        .iter()
        .enumerate()
        .map(|(i, w)| (format!("design.waves[{}].switch_on", i + 1), w.switch_on));
    let periods = c.design.switchback.iter().flat_map(|sb| {
        sb.periods
            .iter()
            .enumerate()
            .map(|(i, p)| (format!("design.switchback.periods[{}].from", i + 1), p.from))
    });
    out.extend(
        waves
            .chain(periods)
            .filter(|(_, date)| !grain.starts(*date))
            .map(|(key, date)| ConfigError::NotAPeriodStart { key, date, grain }),
    );
}

/// A carryover window reads the days after the close `off`s, the same lag after
/// every unit's switch: one switch date, and levers that do turn off. A draft
/// (no waves yet) passes; registering it is what fixes the date.
fn check_carryover(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let Some(days) = c.design.carryover_days else {
        return;
    };
    if days == 0 {
        out.push(ConfigError::CarryoverZero);
    }
    let dates: std::collections::BTreeSet<_> = c.design.waves.iter().map(|w| w.switch_on).collect();
    if dates.len() > 1 {
        out.push(ConfigError::CarryoverNeedsOneSwitchDate {
            switch_dates: dates.len(),
        });
    }
    if c.on_close() == crate::config::OnClose::Keep {
        out.push(ConfigError::CarryoverWithLeversKept);
    }
}

fn check_switchback(c: &ExperimentConfig, sb: &SwitchbackConfig, out: &mut Vec<ConfigError>) {
    if !c.design.waves.is_empty() {
        out.push(ConfigError::WavesAndSwitchback);
    }
    out.extend(
        forbidden_keys(c)
            .into_iter()
            .map(|key| ConfigError::SwitchbackForbids { key }),
    );
    if sb.period_days == 0 {
        out.push(ConfigError::SwitchbackPeriodDaysZero);
        return;
    }
    let (washout_days, grace_days) = (c.design.washout_days, c.grace_days());
    if washout_days >= sb.period_days {
        out.push(ConfigError::SwitchbackWashoutTooLong {
            washout_days,
            period_days: sb.period_days,
        });
    }
    // Compared in hours, so a period shorter than a day still allows no grace.
    if grace_days.saturating_mul(24) >= min_hours(c.design.grain, sb.period_days) {
        out.push(ConfigError::SwitchbackGraceTooLong {
            grace_days,
            period_days: sb.period_days,
        });
    }
    if let Some(why) = pairing_fault(sb, c.design.grain) {
        out.push(ConfigError::PeriodsNotPaired { why });
    }
}

/// The fewest hours `periods` periods of `grain` can span: the read-back
/// grace is in days whatever the grain, because it is how long the lever takes.
fn min_hours(grain: Grain, periods: u32) -> u32 {
    match grain {
        Grain::Hour => periods,
        _ => periods.saturating_mul(grain.min_days()).saturating_mul(24),
    }
}

/// The keys set on a switchback file that mean nothing to a fleet-wide
/// alternation. `design.waves` is `WavesAndSwitchback`'s, not listed here.
fn forbidden_keys(c: &ExperimentConfig) -> Vec<&'static str> {
    let d = &c.design;
    [
        ("design.strata", d.strata.is_some()),
        ("assign_by", c.assign_by.is_some()),
        ("design.pre_days", d.pre_days.is_some()),
        ("design.post_days", d.post_days.is_some()),
        (
            "design.anticipation_days (non-zero)",
            d.anticipation_days != 0,
        ),
        ("design.carryover_days", d.carryover_days.is_some()),
    ]
    .into_iter()
    .filter_map(|(key, set)| set.then_some(key))
    .collect()
}

/// The first reason the periods are not consecutive on/off pairs, judged in
/// file order (file order is the schedule order everywhere; nothing sorts).
/// Empty `periods` is the draft: no fault.
fn pairing_fault(sb: &SwitchbackConfig, grain: Grain) -> Option<String> {
    let periods: &[PeriodConfig] = &sb.periods;
    if !periods.len().is_multiple_of(2) {
        return Some(format!(
            "it lists {} periods, which is not an even number",
            periods.len()
        ));
    }
    if let Some((i, w)) = periods
        .windows(2)
        .enumerate()
        .find(|(_, w)| w[0].from >= w[1].from)
    {
        return Some(format!(
            "periods are not listed in date order: period {} starts {} and period {} starts {}",
            i + 1,
            w[0].from,
            i + 2,
            w[1].from
        ));
    }
    for (i, w) in periods.windows(2).enumerate() {
        let expected = grain.later(w[0].from, sb.period_days);
        if expected != Some(w[1].from) {
            return Some(format!(
                "period {} starts {} and period {} starts {}, which is not {} {}s later",
                i + 1,
                w[0].from,
                i + 2,
                w[1].from,
                sb.period_days,
                grain.as_str()
            ));
        }
    }
    periods.chunks(2).enumerate().find_map(|(k, pair)| {
        (pair[0].state == pair[1].state).then(|| {
            let state = match pair[0].state {
                LeverState::On => "on",
                LeverState::Off => "off",
            };
            format!(
                "pair {} (from {}) is {state} in both periods",
                k + 1,
                pair[0].from
            )
        })
    })
}

/// A waves design needs both windows; the type makes them optional only so a
/// switchback can omit them. A file with no waves and no switchback is a draft
/// of a shape not yet chosen and owes no window yet.
fn check_waves_window(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    if c.design.waves.is_empty() {
        return;
    }
    if c.design.pre_days.is_none() {
        out.push(ConfigError::MissingWindow {
            key: "design.pre_days",
        });
    }
    if c.design.post_days.is_none() {
        out.push(ConfigError::MissingWindow {
            key: "design.post_days",
        });
    }
}

/// A name in two different strata, once per name. A name repeated inside one
/// stratum is not this refusal.
fn check_strata_disjoint(c: &ExperimentConfig, out: &mut Vec<ConfigError>) {
    let Some(strata) = &c.design.strata else {
        return;
    };
    let mut first_stratum: HashMap<&str, usize> = HashMap::new();
    let mut reported: HashSet<&str> = HashSet::new();
    for (i, stratum) in strata.iter().enumerate() {
        for unit in stratum {
            match first_stratum.get(unit.as_str()) {
                Some(&first) if first != i && reported.insert(unit.as_str()) => {
                    out.push(ConfigError::UnitInTwoStrata { unit: unit.clone() });
                }
                Some(_) => {}
                None => {
                    first_stratum.insert(unit.as_str(), i);
                }
            }
        }
    }
}
