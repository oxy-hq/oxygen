//! `fetch_readings` against a layer and a scripted warehouse.

use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeZone, Utc};
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_experiment::parse;
use oxy_shared::errors::OxyError;
use serde_json::{Map, Value, json};

use super::candidate::{Candidate, SwitchbackCandidate, WavesCandidate};
use super::lag::LagReading;
use super::panel::PanelOutcome;
use super::prior::PriorReading;
use super::reads::*;
use super::spread::UnitMeans;
use super::testing::{ANOMALY_AND_TRIGGER, with_guardrails, yaml};

const STORES: &str = r#"
name: stores
datasource: warehouse
table: stores
entities:
  - name: store
    type: primary
    key: store_id
dimensions:
  - name: store_id
    type: string
    expr: store_id
  - name: region
    type: string
    expr: region
measures:
  - name: store_count
    type: count
"#;

const STORE_DAYS: &str = r#"
name: store_days
datasource: warehouse
table: store_days
entities:
  - name: store_day
    type: primary
    key: store_day_id
  - name: store
    type: foreign
    key: store_id
dimensions:
  - name: business_date
    type: date
    expr: business_date
  - name: store_day_id
    type: string
    expr: store_day_id
  - name: store_id
    type: string
    expr: store_id
measures:
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
  - name: promo_active
    type: sum
    expr: promo_active
  - name: refund_rate
    type: sum
    expr: refund_rate
  - name: labor_pct
    type: sum
    expr: labor_pct
  - name: avg_price
    type: average
    expr: price
"#;

fn layer() -> SemanticLayer {
    let views = [STORES, STORE_DAYS]
        .iter()
        .map(|y| oxy_airlayer_compat::parse_view_yaml(y).expect("view parses"))
        .collect();
    SemanticLayer::new(views, None)
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()
}

/// grace_days 2; read-back on `promo_active`; guardrails on `refund_rate`
/// (anomaly) and `labor_pct` (trigger); the lever moves `promo_active`.
fn cfg() -> oxy_experiment::ExperimentConfig {
    let with_driver = yaml("", "  driver: store_days.promo_active\n", 28, 28, 7);
    parse(&with_guardrails(&with_driver, ANOMALY_AND_TRIGGER)).expect("parses")
}

fn unit_candidate() -> Candidate {
    Candidate::Waves(WavesCandidate {
        wave_sizes: vec![1],
        first_switch_on: NaiveDate::from_ymd_opt(2026, 3, 9).unwrap().into(),
        spacing_days: 0,
        blocks: None,
        pairs: false,
        assign_by: None,
        alpha: None,
    })
}

fn group_candidate() -> Candidate {
    match unit_candidate() {
        Candidate::Waves(w) => Candidate::Waves(WavesCandidate {
            assign_by: Some("store.region".to_string()),
            ..w
        }),
        other => other,
    }
}

type Calls = Arc<Mutex<Vec<QueryRequest>>>;

#[derive(Clone, Copy, Default)]
struct Script {
    fail_lag: bool,
    empty_panel: bool,
    fail_panel: bool,
}

fn row(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A warehouse that answers by the SHAPE of the request: no measures → the
/// membership; no dimensions → a lag probe; otherwise a per-unit-per-day panel.
fn warehouse(script: Script) -> (Box<QueryExecutor>, Calls) {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let exec: Box<QueryExecutor> = Box::new(move |r: &QueryRequest| {
        log.lock().unwrap().push(r.clone());
        if r.measures.is_empty() {
            return Ok(vec![
                row(&[
                    ("stores__store_id", json!("bondi")),
                    ("stores__region", json!("east")),
                ]),
                row(&[
                    ("stores__store_id", json!("newtown")),
                    ("stores__region", json!("east")),
                ]),
                row(&[
                    ("stores__store_id", json!("manly")),
                    ("stores__region", json!("west")),
                ]),
            ]);
        }
        let alias = r.measures[0].replace('.', "__");
        if r.dimensions.is_empty() {
            if script.fail_lag {
                return Err(EngineError::QueryError("lag probe refused".to_string()));
            }
            return Ok(vec![row(&[
                ("store_days__business_date__day", json!("2026-02-27")),
                (alias.as_str(), json!(3.0)),
            ])]);
        }
        if script.fail_panel {
            return Err(EngineError::QueryError("connection reset".to_string()));
        }
        if script.empty_panel {
            return Ok(vec![]);
        }
        let mut rows = Vec::new();
        for (unit, base) in [("bondi", 10.0), ("newtown", 12.0), ("manly", 30.0)] {
            for day in ["2026-02-25", "2026-02-26", "2026-02-27"] {
                rows.push(row(&[
                    ("store_days__store_id", json!(unit)),
                    ("store_days__business_date__day", json!(day)),
                    (alias.as_str(), json!(base)),
                ]));
            }
        }
        Ok(rows)
    });
    (exec, calls)
}

fn ask(candidate: &Candidate) -> ReadAsk {
    read_ask(&cfg(), candidate, now())
}

fn kinds(calls: &Calls) -> Vec<&'static str> {
    calls
        .lock()
        .unwrap()
        .iter()
        .map(|r| {
            if r.measures.is_empty() {
                "membership"
            } else if r.dimensions.is_empty() {
                "lag"
            } else {
                "panel"
            }
        })
        .collect()
}

#[test]
fn read_ask_collects_what_the_job_needs() {
    let a = ask(&unit_candidate());
    assert_eq!(
        (a.panel.entity.as_str(), a.panel.measure.as_str()),
        ("store", "store_days.beverages_revenue")
    );
    assert_eq!(a.panel.history.to_exclusive.to_string(), "2026-03-01");
    assert!(a.panel.history.from.is_none() && a.panel.membership.is_none());
    assert_eq!(
        a.lag_measures,
        vec![
            "store_days.promo_active".to_string(),
            "store_days.refund_rate".to_string(),
            "store_days.labor_pct".to_string(),
        ]
    );
    assert_eq!(a.spread_measures, vec!["store_days.labor_pct".to_string()]);
    let edge = a.prior_edge.expect("outcome.driver is set");
    assert_eq!(
        (edge.driver.as_str(), edge.target.as_str()),
        ("store_days.promo_active", "store_days.beverages_revenue")
    );
    assert_eq!(a.assign_by, None);
    assert_eq!(
        ask(&group_candidate()).assign_by.as_deref(),
        Some("store.region")
    );
    let sb = Candidate::Switchback(SwitchbackCandidate {
        first_period_on: NaiveDate::from_ymd_opt(2026, 3, 9).unwrap().into(),
        period_days: 7,
        pairs: 6,
        alpha: None,
    });
    assert_eq!(ask(&sb).assign_by, None, "a switchback has no groups");
}

#[test]
fn a_unit_request_reads_the_panel_the_lags_and_the_spreads_in_one_pass() {
    let (exec, calls) = warehouse(Script::default());
    let r = fetch_readings(&layer(), &*exec, &ask(&unit_candidate())).expect("readings");
    let PanelOutcome::Panel(panel) = &r.panel else {
        panic!("expected a panel")
    };
    assert_eq!(
        panel.units(),
        [
            "bondi".to_string(),
            "manly".to_string(),
            "newtown".to_string()
        ]
    );
    assert!(r.membership.is_none());
    assert_eq!(r.lags.len(), 3);
    assert_eq!(
        r.lags[0].latest_day,
        Some(NaiveDate::from_ymd_opt(2026, 2, 27).unwrap())
    );
    assert_eq!(r.spreads.len(), 1);
    assert_eq!(r.spreads[0].means["manly"], 30.0);
    assert!(
        matches!(r.prior, PriorReading::Absent { .. }),
        "the fixture view declares no edge"
    );
    assert!(r.notes.is_empty(), "{:?}", r.notes);
    assert!(
        !kinds(&calls).contains(&"membership"),
        "a unit request asks for no groups"
    );
}

#[test]
fn a_group_request_resolves_the_membership_first_and_sums_the_panel_by_it() {
    let (exec, calls) = warehouse(Script::default());
    let r = fetch_readings(&layer(), &*exec, &ask(&group_candidate())).expect("readings");
    assert_eq!(kinds(&calls)[0], "membership");
    let PanelOutcome::Panel(panel) = &r.panel else {
        panic!("expected a panel")
    };
    assert_eq!(
        panel.units(),
        ["east".to_string(), "west".to_string()],
        "groups, not stores"
    );
    let m = r
        .membership
        .expect("the membership the panel was summed by");
    assert_eq!(
        m.0["east"],
        vec!["bondi".to_string(), "newtown".to_string()]
    );
}

#[test]
fn a_diagnostic_that_cannot_be_read_becomes_a_note_and_the_panel_survives() {
    let (exec, _) = warehouse(Script {
        fail_lag: true,
        ..Script::default()
    });
    let r = fetch_readings(&layer(), &*exec, &ask(&unit_candidate())).expect("readings");
    assert!(matches!(r.panel, PanelOutcome::Panel(_)));
    assert!(r.lags.is_empty());
    assert_eq!(
        r.notes.len(),
        3,
        "one note per lag read that failed: {:?}",
        r.notes
    );
    assert!(
        r.notes[0].contains("`store_days.promo_active`")
            && r.notes[0].contains("could not be read"),
        "{}",
        r.notes[0]
    );
    assert!(r.notes[0].contains("lag probe refused"), "{}", r.notes[0]);
}

#[test]
fn an_empty_panel_skips_every_diagnostic() {
    let (exec, calls) = warehouse(Script {
        empty_panel: true,
        ..Script::default()
    });
    let r = fetch_readings(&layer(), &*exec, &ask(&unit_candidate())).expect("readings");
    assert!(matches!(r.panel, PanelOutcome::NoHistory { .. }));
    assert_eq!(
        kinds(&calls),
        vec!["panel"],
        "nothing else is worth a query"
    );
    assert!(r.lags.is_empty() && r.spreads.is_empty() && r.notes.is_empty());
}

#[test]
fn a_non_additive_target_under_assign_by_is_a_refused_value_that_skips_the_diagnostics() {
    let mut a = ask(&group_candidate());
    a.panel.measure = "store_days.avg_price".to_string();
    let (exec, calls) = warehouse(Script::default());
    let r = fetch_readings(&layer(), &*exec, &a).expect("a value, not an Err");
    let PanelOutcome::Refused { reason } = &r.panel else {
        panic!("expected a refused outcome")
    };
    assert!(reason.contains("`store_days.avg_price`"), "{reason}");
    assert!(
        !kinds(&calls).contains(&"lag") && !kinds(&calls).contains(&"panel"),
        "{:?}",
        kinds(&calls)
    );
}

#[test]
fn a_panel_fault_is_an_error_not_a_note() {
    let (exec, _) = warehouse(Script {
        fail_panel: true,
        ..Script::default()
    });
    let err = fetch_readings(&layer(), &*exec, &ask(&unit_candidate()))
        .err()
        .expect("must be an Err");
    assert!(
        matches!(err, OxyError::RuntimeError(ref m) if m.contains("connection reset")),
        "{err:?}"
    );
}

#[test]
fn a_model_refusal_is_a_validation_error_the_job_settles_as_refused() {
    let (exec, _) = warehouse(Script::default());
    let mut a = ask(&unit_candidate());
    a.panel.measure = "store_days.no_such".to_string();
    let err = fetch_readings(&layer(), &*exec, &a)
        .err()
        .expect("must be an Err");
    assert!(matches!(err, OxyError::ValidationError(_)), "{err:?}");
}

#[test]
fn warnings_assemble_lag_spread_notes_and_the_missing_prior() {
    let today = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
    let r = Readings {
        panel: PanelOutcome::NoHistory {
            measure: "m".to_string(),
            entity: "e".to_string(),
        },
        membership: None,
        lags: vec![LagReading {
            measure: "store_days.promo_active".to_string(),
            cadence: Some("weekly".to_string()),
            complete_through: None,
            latest_day: None,
        }],
        spreads: vec![UnitMeans {
            measure: "store_days.labor_pct".to_string(),
            means: [("a".to_string(), 1.0), ("b".to_string(), 9.0)]
                .into_iter()
                .collect(),
        }],
        prior: PriorReading::Absent {
            why: "no edge".to_string(),
        },
        notes: vec!["the arrival lag of `x` could not be read: boom".to_string()],
    };
    let w = warnings(&cfg(), &r, today);
    assert!(
        w.iter()
            .any(|x| x.contains("read-back measure `store_days.promo_active`")),
        "{w:?}"
    );
    assert!(
        w.iter()
            .any(|x| x.contains("trigger guardrail on `store_days.labor_pct`")),
        "{w:?}"
    );
    assert!(
        w.iter().any(|x| x.contains("could not be read: boom")),
        "{w:?}"
    );
    assert!(
        w.iter()
            .any(|x| x.contains("no prior_coefficient was frozen")),
        "{w:?}"
    );
}

#[test]
fn panel_only_says_what_the_source_did_not_read() {
    let r = Readings::panel_only(
        PanelOutcome::NoHistory {
            measure: "m".to_string(),
            entity: "e".to_string(),
        },
        "this panel source reads no lag, spread or prior",
    );
    assert_eq!(
        r.notes,
        vec!["this panel source reads no lag, spread or prior".to_string()]
    );
    assert!(r.lags.is_empty() && r.spreads.is_empty() && r.membership.is_none());
    assert_eq!(r.prior, PriorReading::NotAsked);
}

#[test]
fn a_filtered_target_reads_the_filtered_panel_and_marks_the_prior_edge_filtered() {
    use oxy_experiment::{FilterOp, MeasureFilter};
    let mut c = cfg();
    c.outcome.target_filter = vec![MeasureFilter {
        dimension: "store_days.region".into(),
        op: FilterOp::Equals,
        values: vec!["north".into()],
    }];
    let a = read_ask(&c, &unit_candidate(), now());
    assert_eq!(a.panel.filters, c.outcome.target_filter);
    assert!(a.prior_edge.expect("a driver is set").filtered);
    assert!(!ask(&unit_candidate()).prior_edge.unwrap().filtered);
}

#[test]
fn at_hour_grain_a_read_back_measure_with_no_time_of_day_is_named() {
    let mut c = cfg();
    c.design.grain = oxy_experiment::Grain::Hour;
    let a = read_ask(&c, &unit_candidate(), now());
    let notes = super::reads::hour_read_back_notes(&layer(), &a);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("hour by hour") && notes[0].contains("date"),
        "{}",
        notes[0]
    );
    // Below hour grain read-back reads days, and nothing is said.
    let a = read_ask(&cfg(), &unit_candidate(), now());
    assert!(super::reads::hour_read_back_notes(&layer(), &a).is_empty());
}
