//! Which unsent rows one lever task covers, and what it is told (C2b "Lever
//! variables"). Pure: rows in, batches and variables out.

use std::collections::BTreeMap;

use entity::experiment_flips;
use oxy_experiment::Moment;
use oxy_experiment::compliance::PeriodSpan;
use oxy_experiment::config::{LeverPer, SwitchbackConfig};
use oxy_experiment::run::LeverState;
use oxy_experiment::{ExperimentConfig, Grain};
use serde_json::{Value, json};
use uuid::Uuid;

use super::panel::GroupMembership;

/// One lever task's worth of rows: one row under `per: unit`; every unsent
/// row of one `(wave, state, due_at)` under `per: wave`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub state: LeverState,
    pub wave: i32,
    pub due_at: Moment,
    pub ids: Vec<Uuid>,
    /// Sorted, so a task's `units` and the stamp's `RETURNING` compare equal.
    pub units: Vec<String>,
}

/// Ordered by due day, so after a clock jump the earlier periods go first.
/// A row whose state does not parse is skipped (nothing writes one).
pub fn batches(c: &ExperimentConfig, unsent: &[&experiment_flips::Model]) -> Vec<Batch> {
    let per_wave = matches!(c.lever_per(), LeverPer::Wave);
    let mut out: BTreeMap<(Moment, i32, &str, Option<Uuid>), Batch> = BTreeMap::new();
    for f in unsent {
        let Some(state) = LeverState::parse(&f.target_state) else {
            continue;
        };
        let solo = (!per_wave).then_some(f.id);
        let key = (Moment(f.due_at), f.wave, state.as_str(), solo);
        let b = out.entry(key).or_insert_with(|| Batch {
            state,
            wave: f.wave,
            due_at: Moment(f.due_at),
            ids: Vec::new(),
            units: Vec::new(),
        });
        b.ids.push(f.id);
        b.units.push(f.unit.clone());
    }
    out.into_values()
        .map(|mut b| {
            b.units.sort();
            b
        })
        .collect()
}

/// The lever's variables. `per: unit` → `{unit, state, experiment, wave}`;
/// `per: wave` → `{units, state, experiment, wave}`. Under `assign_by`,
/// `members` is added from the PINNED membership — the group's members (per
/// unit) or `{group: [members]}` for the batch's groups (per wave) — never the
/// dimension's current value. `Ok(None)`: not pinned yet, wait. `Err`: a
/// group the pinned membership does not hold; fail the batch.
pub fn variables(
    c: &ExperimentConfig,
    experiment: &str,
    b: &Batch,
    membership: Option<&GroupMembership>,
) -> Result<Option<Value>, String> {
    let per_wave = matches!(c.lever_per(), LeverPer::Wave);
    let (state, wave) = (b.state.as_str(), b.wave);
    let mut v = if per_wave {
        json!({ "units": b.units, "state": state, "experiment": experiment, "wave": wave })
    } else {
        json!({ "unit": b.units[0], "state": state, "experiment": experiment, "wave": wave })
    };
    let Some(dimension) = c.assign_by.as_deref() else {
        return Ok(Some(v));
    };
    let Some(GroupMembership(groups)) = membership else {
        return Ok(None);
    };
    let mut members = BTreeMap::new();
    for group in &b.units {
        let Some(m) = groups.get(group) else {
            return Err(format!(
                "group `{group}` has no pinned members: the first check did not find it under `{dimension}`"
            ));
        };
        members.insert(group.as_str(), m);
    }
    v["members"] = if per_wave {
        json!(members)
    } else {
        json!(members.into_values().next())
    };
    Ok(Some(v))
}

/// The period `flip` is the boundary of: its due day starts a period whose
/// state is the flip's. A terminal `off` (dated the day it was decided) is
/// usually none, but one decided on the first day of an `off` period matches
/// that period; held past the period's end it is cancelled, and `needs_off`
/// ignores cancelled rows, so the Aborted/Reported step re-emits it.
pub fn period_span(
    sb: &SwitchbackConfig,
    grain: Grain,
    flip: &experiment_flips::Model,
) -> Option<PeriodSpan> {
    let state = LeverState::parse(&flip.target_state)?;
    let from = Moment(flip.due_at);
    sb.periods
        .iter()
        .find(|p| p.from == from && p.state == state)?;
    let until = grain.later(from, sb.period_days)?;
    Some(PeriodSpan { from, until, state })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::server::experiment::test_design::{YAML, design, switchback};

    const PER_WAVE: (&str, &str) = ("  on_close: off", "  per: wave\n  on_close: off");
    const GROUPS: (&str, &str) = (
        "for_each: store\n",
        "for_each: store\nassign_by: store.region\n",
    );

    fn edited(edits: &[(&str, &str)]) -> ExperimentConfig {
        let yaml = edits.iter().fold(YAML.to_string(), |y, (from, to)| {
            let e = y.replace(from, to);
            assert_ne!(e, y, "edit `{from}` matched nothing");
            e
        });
        oxy_experiment::config::parse(&yaml).expect("the edited design parses")
    }

    fn row(unit: &str, state: &str, wave: i32, due: &str) -> experiment_flips::Model {
        experiment_flips::Model {
            id: Uuid::new_v4(),
            run_id: Uuid::nil(),
            unit: unit.into(),
            target_state: state.into(),
            due_at: due.parse::<oxy_experiment::Moment>().unwrap().at(),
            wave,
            task_id: None,
            agentic_run_id: None,
            lever_revision_id: None,
            status: "pending".into(),
            confirmed_on: None,
            reason: None,
            created_at: Utc::now().fixed_offset(),
        }
    }

    fn members() -> GroupMembership {
        GroupMembership(BTreeMap::from([
            (
                "east".to_string(),
                vec!["bondi".to_string(), "coogee".to_string()],
            ),
            ("west".to_string(), vec!["parramatta".to_string()]),
            ("north".to_string(), vec!["manly".to_string()]),
        ]))
    }

    #[test]
    fn per_unit_is_one_batch_per_row() {
        let rows = [
            row("bondi", "on", 0, "2026-03-02"),
            row("newtown", "on", 0, "2026-03-02"),
        ];
        let b = batches(&design(), &rows.iter().collect::<Vec<_>>());
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|b| b.ids.len() == 1 && b.units.len() == 1));
    }

    #[test]
    fn per_wave_is_one_batch_per_wave_state_and_due_day() {
        let rows = [
            row("newtown", "on", 0, "2026-03-02"),
            row("bondi", "on", 0, "2026-03-02"),
            row("manly", "on", 1, "2026-03-16"),
            row("bondi", "off", 0, "2026-03-20"),
        ];
        let b = batches(&edited(&[PER_WAVE]), &rows.iter().collect::<Vec<_>>());
        let shape: Vec<_> = b
            .iter()
            .map(|b| (b.wave, b.state, b.due_at.to_string(), b.units.join(",")))
            .collect();
        assert_eq!(
            shape,
            vec![
                (
                    0,
                    LeverState::On,
                    "2026-03-02".to_string(),
                    "bondi,newtown".to_string()
                ),
                (
                    1,
                    LeverState::On,
                    "2026-03-16".to_string(),
                    "manly".to_string()
                ),
                (
                    0,
                    LeverState::Off,
                    "2026-03-20".to_string(),
                    "bondi".to_string()
                ),
            ],
            "ordered by due day; units sorted; a terminal off never joins the wave's on"
        );
    }

    #[test]
    fn variables_follow_c2b_per_unit_and_per_wave() {
        let one = [row("bondi", "on", 0, "2026-03-02")];
        let unit = &batches(&design(), &one.iter().collect::<Vec<_>>())[0];
        assert_eq!(
            variables(&design(), "p", unit, None),
            Ok(Some(
                json!({ "unit": "bondi", "state": "on", "experiment": "p", "wave": 0 })
            ))
        );
        let two = [
            row("newtown", "on", 0, "2026-03-02"),
            row("bondi", "on", 0, "2026-03-02"),
        ];
        let per_wave = edited(&[PER_WAVE]);
        let wave = &batches(&per_wave, &two.iter().collect::<Vec<_>>())[0];
        assert_eq!(
            variables(&per_wave, "p", wave, None),
            Ok(Some(
                json!({ "units": ["bondi", "newtown"], "state": "on", "experiment": "p", "wave": 0 })
            ))
        );
    }

    #[test]
    fn under_assign_by_the_lever_gets_the_pinned_members_of_its_groups_only() {
        let rows = [
            row("east", "on", 0, "2026-03-02"),
            row("west", "on", 0, "2026-03-02"),
        ];
        let grouped = edited(&[PER_WAVE, GROUPS]);
        let wave = &batches(&grouped, &rows.iter().collect::<Vec<_>>())[0];
        let vars = variables(&grouped, "p", wave, Some(&members()))
            .unwrap()
            .unwrap();
        assert_eq!(
            vars["members"],
            json!({ "east": ["bondi", "coogee"], "west": ["parramatta"] })
        );

        let per_unit = edited(&[GROUPS]);
        let one = [row("west", "on", 0, "2026-03-02")];
        let unit = &batches(&per_unit, &one.iter().collect::<Vec<_>>())[0];
        let vars = variables(&per_unit, "p", unit, Some(&members()))
            .unwrap()
            .unwrap();
        assert_eq!(
            (vars["unit"].clone(), vars["members"].clone()),
            (json!("west"), json!(["parramatta"]))
        );
    }

    #[test]
    fn a_group_lever_waits_for_the_membership_and_fails_a_group_it_does_not_hold() {
        let rows = [
            row("east", "on", 0, "2026-03-02"),
            row("south", "on", 0, "2026-03-02"),
        ];
        let grouped = edited(&[PER_WAVE, GROUPS]);
        let wave = &batches(&grouped, &rows.iter().collect::<Vec<_>>())[0];
        assert_eq!(
            variables(&grouped, "p", wave, None),
            Ok(None),
            "not pinned yet: wait"
        );
        let err = variables(&grouped, "p", wave, Some(&members())).unwrap_err();
        assert!(
            err.contains("`south`") && err.contains("no pinned members"),
            "{err}"
        );
    }

    #[test]
    fn a_row_is_the_flip_of_the_period_it_starts_in_that_periods_state() {
        let c = switchback();
        let sb = c.design.switchback.as_ref().unwrap();
        let off =
            period_span(sb, Grain::Day, &row("bondi", "off", 1, "2026-03-09")).expect("period 1");
        assert_eq!(
            (off.from, off.until, off.state),
            (
                "2026-03-09".parse().unwrap(),
                "2026-03-16".parse().unwrap(),
                LeverState::Off
            )
        );
        assert_eq!(
            period_span(sb, Grain::Day, &row("bondi", "on", 1, "2026-03-09")),
            None,
            "period 1 is off, so an on that day is no period's"
        );
        assert_eq!(
            period_span(sb, Grain::Day, &row("bondi", "off", 1, "2026-03-04")),
            None,
            "a terminal off matches no period"
        );
    }
}
