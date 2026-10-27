//! The compliance record (C4): one `(key, FlipOutcome)` per compliance
//! subject, no thresholds — `summarize` pools exactly the share the spec
//! names. A unit (waves over units); under `assign_by` each *member* (treated
//! members Landed / NeverLanded / Failed, control members Clean /
//! LeakedToControl); for a switchback each unit in each period, keyed by Task
//! 2's `period_key`. The status route reads the record so far; PR 5 reads the
//! final record, in which every subject appears (ITT). Leaks are observations,
//! never flip rows.

use entity::experiment_flips;
use oxy_experiment::ExperimentConfig;
use oxy_experiment::Moment;
use oxy_experiment::compliance::{FlipOutcome, period_key};
use oxy_experiment::config::{PeriodConfig, SwitchbackConfig};
use oxy_experiment::run::LeverState;

use super::panel::GroupMembership;
use super::send::PERIOD_ENDED;
use super::store::Observations;

/// C4's signature. `fleet` is `experiment_runs.fleet`: a switchback's units come
/// from it and only from it (flip rows never name them), and only periods that
/// began before the run ended count. Waves designs ignore it.
pub fn compliance_outcomes(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
    pool: Option<&[String]>,
    membership: Option<&GroupMembership>,
    fleet: Option<&[String]>,
    obs: &Observations,
    final_record: bool,
) -> Vec<(String, FlipOutcome)> {
    if let Some(sb) = &c.design.switchback {
        return period_entries(sb, flips, fleet, obs, final_record);
    }
    let mut out = match membership {
        Some(groups) => treated_members(c, flips, groups, obs, final_record),
        None => treated_units(c, flips, final_record),
    };
    // Controls once checked (always, in the final record). Never `NeverLanded` (C2).
    if let Some(pool) = pool.filter(|_| final_record || obs.controls_checked_on.is_some()) {
        out.extend(control_entries(pool, membership, obs));
    }
    out
}

/// The waves that began: some unit (or group) of the wave has its `on` recorded and not
/// abandoned by the abort — the switchback's `began` rule ([`period_entries`]). `step`
/// records a wave's `on`s on its switch day while the run is live and none once it is
/// aborted, so a wave whose switch day is on or after the abort never switched: its units are
/// in neither arm, in the final record too. With no wave begun there is no treated entry, and
/// compliance is not measured (`summarize`: `n_treated == 0`).
fn begun_waves<'a>(
    c: &'a ExperimentConfig,
    flips: &'a [experiment_flips::Model],
) -> impl Iterator<Item = &'a String> + 'a {
    c.design
        .waves
        .iter()
        .filter(|w| {
            flips.iter().any(|f| {
                f.target_state == "on"
                    && w.units.contains(&f.unit)
                    && (f.status != "cancelled" || f.reason.as_deref() == Some(PERIOD_ENDED))
            })
        })
        .flat_map(|w| w.units.iter())
}

fn treated_units(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
    final_record: bool,
) -> Vec<(String, FlipOutcome)> {
    begun_waves(c, flips)
        .filter_map(|unit| {
            settle(
                unit,
                on_flip(flips, unit).and_then(row_outcome),
                final_record,
            )
        })
        .collect()
}

/// One entry per pinned *member* of every treated group — unless the group's
/// flip failed, which is the lever never running for any of them. A member not
/// yet read back is left out of the record so far and is `NeverLanded` in the
/// final one.
fn treated_members(
    c: &ExperimentConfig,
    flips: &[experiment_flips::Model],
    groups: &GroupMembership,
    obs: &Observations,
    final_record: bool,
) -> Vec<(String, FlipOutcome)> {
    begun_waves(c, flips)
        .flat_map(|group| {
            let failed = on_flip(flips, group)
                .filter(|f| f.status == "failed")
                .and_then(row_outcome);
            members(groups, group).iter().filter_map(move |m| {
                let seen = obs.member_outcomes.get(m).cloned();
                settle(m, failed.clone().or(seen), final_record)
            })
        })
        .collect()
}

/// One entry per control unit — or, under `assign_by`, per member of each
/// control group — `Clean` unless read-back saw the lever there.
fn control_entries(
    pool: &[String],
    membership: Option<&GroupMembership>,
    obs: &Observations,
) -> Vec<(String, FlipOutcome)> {
    let leak = |u: &String| {
        let o =
            obs.control_leaks
                .get(u)
                .map_or(FlipOutcome::Clean, |d| FlipOutcome::LeakedToControl {
                    on_day: Moment::midnight(*d),
                });
        (u.clone(), o)
    };
    pool.iter()
        .flat_map(|unit| match membership.and_then(|g| g.0.get(unit)) {
            None => std::slice::from_ref(unit)
                .iter()
                .map(&leak)
                .collect::<Vec<_>>(),
            Some(ms) => ms.iter().map(&leak).collect(),
        })
        .collect()
}

/// A switchback: one entry per fleet unit per period that began, keyed
/// `period_key`. On periods are treated entries (Landed / NeverLanded /
/// Failed), off periods control entries (Clean / LeakedToControl) — read-back's
/// sticky `observations.period_outcomes`, cancelled periods included (decided
/// from what read-back saw, never from a send). The units are the pinned
/// `fleet` (C3), never the units flip rows happen to name; no fleet pinned, no
/// entries. A period **began** when the run recorded its boundary flip — the
/// `(state, due_at)` row `step` checks before emitting one. `step` emits every
/// elapsed boundary for the whole fleet while the run is live and none once it
/// is aborted or closed, so a period whose `from` is not before the run ended
/// has no row for anyone and yields no entries, in the final record too.
fn period_entries(
    sb: &SwitchbackConfig,
    flips: &[experiment_flips::Model],
    fleet: Option<&[String]>,
    obs: &Observations,
    final_record: bool,
) -> Vec<(String, FlipOutcome)> {
    let Some(fleet) = fleet else {
        return Vec::new();
    };
    // A row cancelled by an abort is a period the run abandoned, not one that
    // began (read-back applies the same rule); one cancelled because its period
    // had ended did begin.
    let began = |p: &PeriodConfig| {
        flips.iter().any(|f| {
            Moment(f.due_at) == p.from
                && LeverState::parse(&f.target_state) == Some(p.state)
                && (f.status != "cancelled" || f.reason.as_deref() == Some(PERIOD_ENDED))
        })
    };
    let mut units: Vec<&str> = fleet.iter().map(String::as_str).collect();
    units.sort_unstable();
    units.dedup();
    units
        .into_iter()
        .flat_map(|unit| {
            sb.periods
                .iter()
                .filter(|p| began(*p))
                .filter_map(move |p| period_entry(unit, p, flips, obs, final_record))
        })
        .collect()
}

/// A began period of this unit, read back; in the final record every began
/// period, an undecided on as `NeverLanded` and an undecided off as `Clean`.
fn period_entry(
    unit: &str,
    p: &PeriodConfig,
    flips: &[experiment_flips::Model],
    obs: &Observations,
    final_record: bool,
) -> Option<(String, FlipOutcome)> {
    let key = period_key(unit, p.from);
    let failed = flips
        .iter()
        .find(|f| {
            f.unit == unit
                && Moment(f.due_at) == p.from
                && LeverState::parse(&f.target_state) == Some(p.state)
        })
        .filter(|f| f.status == "failed")
        .and_then(row_outcome);
    match obs.period_outcomes.get(&key).cloned().or(failed) {
        Some(o) => Some((key, o)),
        None if final_record && p.state == LeverState::On => Some((key, FlipOutcome::NeverLanded)),
        None if final_record => Some((key, FlipOutcome::Clean)),
        None => None,
    }
}

fn row_outcome(f: &experiment_flips::Model) -> Option<FlipOutcome> {
    match f.status.as_str() {
        "confirmed" => f
            .confirmed_on
            .map(|at| FlipOutcome::Landed { on_day: Moment(at) }),
        "unconfirmed" => Some(FlipOutcome::NeverLanded),
        "failed" => Some(FlipOutcome::Failed {
            reason: f.reason.clone().unwrap_or_default(),
        }),
        _ => None,
    }
}

fn on_flip<'a>(
    flips: &'a [experiment_flips::Model],
    unit: &str,
) -> Option<&'a experiment_flips::Model> {
    flips
        .iter()
        .find(|f| f.unit == unit && f.target_state == "on")
}

fn members<'a>(groups: &'a GroupMembership, group: &str) -> &'a [String] {
    groups.0.get(group).map_or(&[], Vec::as_slice)
}

/// The record so far leaves an undecided unit out; the final record counts it (ITT).
fn settle(
    unit: &str,
    outcome: Option<FlipOutcome>,
    final_record: bool,
) -> Option<(String, FlipOutcome)> {
    match outcome {
        Some(o) => Some((unit.to_string(), o)),
        None if final_record => Some((unit.to_string(), FlipOutcome::NeverLanded)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use chrono::{NaiveDate, Utc};
    use uuid::Uuid;

    use super::*;
    use crate::server::experiment::test_design::{YAML, design, switchback};

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn flip(
        unit: &str,
        state: &str,
        due: &str,
        status: &str,
        on: Option<&str>,
    ) -> experiment_flips::Model {
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
            confirmed_on: on.map(d).map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
            reason: None,
            created_at: Utc::now().fixed_offset(),
        }
    }

    fn landed(day: &str) -> FlipOutcome {
        FlipOutcome::Landed {
            on_day: d(day).into(),
        }
    }

    #[test]
    fn the_final_record_counts_every_unit_as_assigned() {
        let c = design();
        let flips = vec![
            flip("bondi", "on", "2026-03-02", "confirmed", Some("2026-03-02")),
            flip("newtown", "on", "2026-03-02", "dispatched", None),
            // Wave 2 began: surry_hills' `on` was recorded; manly's never was.
            flip("surry_hills", "on", "2026-03-16", "dispatched", None),
        ];
        let pool = vec!["glebe".to_string(), "paddington".to_string()];
        let obs = Observations {
            control_leaks: BTreeMap::from([("glebe".to_string(), d("2026-03-04"))]),
            ..Default::default()
        };
        let so_far = compliance_outcomes(&c, &flips, Some(&pool), None, None, &obs, false);
        assert_eq!(
            so_far.len(),
            1,
            "only bondi is decided; controls are not checked yet"
        );
        let record: HashMap<_, _> =
            compliance_outcomes(&c, &flips, Some(&pool), None, None, &obs, true)
                .into_iter()
                .collect();
        assert_eq!(record.len(), 6, "four treated and two controls");
        assert_eq!(record["newtown"], FlipOutcome::NeverLanded);
        assert_eq!(
            record["manly"],
            FlipOutcome::NeverLanded,
            "a treated unit never flipped is still treated"
        );
        assert_eq!(
            record["glebe"],
            FlipOutcome::LeakedToControl {
                on_day: d("2026-03-04").into()
            }
        );
        assert_eq!(
            record["paddington"],
            FlipOutcome::Clean,
            "a control is never NeverLanded"
        );
    }

    #[test]
    fn an_aborted_waves_run_counts_only_the_waves_that_began() {
        // Aborted between the waves: wave 2 (16 March) never switched, so its units are
        // in neither arm — never `NeverLanded`.
        let c = design();
        let flips = vec![
            flip("bondi", "on", "2026-03-02", "confirmed", Some("2026-03-02")),
            flip(
                "newtown",
                "on",
                "2026-03-02",
                "confirmed",
                Some("2026-03-03"),
            ),
            flip("bondi", "off", "2026-03-10", "dispatched", None),
            flip("newtown", "off", "2026-03-10", "dispatched", None),
        ];
        let record: HashMap<_, _> =
            compliance_outcomes(&c, &flips, None, None, None, &Observations::default(), true)
                .into_iter()
                .collect();
        assert_eq!(record.len(), 2, "{record:?}");
        assert!(!record.contains_key("manly") && !record.contains_key("surry_hills"));
        let s = oxy_experiment::compliance::summarize(&c, &record.into_iter().collect::<Vec<_>>());
        assert_eq!((s.n_treated, s.c_treated), (2, 1.0));

        // Aborted before the first switch: the wave's `on`s were fenced and cancelled by the
        // abort, so no wave began — compliance is not measured, never "0%".
        let fenced: Vec<_> = ["bondi", "newtown"]
            .into_iter()
            .map(|u| {
                let mut f = flip(u, "on", "2026-03-02", "cancelled", None);
                f.reason = Some(crate::server::experiment::send::RUN_ENDED.into());
                f
            })
            .collect();
        for flips in [Vec::new(), fenced] {
            let entries =
                compliance_outcomes(&c, &flips, None, None, None, &Observations::default(), true);
            assert!(entries.is_empty(), "{entries:?}");
        }
    }

    #[test]
    fn under_assign_by_one_entry_per_member() {
        let yaml = YAML
            .replace(
                "for_each: store\n",
                "for_each: store\nassign_by: store.region\n",
            )
            .replace("[bondi, newtown]", "[east]")
            .replace("[manly, surry_hills]", "[west]");
        let c = oxy_experiment::config::parse(&yaml).expect("the grouped design parses");
        let groups = GroupMembership(BTreeMap::from([
            (
                "east".to_string(),
                vec!["bondi".to_string(), "coogee".to_string()],
            ),
            ("west".to_string(), vec!["parramatta".to_string()]),
            (
                "north".to_string(),
                vec!["manly".to_string(), "glebe".to_string()],
            ),
        ]));
        let flips = vec![
            flip("east", "on", "2026-03-02", "confirmed", Some("2026-03-02")),
            flip("west", "on", "2026-03-16", "dispatched", None),
        ];
        let obs = Observations {
            member_outcomes: BTreeMap::from([
                ("bondi".to_string(), landed("2026-03-02")),
                ("coogee".to_string(), landed("2026-03-03")),
            ]),
            control_leaks: BTreeMap::from([("glebe".to_string(), d("2026-03-05"))]),
            controls_checked_on: Some(d("2026-03-06")),
            ..Default::default()
        };
        let pool = vec!["north".to_string()];
        let so_far = compliance_outcomes(&c, &flips, Some(&pool), Some(&groups), None, &obs, false);
        assert_eq!(
            so_far.len(),
            4,
            "bondi, coogee and both north members; parramatta is undecided"
        );
        let entries = compliance_outcomes(&c, &flips, Some(&pool), Some(&groups), None, &obs, true);
        let record: HashMap<_, _> = entries.iter().cloned().collect();
        assert_eq!(record.len(), 5, "one per member, never per group");
        assert_eq!(record["bondi"], landed("2026-03-02"));
        assert_eq!(record["coogee"], landed("2026-03-03"));
        assert_eq!(
            record["parramatta"],
            FlipOutcome::NeverLanded,
            "never decided: ITT"
        );
        assert_eq!(record["manly"], FlipOutcome::Clean);
        assert_eq!(
            record["glebe"],
            FlipOutcome::LeakedToControl {
                on_day: d("2026-03-05").into()
            }
        );
        let s = oxy_experiment::compliance::summarize(&c, &entries);
        assert_eq!(
            (s.c_treated, s.c_control_leak),
            (2.0 / 3.0, 0.5),
            "pooled over members, no threshold"
        );
    }

    #[test]
    fn a_failed_group_flip_fails_every_member() {
        let yaml = YAML
            .replace(
                "for_each: store\n",
                "for_each: store\nassign_by: store.region\n",
            )
            .replace("[bondi, newtown]", "[east]")
            .replace("[manly, surry_hills]", "[west]");
        let c = oxy_experiment::config::parse(&yaml).expect("the grouped design parses");
        let groups = GroupMembership(BTreeMap::from([
            (
                "east".to_string(),
                vec!["bondi".to_string(), "coogee".to_string()],
            ),
            ("west".to_string(), vec!["parramatta".to_string()]),
        ]));
        let flips = vec![flip("east", "on", "2026-03-02", "failed", None)];
        let record: HashMap<_, _> = compliance_outcomes(
            &c,
            &flips,
            None,
            Some(&groups),
            None,
            &Observations::default(),
            false,
        )
        .into_iter()
        .collect();
        assert_eq!(
            record.len(),
            2,
            "east's two members; west has no flip and no read-back yet"
        );
        assert!(matches!(record["bondi"], FlipOutcome::Failed { .. }));
        assert!(matches!(record["coogee"], FlipOutcome::Failed { .. }));
    }

    #[test]
    fn a_switchback_emits_one_entry_per_unit_per_period() {
        let c = switchback();
        let flips = vec![
            flip("bondi", "on", "2026-03-02", "confirmed", Some("2026-03-03")),
            flip("bondi", "off", "2026-03-09", "confirmed", None),
            flip("glebe", "on", "2026-03-02", "confirmed", Some("2026-03-02")),
            flip(
                "glebe",
                "off",
                "2026-03-09",
                "unconfirmed",
                Some("2026-03-12"),
            ),
            // The last two periods began (their boundary rows exist) but were never read back.
            flip("bondi", "off", "2026-03-16", "dispatched", None),
            flip("glebe", "off", "2026-03-16", "dispatched", None),
            flip("bondi", "on", "2026-03-23", "dispatched", None),
            flip("glebe", "on", "2026-03-23", "dispatched", None),
        ];
        let fleet = vec!["bondi".to_string(), "glebe".to_string()];
        let key = |unit: &str, from: &str| period_key(unit, d(from).into());
        let obs = Observations {
            period_outcomes: BTreeMap::from([
                (key("bondi", "2026-03-02"), landed("2026-03-03")),
                (key("bondi", "2026-03-09"), FlipOutcome::Clean),
                (key("glebe", "2026-03-02"), landed("2026-03-02")),
                (
                    key("glebe", "2026-03-09"),
                    FlipOutcome::LeakedToControl {
                        on_day: d("2026-03-12").into(),
                    },
                ),
            ]),
            ..Default::default()
        };
        let so_far = compliance_outcomes(&c, &flips, None, None, Some(&fleet), &obs, false);
        assert_eq!(so_far.len(), 4, "two units over the two periods read back");
        let entries = compliance_outcomes(&c, &flips, None, None, Some(&fleet), &obs, true);
        let record: HashMap<_, _> = entries.iter().cloned().collect();
        assert_eq!(
            record.len(),
            8,
            "two units over four periods, never one per unit"
        );
        assert_eq!(
            record[&key("glebe", "2026-03-09")],
            FlipOutcome::LeakedToControl {
                on_day: d("2026-03-12").into()
            }
        );
        assert_eq!(
            record[&key("bondi", "2026-03-16")],
            FlipOutcome::Clean,
            "an unrecorded off counts clean (ITT)"
        );
        assert_eq!(
            record[&key("bondi", "2026-03-23")],
            FlipOutcome::NeverLanded,
            "an unrecorded on never landed"
        );
        let s = oxy_experiment::compliance::summarize(&c, &entries);
        assert_eq!(
            (s.c_treated, s.c_control_leak),
            (0.5, 0.25),
            "pooled over unit-periods, no threshold"
        );
    }

    #[test]
    fn a_switchback_aborted_before_its_first_boundary_has_no_entries() {
        // Registered, fleet pinned, aborted before 2 March: `step` recorded no boundary row,
        // so no period began. The final record is empty, not "NeverLanded" for periods that
        // never existed for the run — compliance is not measured (summarize: n_treated == 0).
        let c = switchback();
        let fleet = vec!["bondi".to_string(), "glebe".to_string()];
        let entries = compliance_outcomes(
            &c,
            &[],
            None,
            None,
            Some(&fleet),
            &Observations::default(),
            true,
        );
        assert!(entries.is_empty(), "{entries:?}");
        let s = oxy_experiment::compliance::summarize(&c, &entries);
        assert_eq!((s.n_treated, s.n_control), (0, 0));

        // Aborted during the second period: the periods that began count, the two after do not.
        let flips = vec![
            flip("bondi", "on", "2026-03-02", "confirmed", Some("2026-03-03")),
            flip("glebe", "on", "2026-03-02", "confirmed", Some("2026-03-03")),
            flip("bondi", "off", "2026-03-09", "dispatched", None),
            flip("glebe", "off", "2026-03-09", "dispatched", None),
        ];
        let entries = compliance_outcomes(
            &c,
            &flips,
            None,
            None,
            Some(&fleet),
            &Observations::default(),
            true,
        );
        assert_eq!(
            entries.len(),
            4,
            "two units over the two periods that began, never over all four"
        );

        // No fleet pinned (a run aborted before the first check): nothing to count.
        assert!(
            compliance_outcomes(&c, &flips, None, None, None, &Observations::default(), true)
                .is_empty()
        );
    }

    #[test]
    fn an_abort_cancelled_boundary_is_not_a_period_but_a_period_ended_one_is() {
        let c = switchback();
        let fleet = vec!["bondi".to_string(), "glebe".to_string()];
        let cancelled = |unit: &str, due: &str, reason: &str| {
            let mut f = flip(unit, "off", due, "cancelled", None);
            f.reason = Some(reason.into());
            f
        };
        let on = |unit: &str| flip(unit, "on", "2026-03-02", "confirmed", Some("2026-03-02"));
        let aborted: Vec<_> = ["bondi", "glebe"]
            .into_iter()
            .flat_map(|u| {
                [
                    on(u),
                    cancelled(u, "2026-03-09", crate::server::experiment::send::RUN_ENDED),
                ]
            })
            .collect();
        let entries = compliance_outcomes(
            &c,
            &aborted,
            None,
            None,
            Some(&fleet),
            &Observations::default(),
            true,
        );
        assert_eq!(entries.len(), 2, "only the first period began: {entries:?}");

        let ended: Vec<_> = ["bondi", "glebe"]
            .into_iter()
            .flat_map(|u| [on(u), cancelled(u, "2026-03-09", PERIOD_ENDED)])
            .collect();
        let entries = compliance_outcomes(
            &c,
            &ended,
            None,
            None,
            Some(&fleet),
            &Observations::default(),
            true,
        );
        assert_eq!(
            entries.len(),
            4,
            "a period that ended before dispatch still began: {entries:?}"
        );
    }
}
