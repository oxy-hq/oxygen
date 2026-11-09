//! The dated waves and periods a proposal stores, and the YAML snippet the user
//! commits: every output key of pricing.

use oxy_airlayer_compat::engine::experiment::{Period, ProposedWave};
use oxy_experiment::{Grain, LeverState, Moment, PriorCoefficient, PriorSource};
use serde::{Deserialize, Serialize};

/// A wave as it is stored in `experiment_proposals.waves` and returned by
/// `GET …/proposals` — index C5's `{ switch_on, units }` (group names under `assign_by`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WavePlan {
    pub switch_on: Moment,
    pub units: Vec<String>,
}

/// A switchback period as stored in `experiment_proposals.periods` — index C5's `{ from, state }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodPlan {
    pub from: Moment,
    pub state: LeverState,
}

/// The first moment of period `ord` (its hour at hour grain): the host numbered
/// the periods with [`Grain::ordinal_of`], so it is the one that dates them back.
fn dated(grain: Grain, ord: i64) -> Result<Moment, String> {
    grain
        .start_at(ord)
        .map(Moment)
        .ok_or_else(|| format!("{} {ord} is outside the calendar", grain.as_str()))
}

/// The engine's waves (period ordinals of `grain`), as dated waves.
pub fn plan_waves(grain: Grain, waves: &[ProposedWave]) -> Result<Vec<WavePlan>, String> {
    waves
        .iter()
        .map(|w| {
            Ok(WavePlan {
                switch_on: dated(grain, w.switch_day)?,
                units: w.units.clone(),
            })
        })
        .collect()
}

/// The engine's switchback periods (period ordinals of `grain`), as dated periods.
pub fn plan_periods(grain: Grain, periods: &[Period]) -> Result<Vec<PeriodPlan>, String> {
    periods
        .iter()
        .map(|p| {
            Ok(PeriodPlan {
                from: dated(grain, p.from_day)?,
                state: if p.on {
                    LeverState::On
                } else {
                    LeverState::Off
                },
            })
        })
        .collect()
}

/// Every output key of pricing, as the user pastes it into the draft.
pub struct SnippetKeys<'a> {
    /// Top-level `assign_by`, for a group design.
    pub assign_by: Option<&'a str>,
    pub schedule: SnippetSchedule<'a>,
    /// `design.strata`, for a blocked draw.
    pub strata: Option<&'a [Vec<String>]>,
    pub seed: u64,
    /// `outcome.alpha`, written **only** when the candidate overrode the file's.
    pub alpha: Option<f64>,
    pub mde: f64,
    /// `outcome.prior_coefficient`, when pricing found a slope to freeze.
    pub prior: Option<&'a PriorCoefficient>,
}

pub enum SnippetSchedule<'a> {
    /// `design.waves`.
    Waves(&'a [WavePlan]),
    /// `design.switchback.periods` (with its `period_days`).
    Switchback {
        period_days: u32,
        periods: &'a [PeriodPlan],
    },
}

/// The keys the user merges into the draft and commits: `assign_by`,
/// `design.waves | design.switchback`, `design.strata`, `design.seed`,
/// `outcome.alpha` (if overridden), `outcome.mde_at_proposal` and
/// `outcome.prior_coefficient`.
///
/// Names go through `serde_yaml` so `12`, `yes` and `null` are quoted where they
/// must be, and lists are block sequences: in a flow sequence a name containing
/// `,` or `:` would need quoting logic this function would have to get right.
pub fn render_snippet(k: &SnippetKeys<'_>) -> Result<String, String> {
    let mut out = String::new();
    if let Some(by) = k.assign_by {
        out.push_str(&format!("assign_by: {}\n", yaml_scalar(by)?));
    }
    out.push_str("design:\n");
    match &k.schedule {
        SnippetSchedule::Waves(waves) => write_waves(&mut out, waves)?,
        SnippetSchedule::Switchback {
            period_days,
            periods,
        } => write_periods(&mut out, *period_days, periods),
    }
    if let Some(strata) = k.strata {
        write_strata(&mut out, strata)?;
    }
    out.push_str(&format!("  seed: {}\noutcome:\n", k.seed));
    if let Some(alpha) = k.alpha {
        out.push_str(&format!("  alpha: {alpha}\n"));
    }
    out.push_str(&format!("  mde_at_proposal: {}\n", k.mde));
    if let Some(p) = k.prior {
        let source = match p.source {
            PriorSource::Declared => "declared",
            PriorSource::Fitted => "fitted",
        };
        out.push_str(&format!(
            "  prior_coefficient: {{ value: {}, source: {source} }}\n",
            p.value
        ));
    }
    Ok(out)
}

fn write_waves(out: &mut String, waves: &[WavePlan]) -> Result<(), String> {
    out.push_str("  waves:\n");
    for wave in waves {
        out.push_str(&format!(
            "    - switch_on: {}\n      units:\n",
            wave.switch_on
        ));
        for unit in &wave.units {
            out.push_str(&format!("        - {}\n", yaml_scalar(unit)?));
        }
    }
    Ok(())
}

fn write_periods(out: &mut String, period_days: u32, periods: &[PeriodPlan]) {
    out.push_str(&format!(
        "  switchback:\n    period_days: {period_days}\n    periods:\n"
    ));
    for p in periods {
        let state = match p.state {
            LeverState::On => "on",
            LeverState::Off => "off",
        };
        out.push_str(&format!("      - {{ from: {}, state: {state} }}\n", p.from));
    }
}

fn write_strata(out: &mut String, strata: &[Vec<String>]) -> Result<(), String> {
    out.push_str("  strata:\n");
    for stratum in strata {
        for (i, unit) in stratum.iter().enumerate() {
            let lead = if i == 0 { "    - - " } else { "      - " };
            out.push_str(&format!("{lead}{}\n", yaml_scalar(unit)?));
        }
    }
    Ok(())
}

fn yaml_scalar(s: &str) -> Result<String, String> {
    let rendered = serde_yaml::to_string(s).map_err(|e| e.to_string())?;
    let rendered = rendered.trim_end();
    if rendered.contains('\n') {
        return Err(format!(
            "unit name {s:?} cannot be written as a single-line YAML scalar"
        ));
    }
    Ok(rendered.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use oxy_airlayer_compat::engine::experiment::{Period, ProposedWave, day_ordinal};
    use oxy_experiment::{LeverState, PriorCoefficient, PriorSource};

    use super::*;

    fn first() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()
    }

    fn plan(units: [&[&str]; 2]) -> Vec<WavePlan> {
        vec![
            WavePlan {
                switch_on: first().into(),
                units: units[0].iter().map(|s| s.to_string()).collect(),
            },
            WavePlan {
                switch_on: NaiveDate::from_ymd_opt(2026, 9, 21).unwrap().into(),
                units: units[1].iter().map(|s| s.to_string()).collect(),
            },
        ]
    }

    fn periods() -> Vec<PeriodPlan> {
        let from = |d| NaiveDate::from_ymd_opt(2026, 9, d).unwrap();
        vec![
            PeriodPlan {
                from: from(7).into(),
                state: LeverState::On,
            },
            PeriodPlan {
                from: from(14).into(),
                state: LeverState::Off,
            },
            PeriodPlan {
                from: from(21).into(),
                state: LeverState::Off,
            },
            PeriodPlan {
                from: from(28).into(),
                state: LeverState::On,
            },
        ]
    }

    fn keys<'a>(waves: &'a [WavePlan]) -> SnippetKeys<'a> {
        SnippetKeys {
            assign_by: None,
            schedule: SnippetSchedule::Waves(waves),
            strata: None,
            seed: 8841207,
            alpha: None,
            mde: 17.5,
            prior: None,
        }
    }

    fn parsed(snippet: &str) -> serde_yaml::Value {
        serde_yaml::from_str(snippet).expect("snippet is YAML")
    }

    #[test]
    fn plan_waves_steps_dates_from_the_first_switch() {
        let day = day_ordinal(first());
        let engine = vec![
            ProposedWave {
                switch_day: day,
                units: vec!["a".to_string()],
            },
            ProposedWave {
                switch_day: day + 14,
                units: vec!["b".to_string()],
            },
        ];
        let planned = plan_waves(Grain::Day, &engine).unwrap();
        assert_eq!(
            planned
                .iter()
                .map(|w| w.switch_on.to_string())
                .collect::<Vec<_>>(),
            ["2026-09-07", "2026-09-21"]
        );
        assert_eq!(planned[1].units, ["b"]);
    }

    #[test]
    fn plan_periods_steps_dates_and_maps_the_on_flag() {
        let day = day_ordinal(first());
        let engine = vec![
            Period {
                from_day: day,
                on: false,
            },
            Period {
                from_day: day + 7,
                on: true,
            },
        ];
        let planned = plan_periods(Grain::Day, &engine).unwrap();
        assert_eq!(
            planned[0],
            PeriodPlan {
                from: first().into(),
                state: LeverState::Off
            }
        );
        assert_eq!(planned[1].from.to_string(), "2026-09-14");
        assert_eq!(planned[1].state, LeverState::On);
    }

    #[test]
    fn wave_and_period_json_are_the_c5_shapes() {
        let v = serde_json::to_value(plan([&["a"], &["b"]])).unwrap();
        assert_eq!(v[0]["switch_on"], "2026-09-07");
        assert_eq!(v[0]["units"][0], "a");
        assert_eq!(
            serde_json::from_value::<Vec<WavePlan>>(v).unwrap(),
            plan([&["a"], &["b"]])
        );

        let p = serde_json::to_value(periods()).unwrap();
        assert_eq!(
            p[0],
            serde_json::json!({"from": "2026-09-07", "state": "on"})
        );
        assert_eq!(p[1]["state"], "off");
        assert_eq!(
            serde_json::from_value::<Vec<PeriodPlan>>(p).unwrap(),
            periods()
        );
    }

    #[test]
    fn the_waves_snippet_parses_back_to_the_proposed_waves_seed_and_mde() {
        let waves = plan([&["bondi", "newtown"], &["manly"]]);
        let v = parsed(&render_snippet(&keys(&waves)).unwrap());
        assert_eq!(v["design"]["seed"].as_u64(), Some(8841207));
        assert_eq!(v["outcome"]["mde_at_proposal"].as_f64(), Some(17.5));
        assert_eq!(
            v["design"]["waves"][0]["switch_on"].as_str(),
            Some("2026-09-07")
        );
        assert_eq!(
            v["design"]["waves"][1]["switch_on"].as_str(),
            Some("2026-09-21")
        );
        assert_eq!(
            v["design"]["waves"][0]["units"][1].as_str(),
            Some("newtown")
        );
    }

    #[test]
    fn a_unit_design_writes_no_assign_by_no_strata_no_alpha_and_no_prior() {
        let waves = plan([&["a"], &["b"]]);
        let v = parsed(&render_snippet(&keys(&waves)).unwrap());
        assert!(v.get("assign_by").is_none());
        assert!(v["design"].get("strata").is_none());
        assert!(
            v["outcome"].get("alpha").is_none(),
            "alpha is written only when a candidate overrode it"
        );
        assert!(v["outcome"].get("prior_coefficient").is_none());
    }

    #[test]
    fn every_output_key_is_written_when_the_proposal_has_it() {
        let waves = plan([&["east"], &["west"]]);
        let strata = vec![
            vec!["east".to_string(), "north".to_string()],
            vec!["west".to_string(), "south".to_string()],
        ];
        let prior = PriorCoefficient {
            value: 4.2,
            source: PriorSource::Fitted,
        };
        let k = SnippetKeys {
            assign_by: Some("sku.category"),
            strata: Some(&strata),
            alpha: Some(0.2),
            prior: Some(&prior),
            ..keys(&waves)
        };
        let v = parsed(&render_snippet(&k).unwrap());
        assert_eq!(v["assign_by"].as_str(), Some("sku.category"));
        assert_eq!(v["design"]["strata"][1][0].as_str(), Some("west"));
        assert_eq!(v["outcome"]["alpha"].as_f64(), Some(0.2));
        assert_eq!(
            v["outcome"]["prior_coefficient"]["value"].as_f64(),
            Some(4.2)
        );
        assert_eq!(
            v["outcome"]["prior_coefficient"]["source"].as_str(),
            Some("fitted")
        );
    }

    #[test]
    fn a_declared_prior_says_declared() {
        let waves = plan([&["a"], &["b"]]);
        let prior = PriorCoefficient {
            value: 2.3,
            source: PriorSource::Declared,
        };
        let k = SnippetKeys {
            prior: Some(&prior),
            ..keys(&waves)
        };
        let v = parsed(&render_snippet(&k).unwrap());
        assert_eq!(
            v["outcome"]["prior_coefficient"]["source"].as_str(),
            Some("declared")
        );
    }

    #[test]
    fn the_switchback_snippet_writes_the_period_and_every_period() {
        let p = periods();
        let k = SnippetKeys {
            schedule: SnippetSchedule::Switchback {
                period_days: 7,
                periods: &p,
            },
            alpha: Some(0.2),
            ..keys(&[])
        };
        let v = parsed(&render_snippet(&k).unwrap());
        assert_eq!(v["design"]["switchback"]["period_days"].as_u64(), Some(7));
        let written = v["design"]["switchback"]["periods"]
            .as_sequence()
            .expect("periods");
        assert_eq!(written.len(), 4);
        assert_eq!(written[0]["from"].as_str(), Some("2026-09-07"));
        assert_eq!(
            written[0]["state"].as_str(),
            Some("on"),
            "`on` must stay a string, not a YAML boolean"
        );
        assert_eq!(written[2]["state"].as_str(), Some("off"));
        assert!(v["design"].get("waves").is_none());
        assert_eq!(v["design"]["seed"].as_u64(), Some(8841207));
    }

    #[test]
    fn units_that_look_like_other_scalars_stay_strings() {
        let waves = plan([&["12", "yes", "null"], &["a, b", "x: y"]]);
        let v = parsed(&render_snippet(&keys(&waves)).unwrap());
        let units = |wave: usize| -> Vec<String> {
            v["design"]["waves"][wave]["units"]
                .as_sequence()
                .unwrap()
                .iter()
                .map(|u| {
                    u.as_str()
                        .unwrap_or_else(|| panic!("not a string: {u:?}"))
                        .to_string()
                })
                .collect()
        };
        assert_eq!(units(0), ["12", "yes", "null"]);
        assert_eq!(units(1), ["a, b", "x: y"]);
    }

    #[test]
    fn a_unit_name_with_a_newline_is_refused_rather_than_written_broken() {
        let waves = plan([&["bon\ndi"], &["manly"]]);
        let err = render_snippet(&keys(&waves)).unwrap_err();
        assert!(err.contains("single-line"), "{err}");
    }

    #[test]
    fn at_month_grain_a_wave_ordinal_is_dated_to_the_first_of_its_month() {
        let grain = Grain::Month;
        let ord = grain.ordinal(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        let engine = [ProposedWave {
            switch_day: ord + 2,
            units: vec!["a".to_string()],
        }];
        let planned = plan_waves(grain, &engine).unwrap();
        assert_eq!(
            planned[0].switch_on,
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
    }
}
