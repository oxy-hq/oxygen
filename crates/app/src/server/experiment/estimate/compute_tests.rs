use std::sync::Arc;

use chrono::NaiveDate;
use oxy_experiment::compliance::FlipOutcome;
use oxy_experiment::config::parse;
use oxy_experiment::engine_in::{EffectIn, PowerIn, RatioIn};
use oxy_experiment::report::{Seeds, WriteBackView};
use serde_json::json;

use oxy_experiment::config::{PriorCoefficient, PriorSource};
use oxy_experiment::patch::ReplacedPrior;
use oxy_experiment::report::{DesignKind, SwitchbackFacts};

use super::compute::*;
use super::mirror::Judged;
use super::testdata::{CONFIG_YAML, VIEW, switchback_config};

const SEEDS: Seeds = Seeds {
    effect: 8841207,
    placebo: 1,
    ratio: 2,
};

fn view(text: &str) -> Option<ViewRow> {
    Some(ViewRow {
        file_path: "views/store_days.view.yml".into(),
        definition: serde_yaml::from_str(text).unwrap(),
    })
}

fn inputs(aborted: bool, v: Option<ViewRow>, drop_driver: bool) -> RunInputs {
    let mut cfg = parse(CONFIG_YAML).unwrap();
    if drop_driver {
        cfg.outcome.driver = None;
    }
    let d = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    RunInputs {
        config: Arc::new(cfg),
        aborted,
        retrospective: false,
        abort_reason: aborted.then(|| "guardrail: refund_rate".to_string()),
        measured_until: None,
        levers_kept: false,
        carryover: None,
        control_pool: Some(vec!["s07".into(), "s08".into(), "s09".into()]),
        design_ref: "v3".into(),
        // What PR 4's `compliance_outcomes(.., final_record: true)` returns: every treated
        // unit as assigned, every pinned control as clean or leaked.
        outcomes: vec![
            ("s01".into(), FlipOutcome::Landed { on_day: d.into() }),
            ("s02".into(), FlipOutcome::Landed { on_day: d.into() }),
            ("s03".into(), FlipOutcome::NeverLanded),
            ("s07".into(), FlipOutcome::Clean),
            ("s08".into(), FlipOutcome::Clean),
            (
                "s09".into(),
                FlipOutcome::LeakedToControl { on_day: d.into() },
            ),
        ],
        proposal_count: 2,
        hand_assigned: false,
        mde_unavailable: None,
        lever_revisions: 1,
        view: v,
    }
}

fn effect(est: f64) -> Judged<EffectIn> {
    Judged {
        raw: json!({ "estimate": est }),
        view: EffectIn {
            estimate: Some(est),
            ci_low: Some(est - 3.0),
            ci_high: Some(est + 3.0),
            p_value: Some(0.001),
            n_treated: 3,
            n_control: 9,
            significant: true,
            ..Default::default()
        },
    }
}

fn engines(with_ratio: bool) -> Engines {
    let mut first_stage = effect(5.0).view;
    first_stage.significant = true;
    Engines {
        carryover: None,
        synthetic: None,
        effect: effect(25.0),
        sibling: Some(effect(-1.0)),
        sibling_power: Some(Judged {
            raw: json!({ "mde": 60.0 }),
            view: PowerIn {
                mde: Some(60.0),
                refusal: None,
            },
        }),
        ratio: with_ratio.then(|| Judged {
            raw: json!({ "coefficient": 5.0 }),
            view: RatioIn {
                coefficient: Some(5.0),
                ci_low: Some(4.0),
                ci_high: Some(6.0),
                first_stage,
                refusal: None,
            },
        }),
        n_untreated: 9,
        switchback: None,
        series: None,
    }
}

#[test]
fn a_qualifying_run_stores_an_edit_and_shows_it_in_the_report() {
    let c = assemble(&inputs(false, view(VIEW), false), &engines(true), SEEDS);
    let edit = c.edit.expect("qualifies");
    assert_eq!(
        (edit.coefficient, edit.refs_entry.as_str()),
        (5.0, "experiment:beverage_promo_pilot@v3")
    );
    assert_eq!(edit.file_path, "views/store_days.view.yml");
    assert!(matches!(
        c.report.write_back,
        Some(WriteBackView::Proposed { .. })
    ));
    assert_eq!(c.report.proposals.count, 2);
    assert_eq!(c.report.seeds, SEEDS);
    assert_eq!(c.report.substitution.floor, Some(60.0));
    assert_eq!(c.effect_raw, json!({ "estimate": 25.0 }));
    assert_eq!(c.ratio_raw, Some(json!({ "coefficient": 5.0 })));
}

#[test]
fn an_aborted_run_is_labelled_and_proposes_no_patch() {
    let c = assemble(&inputs(true, view(VIEW), false), &engines(true), SEEDS);
    assert!(c.edit.is_none());
    assert!(c.report.label.starts_with("ABORTED RUN"));
    assert!(matches!(
        c.report.write_back,
        Some(WriteBackView::Refused { .. })
    ));
}

#[test]
fn a_non_linear_edge_stores_no_patch_but_the_reason_reaches_the_report() {
    let text = VIEW.replace(
        "        direction: positive\n",
        "        form: log-log\n        coefficient: 1.2\n",
    );
    let c = assemble(&inputs(false, view(&text), false), &engines(true), SEEDS);
    assert!(c.edit.is_none());
    let Some(WriteBackView::Refused { reason }) = c.report.write_back else {
        panic!("refused")
    };
    assert!(reason.contains("log-log"), "{reason}");
}

#[test]
fn without_a_driver_there_is_no_ratio_and_no_write_back() {
    let c = assemble(&inputs(false, view(VIEW), true), &engines(false), SEEDS);
    assert!(c.edit.is_none() && c.report.ratio.is_none() && c.report.write_back.is_none());
    assert!(c.ratio_raw.is_none());
}

#[test]
fn compliance_is_summarised_from_the_flips_and_stored_as_json() {
    let c = assemble(&inputs(false, view(VIEW), false), &engines(true), SEEDS);
    assert!(
        c.compliance_json["c_treated"].as_f64().unwrap() < 1.0,
        "one of three never landed"
    );
    assert!(
        c.compliance_json["c_control_leak"].as_f64().unwrap() > 0.0,
        "one of three controls leaked"
    );
    for key in ["c_control_leak", "restated_mde", "below_floor"] {
        assert!(c.compliance_json.get(key).is_some(), "{key}");
    }
}

#[test]
fn an_unpinned_control_pool_is_a_refusal_in_every_section() {
    // No `experiment_check` ever pinned the pool (an abort before the first check): there is
    // no honest assignment, so every section says so — no fresh unit query fills the gap.
    let inp = inputs(false, view(VIEW), false);
    let why = "the run's control pool was never pinned, so no assignment can be built";
    let c = assemble(&inp, &refused_engines(&inp.config, why), SEEDS);
    assert_eq!(c.report.refusal.as_deref(), Some(why));
    assert!(c.report.headline.is_none());
    assert!(
        c.report.substitution.statement.contains("unavailable"),
        "{}",
        c.report.substitution.statement
    );
    assert_eq!(
        c.report.ratio.as_ref().unwrap().refusal.as_deref(),
        Some(why)
    );
    assert!(c.edit.is_none());
}

#[test]
fn a_lever_whose_revision_changed_reaches_the_report() {
    let mut inp = inputs(false, view(VIEW), false);
    inp.lever_revisions = 2;
    let c = assemble(&inp, &engines(true), SEEDS);
    assert!(
        c.report
            .lever_note
            .unwrap()
            .contains("the lever's definition changed during the experiment")
    );
    assert!(
        assemble(&inputs(false, view(VIEW), false), &engines(true), SEEDS)
            .report
            .lever_note
            .is_none()
    );
}

/// Per-member entries as PR 4's `compliance_outcomes` returns them under `assign_by`: `landed` of
/// `n` treated members landed in group `g`; `controls` clean control members.
fn member_outcomes(groups: &[(&str, usize, usize)], controls: usize) -> Vec<(String, FlipOutcome)> {
    let d = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    let mut out = Vec::new();
    for (g, landed, n) in groups {
        for i in 0..*n {
            let outcome = if i < *landed {
                FlipOutcome::Landed { on_day: d.into() }
            } else {
                FlipOutcome::NeverLanded
            };
            out.push((format!("{g}m{i}"), outcome));
        }
    }
    out.extend((0..controls).map(|i| (format!("c{i}"), FlipOutcome::Clean)));
    out
}

#[test]
fn under_assign_by_compliance_pools_members_it_does_not_average_group_shares() {
    // g1: 9 of 9 members landed; g2: 0 of 1. Pooled: 9/10 = 0.9 — above the 0.8 floor.
    // A mean of group shares would be 0.5 and would wrongly warn.
    let mut inp = inputs(false, view(VIEW), false);
    let mut cfg = (*inp.config).clone();
    cfg.assign_by = Some("store.region".into());
    inp.config = Arc::new(cfg);
    inp.outcomes = member_outcomes(&[("g1", 9, 9), ("g2", 0, 1)], 10);
    let c = assemble(&inp, &engines(false), SEEDS);
    assert!((c.compliance_json["c_treated"].as_f64().unwrap() - 0.9).abs() < 1e-9);
    assert_eq!(c.compliance_json["below_floor"], json!(false));
    assert!(
        c.report.compliance.warning.is_none(),
        "{:?}",
        c.report.compliance.warning
    );

    // 6 of 10 members landed: the pooled share is below the floor, so the attempt-not-the-lever warning
    // and the restated MDE both read 60%.
    inp.outcomes = member_outcomes(&[("g1", 3, 5), ("g2", 3, 5)], 10);
    let c = assemble(&inp, &engines(false), SEEDS);
    assert!((c.compliance_json["c_treated"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    let w = c.report.compliance.warning.expect("below the floor");
    assert!(
        w.contains("measures the attempt") && w.contains("60%"),
        "{w}"
    );
    assert!(
        c.report
            .compliance
            .restated
            .expect("restated MDE")
            .contains("60%")
    );
}

#[test]
fn a_switchback_pools_unit_period_entries_keyed_by_unit_and_period() {
    // Eight on-period entries (6 landed) and eight off-period entries (1 leaked): pooled 6/8 = 0.75, 1/8.
    let d = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    let mut inp = inputs(false, view(VIEW), true);
    inp.config = Arc::new(switchback_config());
    inp.outcomes = (0..8)
        .map(|i| {
            let o = if i < 6 {
                FlipOutcome::Landed { on_day: d.into() }
            } else {
                FlipOutcome::NeverLanded
            };
            (format!("s{i:02}@2026-03-02"), o)
        })
        .chain((0..8).map(|i| {
            let o = if i == 0 {
                FlipOutcome::LeakedToControl { on_day: d.into() }
            } else {
                FlipOutcome::Clean
            };
            (format!("s{i:02}@2026-03-09"), o)
        }))
        .collect();
    let c = assemble(&inp, &engines(false), SEEDS);
    assert!((c.compliance_json["c_treated"].as_f64().unwrap() - 0.75).abs() < 1e-9);
    assert!((c.compliance_json["c_control_leak"].as_f64().unwrap() - 0.125).abs() < 1e-9);
    assert!(
        c.report
            .compliance
            .warning
            .expect("0.75 < the 0.8 floor")
            .contains("75%")
    );
}

fn with_prior(mut inp: RunInputs, value: f64) -> RunInputs {
    let mut cfg = (*inp.config).clone();
    cfg.outcome.prior_coefficient = Some(PriorCoefficient {
        value,
        source: PriorSource::Fitted,
    });
    inp.config = Arc::new(cfg);
    inp
}

#[test]
fn a_frozen_prior_is_compared_and_its_prediction_is_kept_for_its_own_column() {
    let c = assemble(
        &with_prior(inputs(false, view(VIEW), false), 4.2),
        &engines(true),
        SEEDS,
    );
    let p = c.report.prediction.as_ref().expect("compared");
    assert_eq!(
        p.predicted,
        4.2 * 5.0,
        "prior × the first stage the experiment measured (5.0)"
    );
    assert_eq!(p.measured, 25.0);
    assert_eq!(c.prediction_json, Some(serde_json::to_value(p).unwrap()));
}

#[test]
fn no_frozen_prior_keeps_no_prediction() {
    let c = assemble(&inputs(false, view(VIEW), false), &engines(true), SEEDS);
    assert!(c.report.prediction.is_none() && c.prediction_json.is_none());
}

#[test]
fn the_edit_names_the_prior_it_would_replace() {
    let c = assemble(
        &with_prior(inputs(false, view(VIEW), false), 4.2),
        &engines(true),
        SEEDS,
    );
    let edit = c.edit.expect("qualifies");
    assert_eq!(
        edit.replaces,
        Some(ReplacedPrior {
            value: 4.2,
            source: PriorSource::Fitted,
            edge_coefficient: None
        })
    );
    assert!(matches!(
        c.report.write_back,
        Some(WriteBackView::Proposed {
            replaces: Some(_),
            ..
        })
    ));
    // and nothing is claimed when the run froze no prior
    let bare = assemble(&inputs(false, view(VIEW), false), &engines(true), SEEDS);
    assert_eq!(bare.edit.expect("qualifies").replaces, None);
}

#[test]
fn a_switchback_reports_its_assumptions_totals_over_on_days_and_no_projection() {
    let mut inp = inputs(false, view(VIEW), true);
    inp.config = Arc::new(switchback_config()); // alpha 0.2 → directional; pre/post days are None
    let mut e = engines(false);
    e.n_untreated = 9; // must be ignored: a switchback treats the whole fleet
    e.switchback = Some(SwitchbackFacts {
        fleet_units: 12,
        on_days: 24,
    });
    let c = assemble(&inp, &e, SEEDS);
    assert_eq!(c.report.design, DesignKind::Switchback);
    assert!(c.report.directional);
    assert_eq!(c.report.assumptions.len(), 2);
    assert!(c.report.projection.is_none());
    let t = c.report.treated_total.expect("a total over the on days");
    assert_eq!((t.units, t.days, t.total), (12, 24, 25.0 * 12.0 * 24.0));
}

#[test]
fn an_aborted_switchback_with_no_flips_reports_compliance_not_measured() {
    // Aborted before the first period flipped anything: `compliance_outcomes(.., true)` has no
    // on-period entries, so `summarize` reports `n_treated == 0`. That is "not measured" —
    // never 0%, no "measures the attempt" warning, no restated MDE (C5).
    let mut inp = inputs(true, view(VIEW), false);
    inp.config = Arc::new(switchback_config());
    inp.outcomes = Vec::new();
    let c = assemble(&inp, &engines(false), SEEDS);
    assert_eq!(c.compliance_json["n_treated"], json!(0));
    assert_eq!(c.compliance_json["n_control"], json!(0));
    let v = &c.report.compliance;
    assert!(
        v.not_measured
            .as_deref()
            .is_some_and(|m| m.contains("not measured")),
        "{:?}",
        v.not_measured
    );
    assert!(v.warning.is_none() && v.restated.is_none() && v.restated_mde.is_none());
    assert!(!v.below_floor);
}

#[test]
fn the_series_is_stored_with_the_abort_cut_and_absent_when_never_read() {
    use super::series::{DailySeries, SeriesShape};
    let mut e = engines(false);
    assert!(
        assemble(&inputs(false, None, true), &e, SEEDS)
            .series_json
            .is_none()
    );

    e.series = Some(DailySeries {
        measure: "store_days.beverages_revenue".into(),
        grain: oxy_experiment::Grain::Day,
        shape: SeriesShape::Waves {
            treated_units: 3,
            control_units: 9,
            waves: vec![],
            days: vec![],
        },
        measured_until: None,
    });
    let mut inp = inputs(true, None, true);
    inp.measured_until = NaiveDate::from_ymd_opt(2026, 4, 1);
    let v = assemble(&inp, &e, SEEDS).series_json.expect("stored");
    assert_eq!(v["design"], "waves");
    assert_eq!(v["measured_until"], "2026-04-01");
}

#[test]
fn a_ratio_outcome_stores_no_series_of_its_numerator() {
    use super::series::{DailySeries, SeriesShape};
    let mut e = engines(false);
    e.series = Some(DailySeries {
        measure: "store_days.beverages_revenue".into(),
        grain: oxy_experiment::Grain::Day,
        shape: SeriesShape::Waves {
            treated_units: 3,
            control_units: 9,
            waves: vec![],
            days: vec![],
        },
        measured_until: None,
    });
    let mut inp = inputs(false, None, true);
    let mut cfg = (*inp.config).clone();
    cfg.outcome.denominator = Some("store_days.covers".into());
    inp.config = std::sync::Arc::new(cfg);
    assert!(
        assemble(&inp, &e, SEEDS).series_json.is_none(),
        "a day-by-day of the numerator would read as the outcome"
    );
}
