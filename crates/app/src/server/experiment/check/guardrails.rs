//! Guardrails: safety stops watched per treated unit while the effect stays
//! sealed. `anomaly` runs the monitor detector (`scan_one`) once per watched
//! unit over its own series — or a group's pinned members — on the side of the
//! band that hurts, and breaches only on [`SUSTAINED_DAYS`] consecutive flagged
//! treated days: one flag is a chance event across many units and days. `trigger`
//! tests the `{{measure}}` comparison against each unit's latest returned
//! treated day, reading through YESTERDAY: today is still loading, so a
//! partial total is never evidence. Any one unit breaching is the breach, and
//! names the unit.

use std::collections::BTreeMap;

use chrono::{DateTime, Days, NaiveDate, Utc};
use entity::experiment_flips;
use oxy_experiment::Moment;
use oxy_experiment::config::{GuardDirection, GuardrailCriteria, SwitchbackConfig};
use oxy_experiment::expr::{Trigger, parse_trigger};
use oxy_experiment::run::LeverState;
use oxy_experiment::{ExperimentConfig, Grain};

use super::Readers;
use super::reader::AnomalyQuery;
use crate::server::experiment::panel::GroupMembership;
use crate::server::experiment::store::{GuardrailState, GuardrailStatus, UnitGuardrail};

pub const TRIGGER_LOOKBACK_DAYS: u64 = 7;
/// Consecutive flagged treated days a unit needs before `anomaly` breaches. At
/// 2σ a day is flagged by chance about one time in twenty, so 40 units over a
/// month flag one day almost surely; two in a row at one unit is ~1 in 400 a day.
pub const SUSTAINED_DAYS: u64 = 2;
/// Two-sided coverage of ±3σ and ±5σ — the detector's `medium` and `low` cutoffs.
const SIG_3_SIGMA: f64 = 0.9973;
const SIG_5_SIGMA: f64 = 0.999_999_4;

pub fn sensitivity_for(significance: f64) -> Result<&'static str, String> {
    if !(significance > 0.0 && significance < 1.0) {
        return Err(format!(
            "anomaly significance {significance} is not in (0, 1)"
        ));
    }
    Ok(if significance < SIG_3_SIGMA {
        "high"
    } else if significance < SIG_5_SIGMA {
        "medium"
    } else {
        "low"
    })
}

/// One treated unit as the guardrails see it: its name, the warehouse units
/// its value is read over, and the `[from, until)` spans it counts as treated.
#[derive(Debug, Clone, PartialEq)]
pub struct Watched {
    pub unit: String,
    pub members: Vec<String>,
    pub treated: Vec<(NaiveDate, NaiveDate)>,
}

impl Watched {
    pub fn treats(&self, day: NaiveDate) -> bool {
        self.treated
            .iter()
            .any(|(from, until)| *from <= day && day < *until)
    }

    fn since(&self) -> Option<NaiveDate> {
        self.treated.iter().map(|(from, _)| *from).min()
    }
}

pub fn watched(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
    membership: Option<&GroupMembership>,
    fleet: Option<&[String]>,
    today: NaiveDate,
) -> Vec<Watched> {
    match &c.design.switchback {
        Some(sb) => switchback_watched(sb, c.design.grain, flips, fleet.unwrap_or_default(), today),
        None => wave_watched(flips, membership),
    }
}

/// Units (or groups) whose `on` was sent, has not failed, and has no `off`
/// since — treated from their switch day on.
fn wave_watched(
    flips: &[experiment_flips::Model],
    membership: Option<&GroupMembership>,
) -> Vec<Watched> {
    let off_since = |on: &experiment_flips::Model| {
        flips
            .iter()
            .any(|f| f.unit == on.unit && f.target_state == "off" && f.due_at >= on.due_at)
    };
    flips
        .iter()
        .filter(|f| f.target_state == "on" && f.task_id.is_some() && f.status != "failed")
        .filter(|f| !off_since(f))
        .map(|f| Watched {
            unit: f.unit.clone(),
            members: membership
                .and_then(|g| g.0.get(&f.unit).cloned())
                .unwrap_or_else(|| vec![f.unit.clone()]),
            treated: vec![(f.due_at.date(), NaiveDate::MAX)],
        })
        .collect()
}

/// Every fleet unit, treated on the on periods that have begun **and whose `on` went out
/// to it** — sent and not failed, as [`wave_watched`] asks. An `on` cancelled because its
/// period ended before dispatch, or one that failed, left that period's days untreated.
fn switchback_watched(
    sb: &SwitchbackConfig,
    grain: Grain,
    flips: &[experiment_flips::Model],
    fleet: &[String],
    today: NaiveDate,
) -> Vec<Watched> {
    let sent = |unit: &str, from: Moment| {
        flips.iter().any(|f| {
            f.unit == unit
                && f.target_state == "on"
                && Moment(f.due_at) == from
                && f.task_id.is_some()
                && f.status != "failed"
                && f.status != "cancelled"
        })
    };
    fleet
        .iter()
        .filter_map(|u| {
            let treated: Vec<(NaiveDate, NaiveDate)> = sb
                .periods
                .iter()
                .filter(|p| p.state == LeverState::On && p.from.date() <= today && sent(u, p.from))
                // Guardrails read days: a day is treated if any hour of the period is in it.
                .filter_map(|p| {
                    let until = grain.later(p.from, sb.period_days)?;
                    Some((p.from.date(), until.ceil_date()))
                })
                .collect();
            (!treated.is_empty()).then(|| Watched {
                unit: u.clone(),
                members: vec![u.clone()],
                treated,
            })
        })
        .collect()
}

pub struct GuardrailInputs<'a> {
    pub c: &'a ExperimentConfig,
    pub watched: &'a [Watched],
    pub today: NaiveDate,
    pub now: DateTime<Utc>,
}

pub async fn evaluate(
    inp: &GuardrailInputs<'_>,
    readers: &Readers<'_>,
) -> (Vec<GuardrailStatus>, Option<String>) {
    let mut statuses = Vec::new();
    let mut breach = None;
    for g in &inp.c.guardrails {
        let status = match &g.criteria {
            GuardrailCriteria::Anomaly {
                measure,
                significance,
                direction,
            } => {
                let direction = inp.c.anomaly_direction(measure, *direction);
                anomaly(inp, readers, measure, *significance, direction).await
            }
            GuardrailCriteria::Trigger { when } => trigger(inp, readers, when).await,
        };
        if status.status == GuardrailState::Breached && breach.is_none() {
            breach = status.detail.clone();
        }
        statuses.push(status);
    }
    (statuses, breach)
}

fn unit(w: &Watched, status: GuardrailState, detail: Option<String>) -> UnitGuardrail {
    UnitGuardrail {
        unit: w.unit.clone(),
        status,
        detail,
    }
}

/// A criterion's status from its units: breached if any unit is (that
/// unit's detail), else not checked if any unit was not, else ok.
fn roll_up(criteria: String, units: Vec<UnitGuardrail>) -> GuardrailStatus {
    let not_checked = units
        .iter()
        .filter(|u| u.status == GuardrailState::NotChecked)
        .count();
    let (status, detail) = match units.iter().find(|u| u.status == GuardrailState::Breached) {
        Some(b) => (GuardrailState::Breached, b.detail.clone()),
        None if units.is_empty() => (
            GuardrailState::NotChecked,
            Some("no treated unit is live".into()),
        ),
        None if not_checked > 0 => (
            GuardrailState::NotChecked,
            Some(format!(
                "{not_checked} of {} treated units not checked",
                units.len()
            )),
        ),
        None => (GuardrailState::Ok, None),
    };
    GuardrailStatus {
        criteria,
        status,
        detail,
        units,
    }
}

fn refused(criteria: String, why: String) -> GuardrailStatus {
    GuardrailStatus {
        criteria,
        status: GuardrailState::Refused,
        detail: Some(why),
        units: Vec::new(),
    }
}

async fn anomaly(
    inp: &GuardrailInputs<'_>,
    readers: &Readers<'_>,
    measure: &str,
    significance: f64,
    direction: GuardDirection,
) -> GuardrailStatus {
    let criteria = format!(
        "anomaly on {measure} at significance {significance} ({})",
        direction.as_str()
    );
    if let Err(e) = sensitivity_for(significance) {
        return refused(criteria, e);
    }
    let mut units = Vec::new();
    for w in inp.watched {
        let Some(since) = w.since() else { continue };
        let q = AnomalyQuery {
            measure,
            members: &w.members,
            significance,
            direction,
            since,
            now: inp.now,
        };
        units.push(match readers.anomalies.flagged(&q).await {
            Ok(flags) => anomaly_unit(w, measure, &flags),
            Err(e) => unit(w, GuardrailState::NotChecked, Some(e)),
        });
    }
    roll_up(criteria, units)
}

/// One unit's anomaly verdict from its flagged days: breached on the first run of
/// [`SUSTAINED_DAYS`] consecutive flagged treated days; a shorter run is ok, and
/// says so, since the next day may complete it.
fn anomaly_unit(w: &Watched, measure: &str, flags: &[(NaiveDate, String)]) -> UnitGuardrail {
    let flagged: BTreeMap<NaiveDate, &str> = flags
        .iter()
        .filter(|(day, _)| w.treats(*day))
        .map(|(day, detail)| (*day, detail.as_str()))
        .collect();
    let sustained = flagged.keys().find(|day| {
        (1..SUSTAINED_DAYS).all(|k| {
            day.checked_add_days(Days::new(k))
                .is_some_and(|next| flagged.contains_key(&next))
        })
    });
    match (sustained, flagged.iter().next_back()) {
        (Some(from), _) => {
            let last = *from + Days::new(SUSTAINED_DAYS - 1);
            let detail = format!(
                "anomaly on `{measure}` at `{}` on {from} through {last}: {}",
                w.unit, flagged[&last]
            );
            unit(w, GuardrailState::Breached, Some(detail))
        }
        (None, Some((day, detail))) => {
            let detail = format!(
                "flagged on {day} ({detail}); trips on {SUSTAINED_DAYS} flagged days in a row"
            );
            unit(w, GuardrailState::Ok, Some(detail))
        }
        (None, None) => unit(w, GuardrailState::Ok, None),
    }
}

async fn trigger(inp: &GuardrailInputs<'_>, readers: &Readers<'_>, when: &str) -> GuardrailStatus {
    let criteria = format!("trigger {when}");
    let trig = match parse_trigger(when) {
        Ok(t) => t,
        Err(e) => return refused(criteria, e.to_string()),
    };
    // Through yesterday, never today's partly loaded day.
    let through = inp.today - Days::new(1);
    let from = inp.today - Days::new(TRIGGER_LOOKBACK_DAYS);
    let mut units = Vec::new();
    for w in inp.watched {
        units.push(match trigger_values(readers, &trig, &w.members, from, through).await {
            // The latest day through yesterday that ARRIVED, was treated and has a reading.
            Ok(values) => match values.iter().rev().find(|(day, _)| w.treats(**day)) {
                Some((day, value)) if trig.holds(*value) => {
                    let detail = format!("`{when}` held at `{}` on {day} (value {value})", w.unit);
                    unit(w, GuardrailState::Breached, Some(detail))
                }
                Some(_) => unit(w, GuardrailState::Ok, None),
                None => {
                    let why = format!("no treated day of `{}` has arrived in the last {TRIGGER_LOOKBACK_DAYS} days", w.unit);
                    unit(w, GuardrailState::NotChecked, Some(why))
                }
            },
            Err(e) => unit(w, GuardrailState::NotChecked, Some(e)),
        });
    }
    roll_up(criteria, units)
}

/// The trigger's left-hand side per day: one measure's values, or a ratio's on the
/// days both parts arrived and the denominator is not 0. Over a group, each part is
/// the members' sum, so a ratio is the ratio of the sums.
async fn trigger_values(
    readers: &Readers<'_>,
    trig: &Trigger,
    members: &[String],
    from: NaiveDate,
    through: NaiveDate,
) -> Result<BTreeMap<NaiveDate, f64>, String> {
    let mut parts = Vec::new();
    for m in trig.measures() {
        parts.push(readers.days.values_over(m, members, from, through).await?);
    }
    let Some((first, rest)) = parts.split_first() else {
        return Ok(BTreeMap::new());
    };
    Ok(first
        .iter()
        .filter_map(|(day, v)| {
            let mut values = vec![*v];
            for p in rest {
                values.push(*p.get(day)?);
            }
            trig.value(&values).map(|x| (*day, x))
        })
        .collect())
}

#[cfg(test)]
#[path = "guardrails_tests.rs"]
mod tests;
