//! Read-back: did each flip land, and did the lever reach a control? Plus the
//! last post window's coverage, which `closing` waits on. A value that cannot
//! be read is a `problem` on the run — never a silent skip, never an error
//! that stops the rest of the check.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use entity::{experiment_flips, experiment_runs};
use oxy_experiment::compliance::{FlipOutcome, Sampling, classify_treated, first_leak};
use oxy_experiment::expr::{Comparison, parse_comparison};
use oxy_experiment::timeline::{last_post_window_end, last_post_window_end_date};
use oxy_experiment::{ExperimentConfig, Grain, Moment};
use sea_orm::{DatabaseConnection, DbErr};

use super::reader::UnitDays;
use crate::server::experiment::panel::GroupMembership;
use crate::server::experiment::store::{self, Observations};

pub struct ReadBackInputs<'a> {
    pub run: &'a experiment_runs::Model,
    pub c: &'a ExperimentConfig,
    pub flips: &'a [experiment_flips::Model],
    pub today: NaiveDate,
    /// Now in `design.timezone`, floored to the hour: the reading in progress,
    /// never read. Its date is `today`.
    pub now: Moment,
    /// Under `assign_by`: the pinned members of every group.
    pub membership: Option<&'a GroupMembership>,
    /// A switchback's pinned fleet.
    pub fleet: Option<&'a [String]>,
}

/// The warehouse units a design unit is read at: itself, or a group's pinned
/// members — never the dimension's current value.
pub fn subjects<'a>(inp: &ReadBackInputs<'a>, unit: &'a str) -> Vec<&'a str> {
    match inp.membership.and_then(|g| g.0.get(unit)) {
        Some(members) => members.iter().map(String::as_str).collect(),
        None => vec![unit],
    }
}

/// `cmp` per reading at `units` (one unit, or a group's members) over the
/// moments `[from, to]`: daily readings keyed by midnight, or hourly ones on an
/// hour-grain design. Only the readings the warehouse returned.
pub(super) async fn held(
    days: &dyn UnitDays,
    cmp: &Comparison,
    units: &[String],
    (from, to): (Moment, Moment),
    sampling: Sampling,
) -> Result<BTreeMap<Moment, bool>, String> {
    let values = days
        .readings(&cmp.measure, units, from.date(), to.date(), sampling)
        .await?;
    let first = sampling.floor(from);
    Ok(values
        .into_iter()
        .filter(|(m, _)| *m >= first && *m <= to)
        .map(|(m, v)| (m, cmp.holds(v)))
        .collect())
}

/// The last reading inside a treated unit's grace: `grace_days` days after its switch.
pub(super) fn grace_end(switch_on: Moment, grace_days: u32) -> Moment {
    switch_on
        .plus_hours(24 * i64::from(grace_days))
        .unwrap_or(switch_on)
}

pub async fn treated(
    db: &DatabaseConnection,
    inp: &ReadBackInputs<'_>,
    days: &dyn UnitDays,
    problems: &mut Vec<String>,
) -> Result<(), DbErr> {
    let Some(rb) = inp.c.read_back() else {
        problems.push("the design has no lever, so there is nothing to read back".into());
        return Ok(());
    };
    let cmp = match parse_comparison(&rb.when_on) {
        Ok(cmp) => cmp,
        Err(e) => {
            problems.push(format!("lever.read_back.when_on: {e}"));
            return Ok(());
        }
    };
    let grace = inp.c.grace_days();
    let sampling = Sampling::of(inp.c);
    for flip in inp
        .flips
        .iter()
        .filter(|f| f.target_state == "on" && f.status == "dispatched")
    {
        // An `on` is due on its wave's `switch_on` (Task 1).
        let switch_on = Moment(flip.due_at);
        let to = grace_end(switch_on, grace).min(inp.now);
        if to < switch_on {
            continue;
        }
        let unit = [flip.unit.clone()];
        let held = match held(days, &cmp, &unit, (switch_on, to), sampling).await {
            Ok(held) => held,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        match classify_treated(switch_on, grace, sampling, inp.now, &held) {
            Some(FlipOutcome::Landed { on_day }) => {
                store::settle_flip(db, flip.id, "confirmed", Some(on_day), None).await?
            }
            Some(FlipOutcome::NeverLanded) => {
                let why = format!(
                    "`{}` never held at `{}` within grace_days {grace} of {switch_on}",
                    rb.when_on, flip.unit
                );
                store::settle_flip(db, flip.id, "unconfirmed", None, Some(&why)).await?
            }
            _ => {}
        }
    }
    Ok(())
}

pub async fn controls(
    inp: &ReadBackInputs<'_>,
    pool: &[String],
    days: &dyn UnitDays,
    obs: &mut Observations,
) {
    if obs.controls_checked_on == Some(inp.today) {
        return;
    }
    // Controls are read per day whatever the grain: they never switch.
    let Some(first) = inp.c.design.waves.iter().map(|w| w.switch_on.date()).min() else {
        return;
    };
    if inp.today < first {
        return;
    }
    let Some(rb) = inp.c.read_back() else {
        obs.problems
            .push("the design has no lever, so there is nothing to read back".into());
        return;
    };
    let cmp = match parse_comparison(&rb.when_off) {
        Ok(cmp) => cmp,
        Err(e) => {
            obs.problems.push(format!("lever.read_back.when_off: {e}"));
            return;
        }
    };
    let to = last_post_window_end_date(inp.c)
        .and_then(|end| end.pred_opt())
        .map_or(inp.today, |last| last.min(inp.today));
    let leaked: BTreeSet<String> = obs.control_leaks.keys().cloned().collect();
    for unit in pool.iter().filter(|u| !leaked.contains(*u)) {
        match days.values(&cmp.measure, unit, first, to).await {
            Ok(values) => {
                let held = values
                    .into_iter()
                    .map(|(day, v)| (Moment::midnight(day), cmp.holds(v)))
                    .collect();
                if let Some(at) = first_leak(&held) {
                    obs.control_leaks.insert(unit.clone(), at.date());
                }
            }
            Err(e) => obs.problems.push(e),
        }
    }
    obs.controls_checked_on = Some(inp.today);
}

/// The last window `closing` waits on: a waves design's last post window over
/// the last wave's units (a group's members), or a switchback's last period
/// over its fleet. `(first day, length in periods of [`coverage_grain`], units)`.
/// Coverage is read per day at hour grain, so the window is the days it touches.
fn last_window(inp: &ReadBackInputs<'_>) -> Option<(NaiveDate, u32, Vec<String>)> {
    let end = last_post_window_end(inp.c)?;
    let grain = inp.c.design.grain;
    let span = |n: u32| -> Option<(NaiveDate, u32)> {
        let start = grain.earlier(end, n)?;
        if grain != Grain::Hour {
            return Some((start.date(), n));
        }
        let days = (end.ceil_date() - start.date()).num_days();
        Some((start.date(), u32::try_from(days).ok()?))
    };
    if let Some(sb) = &inp.c.design.switchback {
        let (start, len) = span(sb.period_days)?;
        return Some((start, len, inp.fleet?.to_vec()));
    }
    let post = inp.c.design.post_days?;
    let last_wave = inp.c.design.waves.iter().max_by_key(|w| w.switch_on)?;
    let units = last_wave
        .units
        .iter()
        .flat_map(|u| subjects(inp, u))
        .map(str::to_string)
        .collect();
    let (start, len) = span(post)?;
    Some((start, len, units))
}

/// The grain the closing coverage counts in: the design's, except hour, whose
/// coverage is judged on the daily values read-back reads.
fn coverage_grain(g: Grain) -> Grain {
    if g == Grain::Hour { Grain::Day } else { g }
}

pub async fn coverage(inp: &ReadBackInputs<'_>, days: &dyn UnitDays, obs: &mut Observations) {
    let Some((start, len, units)) = last_window(inp) else {
        return;
    };
    if len == 0 || inp.today < start {
        return;
    }
    let grain = coverage_grain(inp.c.design.grain);
    let to = grain
        .add(start, len)
        .and_then(|end| end.pred_opt())
        .map_or(inp.today, |last| last.min(inp.today));
    // Coverage counts periods: a period is in once any of its days is.
    let mut seen = BTreeSet::new();
    for unit in &units {
        match days.values(&inp.c.outcome.target, unit, start, to).await {
            Ok(values) => seen.extend(values.into_keys().map(|d| grain.floor(d))),
            Err(e) => {
                obs.problems.push(e);
                return;
            }
        }
        if seen.len() == len as usize {
            break; // every period is in: the rest cannot add one
        }
    }
    obs.coverage = Some(seen.len() as f64 / f64::from(len));
}
