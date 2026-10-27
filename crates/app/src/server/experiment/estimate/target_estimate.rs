//! Which estimator reads the target, and what it is told about units the data
//! lacked.
//!
//! One switch date or several, waves or switchback: [`estimate`] routes, and the
//! host never branches on the design. Two cases branch here. A ratio outcome
//! (`outcome.denominator`) is read by `estimate_ratio_of_sums`. A reading of past
//! data (`lenient`) drops a named unit the panel lacks instead of refusing
//! (`estimate_effect_dropping_missing`), and lists, beside the engine's own, the
//! units the panel builder left out for sparse data.

use oxy_airlayer_compat::engine::experiment::{
    DroppedUnit, EffectResult, ExperimentDesign, PanelMatrix, estimate,
    estimate_effect_dropping_missing, estimate_ratio_of_sums,
};
use oxy_experiment::engine_in::EffectIn;

use super::engines::Loaded;
use super::mirror::Judged;

/// What the panels said beyond the target itself.
#[derive(Clone, Copy)]
pub struct Read<'a> {
    pub lenient: bool,
    pub denominator: Option<&'a Loaded>,
    pub dropped: &'a [DroppedUnit],
}

impl Read<'_> {
    pub const STRICT: Read<'static> = Read {
        lenient: false,
        denominator: None,
        dropped: &[],
    };
}

pub fn estimate_target(
    m: &PanelMatrix,
    design: &ExperimentDesign,
    seed: u64,
    read: Read<'_>,
) -> Judged<EffectIn> {
    let mut r: EffectResult = match (read.denominator, design) {
        (Some(Loaded::Unavailable(why)), _) => {
            return Judged::refused(format!("the ratio's denominator: {why}"));
        }
        (Some(Loaded::Panel(d)), ExperimentDesign::Waves(a)) => {
            // Named units either panel lacks are listed below, not refused here.
            estimate_ratio_of_sums(m, d, &held_by_both(a, m, d))
        }
        (Some(Loaded::Panel(_)), _) => {
            return Judged::refused(
                "a ratio outcome needs waves; a switchback cannot read one".to_string(),
            );
        }
        (None, ExperimentDesign::Waves(a)) if read.lenient => {
            estimate_effect_dropping_missing(m, a, seed)
        }
        (None, _) => estimate(m, design, seed),
    };
    // A unit the panel builder dropped is still named by the design, so the engine
    // reports it again as having no rows: keep the builder's reason, once.
    r.dropped_units
        .retain(|d| !read.dropped.iter().any(|b| b.unit == d.unit));
    r.dropped_units.splice(0..0, read.dropped.iter().cloned());
    if let (Some(Loaded::Panel(d)), ExperimentDesign::Waves(a)) = (read.denominator, design) {
        list_missing_from_ratio(&mut r, m, d, a);
    }
    Judged::from_engine(&r)
}

/// `a` without the units either panel lacks (from their arm and stratum).
fn held_by_both(
    a: &oxy_airlayer_compat::engine::experiment::Assignment,
    num: &PanelMatrix,
    den: &PanelMatrix,
) -> oxy_airlayer_compat::engine::experiment::Assignment {
    let held = |u: &String| num.unit_index(u).is_some() && den.unit_index(u).is_some();
    let mut kept = a.clone();
    kept.switch_day.retain(|u, _| held(u));
    if let Some(strata) = kept.strata.as_mut() {
        for s in strata.iter_mut() {
            s.retain(|u| held(u));
        }
        strata.retain(|s| !s.is_empty());
    }
    kept
}

/// The ratio estimator reads only the units both panels hold and skips a named
/// unit outside them without saying so: name each one here, unless already listed.
fn list_missing_from_ratio(
    r: &mut EffectResult,
    num: &PanelMatrix,
    den: &PanelMatrix,
    a: &oxy_airlayer_compat::engine::experiment::Assignment,
) {
    let mut names: Vec<(&String, &Option<i64>)> = a.switch_day.iter().collect();
    names.sort();
    for (unit, switch) in names {
        let held = num.unit_index(unit).is_some() && den.unit_index(unit).is_some();
        if held || r.dropped_units.iter().any(|d| &d.unit == unit) {
            continue;
        }
        let arm = if switch.is_some() {
            "treated"
        } else {
            "control"
        };
        r.dropped_units.push(DroppedUnit {
            unit: unit.clone(),
            reason: format!("a {arm} unit with no rows in the panel"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_airlayer_compat::engine::experiment::Assignment;
    use std::collections::HashMap;

    /// 6 treated `t*`, 6 control `c*`, days 1..=40, switch 21; `t*` gain 5.
    fn panel(effect: f64) -> PanelMatrix {
        let mut rows = Vec::new();
        for u in 0..12 {
            let name = if u < 6 {
                format!("t{u}")
            } else {
                format!("c{u}")
            };
            for day in 1..=40_i64 {
                let wobble = ((u * 7 + day as usize * 13) % 11) as f64;
                let lift = if u < 6 && day >= 21 { effect } else { 0.0 };
                rows.push((name.clone(), day, 100.0 + wobble + lift));
            }
        }
        PanelMatrix::from_triples(rows)
    }

    fn design(m: &PanelMatrix, extra: Option<&str>) -> ExperimentDesign {
        let mut switch_day: HashMap<String, Option<i64>> = m
            .units()
            .iter()
            .map(|u| (u.clone(), u.starts_with('t').then_some(21)))
            .collect();
        if let Some(x) = extra {
            switch_day.insert(x.to_string(), Some(21));
        }
        ExperimentDesign::Waves(Assignment {
            switch_day,
            strata: None,
            pre_days: 20,
            post_days: 20,
            anticipation_days: 0,
            washout_days: 0,
            coverage_floor: 0.9,
            alpha: 0.05,
            family: 1,
        })
    }

    #[test]
    fn a_missing_treated_unit_refuses_strictly_and_is_listed_when_lenient() {
        let m = panel(5.0);
        let d = design(&m, Some("ghost"));
        assert!(
            estimate_target(&m, &d, 1, Read::STRICT)
                .view
                .refusal
                .is_some()
        );
        let lenient = Read {
            lenient: true,
            denominator: None,
            dropped: &[],
        };
        let j = estimate_target(&m, &d, 1, lenient);
        assert!(j.view.refusal.is_none(), "{:?}", j.view.refusal);
        assert_eq!(j.view.dropped_units.len(), 1);
        assert_eq!(j.view.dropped_units[0].unit, "ghost");
    }

    #[test]
    fn units_the_panel_builder_left_out_are_listed_first() {
        let m = panel(5.0);
        let built = [DroppedUnit {
            unit: "c99".into(),
            reason: "holds 3 of the panel's 40 days".into(),
        }];
        let read = Read {
            lenient: true,
            denominator: None,
            dropped: &built,
        };
        let j = estimate_target(&m, &design(&m, None), 1, read);
        assert_eq!(j.view.dropped_units[0].unit, "c99");
    }

    #[test]
    fn a_denominator_makes_the_target_a_ratio_of_sums() {
        let num = panel(0.0);
        let den = panel(0.0);
        let loaded = Loaded::Panel(den);
        let read = Read {
            lenient: true,
            denominator: Some(&loaded),
            dropped: &[],
        };
        let j = estimate_target(&num, &design(&num, None), 1, read);
        // Numerator == denominator: every unit's ratio is 1 in both windows, which
        // only the ratio estimator can notice.
        let why = j.view.refusal.expect("no spread in a constant ratio");
        assert!(why.contains("ratio moved identically"), "{why}");
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;
    use oxy_airlayer_compat::engine::experiment::Assignment;
    use std::collections::HashMap;

    #[test]
    fn a_treated_unit_the_builder_dropped_is_listed_once_with_the_builders_reason() {
        let rows: Vec<(String, i64, f64)> = (0..12)
            .flat_map(|u| {
                let name = if u < 6 {
                    format!("t{u}")
                } else {
                    format!("c{u}")
                };
                (1..=40_i64).map(move |d| {
                    let lift = if u < 6 && d >= 21 { 5.0 } else { 0.0 };
                    (
                        name.clone(),
                        d,
                        100.0 + ((u * 7 + d as usize * 13) % 11) as f64 + lift,
                    )
                })
            })
            .collect();
        let m = PanelMatrix::from_triples(rows);
        let mut switch_day: HashMap<String, Option<i64>> = m
            .units()
            .iter()
            .map(|u| (u.clone(), u.starts_with('t').then_some(21)))
            .collect();
        // t9 was in the wave but the builder dropped it for sparse data.
        switch_day.insert("t9".into(), Some(21));
        let design = ExperimentDesign::Waves(Assignment {
            switch_day,
            strata: None,
            pre_days: 20,
            post_days: 20,
            anticipation_days: 0,
            washout_days: 0,
            coverage_floor: 0.9,
            alpha: 0.05,
            family: 1,
        });
        let built = [DroppedUnit {
            unit: "t9".into(),
            reason: "holds 15 of the panel's 40 days".into(),
        }];
        let read = Read {
            lenient: true,
            denominator: None,
            dropped: &built,
        };
        let j = estimate_target(&m, &design, 1, read);
        assert_eq!(j.view.dropped_units.len(), 1, "{:?}", j.view.dropped_units);
        assert_eq!(
            j.view.dropped_units[0].reason,
            "holds 15 of the panel's 40 days"
        );
    }
}

#[cfg(test)]
mod ratio_missing_tests {
    use super::*;
    use oxy_airlayer_compat::engine::experiment::Assignment;
    use std::collections::HashMap;

    fn panel(scale: f64) -> PanelMatrix {
        let mut rows = Vec::new();
        for u in 0..12 {
            let name = if u < 6 {
                format!("t{u}")
            } else {
                format!("c{u}")
            };
            for d in 1..=40_i64 {
                let w = ((u * 7 + d as usize * 13) % 11) as f64;
                rows.push((name.clone(), d, (10.0 + w) * scale));
            }
        }
        PanelMatrix::from_triples(rows)
    }

    #[test]
    fn a_ratio_with_a_named_unit_the_panels_lack_lists_it_instead_of_refusing() {
        let num = panel(1.0);
        let den = Loaded::Panel(panel(4.0));
        let mut switch_day: HashMap<String, Option<i64>> = num
            .units()
            .iter()
            .map(|u| (u.clone(), u.starts_with('t').then_some(21)))
            .collect();
        switch_day.insert("c99".into(), None);
        let design = ExperimentDesign::Waves(Assignment {
            switch_day,
            strata: None,
            pre_days: 20,
            post_days: 20,
            anticipation_days: 0,
            washout_days: 0,
            coverage_floor: 0.9,
            alpha: 0.05,
            family: 1,
        });
        let read = Read {
            lenient: true,
            denominator: Some(&den),
            dropped: &[],
        };
        let j = estimate_target(&num, &design, 1, read);
        assert!(
            !j.view
                .refusal
                .as_deref()
                .unwrap_or("")
                .contains("not in the panel"),
            "{:?}",
            j.view.refusal
        );
        assert_eq!(j.view.dropped_units.len(), 1);
        assert_eq!(j.view.dropped_units[0].unit, "c99");
    }
}
