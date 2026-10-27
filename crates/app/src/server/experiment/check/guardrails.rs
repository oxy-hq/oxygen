//! Guardrails: safety stops watched per treated unit while the effect stays
//! sealed. `anomaly` runs the monitor detector (`scan_one`) once per watched
//! unit over its own series — or a group's pinned members — and `trigger`
//! tests the `{{measure}}` comparison against each unit's latest returned
//! treated day, reading through YESTERDAY: today is still loading, so a
//! partial total is never evidence. Any one unit breaching is the breach, and
//! names the unit.

use std::collections::BTreeMap;

use chrono::{DateTime, Days, NaiveDate, Utc};
use entity::experiment_flips;
use oxy_experiment::Moment;
use oxy_experiment::config::{GuardrailCriteria, SwitchbackConfig};
use oxy_experiment::expr::{Trigger, parse_trigger};
use oxy_experiment::run::LeverState;
use oxy_experiment::{ExperimentConfig, Grain};

use super::Readers;
use super::reader::AnomalyQuery;
use crate::server::experiment::panel::GroupMembership;
use crate::server::experiment::store::{GuardrailState, GuardrailStatus, UnitGuardrail};

pub const TRIGGER_LOOKBACK_DAYS: u64 = 7;
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
            } => anomaly(inp, readers, measure, *significance).await,
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
) -> GuardrailStatus {
    let criteria = format!("anomaly on {measure} at significance {significance}");
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
            since,
            now: inp.now,
        };
        units.push(match readers.anomalies.flagged(&q).await {
            Ok(flags) => match flags.into_iter().find(|(day, _)| w.treats(*day)) {
                Some((day, detail)) => {
                    let detail =
                        format!("anomaly on `{measure}` at `{}` on {day}: {detail}", w.unit);
                    unit(w, GuardrailState::Breached, Some(detail))
                }
                None => unit(w, GuardrailState::Ok, None),
            },
            Err(e) => unit(w, GuardrailState::NotChecked, Some(e)),
        });
    }
    roll_up(criteria, units)
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
mod tests {
    use chrono::Utc;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    use super::*;
    use crate::server::experiment::test_design::{design, switchback};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn flip(unit: &str, state: &str, due: &str, status: &str) -> experiment_flips::Model {
        experiment_flips::Model {
            id: Uuid::new_v4(),
            run_id: Uuid::nil(),
            unit: unit.into(),
            target_state: state.into(),
            due_at: d(due).and_hms_opt(0, 0, 0).unwrap(),
            wave: 0,
            task_id: Some(Uuid::nil()),
            agentic_run_id: Some(Uuid::nil()),
            lever_revision_id: None,
            status: status.into(),
            confirmed_on: None,
            reason: None,
            created_at: Utc::now().fixed_offset(),
        }
    }

    #[test]
    fn significance_selects_the_most_sensitive_cutoff_it_does_not_undercut() {
        assert_eq!(sensitivity_for(0.95).unwrap(), "high");
        assert_eq!(sensitivity_for(0.99).unwrap(), "high");
        assert_eq!(sensitivity_for(0.9973).unwrap(), "medium");
        assert_eq!(sensitivity_for(0.999).unwrap(), "medium");
        assert_eq!(sensitivity_for(0.999_999_9).unwrap(), "low");
    }

    #[test]
    fn significance_outside_the_open_unit_interval_is_refused() {
        for bad in [0.0, 1.0, -0.5, f64::NAN] {
            let err = sensitivity_for(bad).unwrap_err();
            assert!(err.contains("not in (0, 1)"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_waves_unit_is_watched_from_its_switch_day_while_its_on_stands() {
        let flips = [
            flip("bondi", "on", "2026-03-02", "confirmed"),
            flip("newtown", "on", "2026-03-02", "failed"),
            flip("manly", "on", "2026-03-16", "dispatched"),
            flip("manly", "off", "2026-03-20", "pending"),
        ];
        let w = watched(&design(), &flips, None, None, d("2026-03-21"));
        assert_eq!(
            w.iter().map(|w| w.unit.as_str()).collect::<Vec<_>>(),
            vec!["bondi"]
        );
        assert_eq!(w[0].members, vec!["bondi"]);
        assert!(w[0].treats(d("2026-03-02")) && !w[0].treats(d("2026-03-01")));
    }

    #[test]
    fn a_group_is_watched_over_its_pinned_members() {
        let flips = [flip("east", "on", "2026-03-02", "confirmed")];
        let pinned = GroupMembership(BTreeMap::from([(
            "east".to_string(),
            vec!["bondi".to_string(), "coogee".to_string()],
        )]));
        let w = watched(&design(), &flips, Some(&pinned), None, d("2026-03-05"));
        assert_eq!(
            (w[0].unit.as_str(), w[0].members.clone()),
            ("east", vec!["bondi".to_string(), "coogee".to_string()])
        );
    }

    #[test]
    fn a_switchback_unit_is_treated_on_its_begun_on_periods_only() {
        let fleet = ["bondi".to_string(), "glebe".to_string()];
        assert!(
            watched(&switchback(), &[], None, Some(&fleet), d("2026-03-01")).is_empty(),
            "nothing has begun"
        );
        let flips: Vec<_> = ["bondi", "glebe"]
            .into_iter()
            .flat_map(|u| {
                [
                    flip(u, "on", "2026-03-02", "confirmed"),
                    flip(u, "on", "2026-03-23", "dispatched"),
                ]
            })
            .collect();
        let w = watched(&switchback(), &flips, None, Some(&fleet), d("2026-03-24"));
        assert_eq!(w.len(), 2, "every fleet unit");
        assert!(w[0].treats(d("2026-03-08")), "period 0 is on");
        assert!(
            !w[0].treats(d("2026-03-09")) && !w[0].treats(d("2026-03-22")),
            "periods 1 and 2 are off"
        );
        assert!(w[0].treats(d("2026-03-23")), "period 3 is on and has begun");
    }

    #[test]
    fn a_switchback_on_period_whose_on_never_went_out_is_not_watched() {
        // glebe's first `on` was cancelled (its period ended before dispatch) and bondi's
        // second failed: neither is a treated day, so a trigger there cannot abort the run.
        let fleet = ["bondi".to_string(), "glebe".to_string()];
        let mut cancelled = flip("glebe", "on", "2026-03-02", "cancelled");
        cancelled.task_id = None;
        cancelled.reason = Some(crate::server::experiment::send::PERIOD_ENDED.into());
        let flips = [
            flip("bondi", "on", "2026-03-02", "confirmed"),
            cancelled,
            flip("bondi", "on", "2026-03-23", "failed"),
            flip("glebe", "on", "2026-03-23", "dispatched"),
        ];
        let w = watched(&switchback(), &flips, None, Some(&fleet), d("2026-03-24"));
        let bondi = w.iter().find(|w| w.unit == "bondi").expect("bondi");
        assert!(bondi.treats(d("2026-03-02")));
        assert!(!bondi.treats(d("2026-03-23")), "its on failed");
        let glebe = w.iter().find(|w| w.unit == "glebe").expect("glebe");
        assert!(!glebe.treats(d("2026-03-02")), "its on was never sent");
        assert!(glebe.treats(d("2026-03-23")));
    }
}
