//! Arrival lag: how late a measure's data lands, from the view's freshness
//! contract where one is declared, else the gap to the latest day it holds
//! (`internal-docs/experiments.md`, "Data that arrives late").

use chrono::{Duration, NaiveDate};
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::{
    FilterOperator, QueryFilter, QueryRequest, TimeDimensionQuery,
};
use oxy_experiment::expr::{parse_comparison, parse_trigger};
use oxy_experiment::{ExperimentConfig, GuardrailCriteria};
use serde_json::{Map, Value};

use super::panel::{measure_view, parse_day, parse_value, time_dimension};

/// How far back the observed-lag probe looks.
pub const LAG_LOOKBACK_DAYS: i64 = 120;

/// What was learned about one measure's arrival: its view's freshness contract
/// (raw text) and the latest day it holds a value for.
#[derive(Debug, Clone, PartialEq)]
pub struct LagReading {
    pub measure: String,
    /// `meta: freshness_expected_cadence`, first value.
    pub cadence: Option<String>,
    /// `meta: freshness_complete_through`, first value.
    pub complete_through: Option<String>,
    pub latest_day: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lag {
    pub days: i64,
    /// Where the number came from, in words the warning quotes.
    pub basis: String,
}

/// The first integer after `marker`, e.g. `digits_after("t-7 days", "t-") == Some(7)`.
fn digits_after(s: &str, marker: &str) -> Option<i64> {
    let rest = &s[s.find(marker)? + marker.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn through_days(text: &str) -> Option<i64> {
    let t = text.to_lowercase();
    if t.contains("prior full day") || t.contains("previous full day") {
        Some(1)
    } else if t.contains("prior full week") || t.contains("previous full week") {
        Some(7)
    } else {
        digits_after(&t, "t-")
    }
}

fn cadence_days(text: &str) -> Option<i64> {
    let t = text.to_lowercase();
    if t.contains("biweekly") || t.contains("bi-weekly") || t.contains("fortnight") {
        Some(14)
    } else if t.contains("weekly") {
        Some(7)
    } else if t.contains("monthly") {
        Some(30)
    } else if t.contains("daily") || t.contains("hourly") || t.contains("intraday") {
        Some(0)
    } else {
        digits_after(&t, "every ")
    }
}

/// The lag a freshness contract declares, in days: the larger of what
/// `complete_through` and `cadence` imply. `None` when neither says anything this
/// can read — which is **not** "no lag".
pub fn declared_lag_days(complete_through: Option<&str>, cadence: Option<&str>) -> Option<i64> {
    let through = complete_through.and_then(through_days);
    let cadence = cadence.and_then(cadence_days);
    (through.is_some() || cadence.is_some()).then(|| through.unwrap_or(0).max(cadence.unwrap_or(0)))
}

fn contract_text(r: &LagReading) -> String {
    [
        r.cadence.as_deref().map(|c| format!("cadence: {c}")),
        r.complete_through
            .as_deref()
            .map(|c| format!("complete through: {c}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("; ")
}

/// The measure's lag, or why it is unknown. The contract wins where it is
/// readable; otherwise the gap between `today` and the latest day it holds.
pub fn lag_of(r: &LagReading, today: NaiveDate) -> Result<Lag, String> {
    if let Some(days) = declared_lag_days(r.complete_through.as_deref(), r.cadence.as_deref()) {
        return Ok(Lag {
            days,
            basis: format!("the view's freshness contract, {}", contract_text(r)),
        });
    }
    let unread = if r.cadence.is_some() || r.complete_through.is_some() {
        format!(
            " (its freshness contract, {}, is not one this reads)",
            contract_text(r)
        )
    } else {
        String::new()
    };
    match r.latest_day {
        Some(d) => Ok(Lag {
            days: (today - d).num_days().max(0),
            basis: format!("observed: the latest day it holds is {d}{unread}"),
        }),
        None => Err(format!(
            "it holds no rows in the {LAG_LOOKBACK_DAYS} days before {today}{unread}"
        )),
    }
}

fn push_unique(out: &mut Vec<String>, measure: Option<String>) {
    if let Some(m) = measure
        && !out.contains(&m)
    {
        out.push(m);
    }
}

fn comparison_measure(expr: &str) -> Option<String> {
    parse_comparison(expr).ok().map(|c| c.measure)
}

/// The measures `lever.read_back` reads, `when_on` then `when_off`, deduplicated.
pub fn read_back_measures(cfg: &ExperimentConfig) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(lever) = &cfg.lever {
        push_unique(&mut out, comparison_measure(&lever.read_back.when_on));
        push_unique(&mut out, comparison_measure(&lever.read_back.when_off));
    }
    out
}

/// The measures the guardrails read, in file order, deduplicated.
pub fn guardrail_measures(cfg: &ExperimentConfig) -> Vec<String> {
    let mut out = Vec::new();
    for g in &cfg.guardrails {
        match &g.criteria {
            GuardrailCriteria::Anomaly { measure, .. } => {
                push_unique(&mut out, Some(measure.clone()))
            }
            GuardrailCriteria::Trigger { when } => {
                // A ratio trigger needs both its measures to have arrived.
                for m in parse_trigger(when).iter().flat_map(|t| t.measures()) {
                    push_unique(&mut out, Some(m.to_string()));
                }
            }
        }
    }
    out
}

/// The warnings the readings earn: a read-back measure whose lag reaches
/// `grace_days`, a guardrail measure whose lag exceeds a day, and any lag that
/// could not be determined.
pub fn lag_warnings(
    cfg: &ExperimentConfig,
    readings: &[LagReading],
    today: NaiveDate,
) -> Vec<String> {
    let grace = i64::from(cfg.grace_days());
    let (read_back, guards) = (read_back_measures(cfg), guardrail_measures(cfg));
    let mut out = Vec::new();
    for r in readings {
        let lag = match lag_of(r, today) {
            Ok(l) => l,
            Err(why) => {
                out.push(format!(
                    "the arrival lag of `{}` could not be determined: {why}",
                    r.measure
                ));
                continue;
            }
        };
        if read_back.contains(&r.measure) && lag.days >= grace {
            out.push(format!(
                "read-back measure `{}` arrives about {} days late ({}) and lever.read_back.grace_days is {grace}: every flip would read as unconfirmed before the data could confirm it. Raise grace_days above {}, or read back through a fresher measure",
                r.measure, lag.days, lag.basis, lag.days
            ));
        }
        if guards.contains(&r.measure) && lag.days > 1 {
            out.push(format!(
                "guardrail measure `{}` arrives about {} days late ({}): the guardrail can only see a breach that late",
                r.measure, lag.days, lag.basis
            ));
        }
    }
    out
}

fn lag_request(measure: &str, time_dimension: &str, today: NaiveDate) -> QueryRequest {
    let bound = |operator: FilterOperator, date: NaiveDate| QueryFilter {
        member: Some(time_dimension.to_string()),
        operator: Some(operator),
        values: vec![date.to_string()],
        and: None,
        or: None,
    };
    QueryRequest {
        measures: vec![measure.to_string()],
        time_dimensions: vec![TimeDimensionQuery {
            dimension: time_dimension.to_string(),
            granularity: Some("day".to_string()),
            date_range: None,
        }],
        filters: vec![
            bound(
                FilterOperator::AfterOrOnDate,
                today - Duration::days(LAG_LOOKBACK_DAYS),
            ),
            bound(FilterOperator::BeforeOrOnDate, today),
        ],
        limit: Some(UNBOUNDED_QUERY_LIMIT),
        ..QueryRequest::new()
    }
}

/// The latest day carrying a non-null value of the measure.
fn latest_day(
    rows: &[Map<String, Value>],
    time_dimension: &str,
    measure: &str,
) -> Option<NaiveDate> {
    let day_alias = format!("{}__day", time_dimension.replace('.', "__"));
    let value_alias = measure.replace('.', "__");
    rows.iter()
        .filter(|r| r.get(&value_alias).and_then(parse_value).is_some())
        .filter_map(|r| r.get(&day_alias).and_then(parse_day))
        .max()
}

/// Read one measure's freshness contract and latest day. `requested_td` is
/// `design.time_dimension` **only when it belongs to this measure's view**;
/// otherwise `None` and the view's only date dimension is used. `Err(why)` is a
/// note for the proposal, never a fault.
pub fn read_lag(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    measure: &str,
    requested_td: Option<&str>,
    today: NaiveDate,
) -> Result<LagReading, String> {
    let view = measure_view(layer, measure).map_err(|e| e.to_string())?;
    let td = time_dimension(view, measure, requested_td).map_err(|e| e.to_string())?;
    let rows = executor(&lag_request(measure, &td, today))
        .map_err(|e| format!("the lag probe failed: {e}"))?;
    let meta = |key: &str| {
        view.meta
            .as_ref()
            .and_then(|m| m.get(key))
            .and_then(|v| v.first())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Ok(LagReading {
        measure: measure.to_string(),
        cadence: meta("freshness_expected_cadence"),
        complete_through: meta("freshness_complete_through"),
        latest_day: latest_day(&rows, &td, measure),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use oxy_airlayer_compat::SemanticLayer;
    use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
    use oxy_airlayer_compat::engine::query::QueryRequest;
    use oxy_experiment::parse;
    use serde_json::json;

    use super::*;
    use crate::server::experiment::testing::{ANOMALY_AND_TRIGGER, with_guardrails, yaml};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn today() -> NaiveDate {
        day(2026, 3, 20)
    }

    fn reading(
        measure: &str,
        through: Option<&str>,
        cadence: Option<&str>,
        latest: Option<NaiveDate>,
    ) -> LagReading {
        LagReading {
            measure: measure.to_string(),
            cadence: cadence.map(str::to_string),
            complete_through: through.map(str::to_string),
            latest_day: latest,
        }
    }

    /// grace_days is 2; read-back reads `store_days.promo_active`; the guardrails
    /// read `store_days.refund_rate` (anomaly) and `store_days.labor_pct` (trigger).
    fn cfg() -> oxy_experiment::ExperimentConfig {
        parse(&with_guardrails(
            &yaml("", "", 28, 28, 7),
            ANOMALY_AND_TRIGGER,
        ))
        .expect("parses")
    }

    #[test]
    fn a_ratio_trigger_reads_the_lag_of_both_its_measures() {
        let c = parse(&with_guardrails(
            &yaml("", "", 28, 28, 7),
            "  - criteria: { type: trigger, when: \"{{store_days.cost}} / {{store_days.sales}} > 0.6\" }\n    action: abort\n",
        ))
        .expect("parses");
        assert_eq!(
            guardrail_measures(&c),
            vec![
                "store_days.cost".to_string(),
                "store_days.sales".to_string()
            ]
        );
    }

    #[test]
    fn the_documented_contract_phrases_parse() {
        for (through, cadence, want) in [
            (Some("prior full day (America/Los_Angeles)"), None, Some(1)),
            (Some("Prior full week"), None, Some(7)),
            (Some("T-3 days"), None, Some(3)),
            (None, Some("weekly"), Some(7)),
            (None, Some("bi-weekly"), Some(14)),
            (None, Some("monthly"), Some(30)),
            (None, Some("several times daily"), Some(0)),
            (None, Some("every 3 days"), Some(3)),
            (Some("prior full day"), Some("weekly"), Some(7)),
            (Some("whenever finance closes the books"), None, None),
            (None, None, None),
        ] {
            assert_eq!(
                declared_lag_days(through, cadence),
                want,
                "{through:?} / {cadence:?}"
            );
        }
    }

    #[test]
    fn the_declared_contract_wins_over_the_observed_gap() {
        // Review Focus 7. Weekly cadence says 7 days; the data happens to hold
        // yesterday. The contract is what the owner promised the next run.
        let r = reading("m.x", None, Some("weekly"), Some(day(2026, 3, 19)));
        let lag = lag_of(&r, today()).unwrap();
        assert_eq!(lag.days, 7);
        assert!(
            lag.basis.contains("freshness contract") && lag.basis.contains("weekly"),
            "{}",
            lag.basis
        );
    }

    #[test]
    fn without_a_contract_the_gap_to_the_latest_day_is_the_lag() {
        let lag = lag_of(&reading("m.x", None, None, Some(day(2026, 3, 13))), today()).unwrap();
        assert_eq!(lag.days, 7);
        assert!(
            lag.basis.contains("observed") && lag.basis.contains("2026-03-13"),
            "{}",
            lag.basis
        );
        let fresh = lag_of(&reading("m.x", None, None, Some(today())), today()).unwrap();
        assert_eq!(fresh.days, 0);
    }

    #[test]
    fn a_contract_this_cannot_read_falls_back_to_the_gap_and_says_so() {
        let r = reading(
            "m.x",
            Some("whenever finance closes the books"),
            None,
            Some(day(2026, 3, 18)),
        );
        let lag = lag_of(&r, today()).unwrap();
        assert_eq!(lag.days, 2);
        assert!(lag.basis.contains("is not one this reads"), "{}", lag.basis);
    }

    #[test]
    fn a_measure_with_no_rows_has_an_unknown_lag_not_a_zero_one() {
        let err = lag_of(&reading("m.x", None, None, None), today()).unwrap_err();
        assert!(err.contains("no rows") && err.contains("120 days"), "{err}");
    }

    #[test]
    fn the_measures_to_read_come_from_the_expressions() {
        let c = cfg();
        assert_eq!(
            read_back_measures(&c),
            vec!["store_days.promo_active".to_string()]
        );
        assert_eq!(
            guardrail_measures(&c),
            vec![
                "store_days.refund_rate".to_string(),
                "store_days.labor_pct".to_string()
            ]
        );
    }

    #[test]
    fn a_read_back_lag_equal_to_the_grace_warns() {
        // Review Focus 7. grace_days is 2: a lag of 2 reads every flip as
        // unconfirmed on the last day of the grace; a lag of 1 does not.
        let at = |days: i64| {
            vec![reading(
                "store_days.promo_active",
                None,
                None,
                Some(today() - chrono::Duration::days(days)),
            )]
        };
        let warned = lag_warnings(&cfg(), &at(2), today());
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0].contains("`store_days.promo_active`")
                && warned[0].contains("2 days")
                && warned[0].contains("grace_days is 2"),
            "{}",
            warned[0]
        );
        assert!(warned[0].contains("unconfirmed"), "{}", warned[0]);
        assert!(lag_warnings(&cfg(), &at(1), today()).is_empty());
    }

    #[test]
    fn a_guardrail_lagging_more_than_a_day_warns_and_says_how_late() {
        let late = |days: i64| {
            vec![
                reading(
                    "store_days.refund_rate",
                    None,
                    None,
                    Some(today() - chrono::Duration::days(days)),
                ),
                reading(
                    "store_days.labor_pct",
                    None,
                    None,
                    Some(today() - chrono::Duration::days(1)),
                ),
            ]
        };
        let warned = lag_warnings(&cfg(), &late(3), today());
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0].contains("`store_days.refund_rate`") && warned[0].contains("3 days late"),
            "{}",
            warned[0]
        );
        assert!(
            lag_warnings(&cfg(), &late(1), today()).is_empty(),
            "a day is allowed"
        );
    }

    #[test]
    fn a_lag_that_cannot_be_determined_is_said_not_assumed_fine() {
        let readings = vec![reading("store_days.promo_active", None, None, None)];
        let warned = lag_warnings(&cfg(), &readings, today());
        assert_eq!(warned.len(), 1);
        assert!(
            warned[0].contains("could not be determined")
                && warned[0].contains("`store_days.promo_active`"),
            "{}",
            warned[0]
        );
    }

    #[test]
    fn a_measure_that_is_neither_read_back_nor_a_guardrail_never_warns() {
        let readings = vec![reading("store_days.other", Some("T-30"), None, None)];
        assert!(
            lag_warnings(&cfg(), &readings, today())
                .iter()
                .all(|w| !w.contains("grace_days"))
        );
    }

    const PROMOS: &str = r#"
name: store_days
datasource: warehouse
table: store_days
meta:
  freshness_expected_cadence: ["weekly"]
  freshness_complete_through: ["prior full day (America/Los_Angeles)"]
entities:
  - name: store_day
    type: primary
    key: store_day_id
dimensions:
  - name: business_date
    type: date
    expr: business_date
  - name: store_day_id
    type: string
    expr: store_day_id
measures:
  - name: promo_active
    type: sum
    expr: promo_active
"#;

    fn layer() -> SemanticLayer {
        SemanticLayer::new(
            vec![oxy_airlayer_compat::parse_view_yaml(PROMOS).expect("view parses")],
            None,
        )
    }

    #[test]
    fn read_lag_returns_the_contract_and_the_latest_non_null_day() {
        let rows = vec![
            json!({"store_days__business_date__day": "2026-03-10", "store_days__promo_active": 4}),
            json!({"store_days__business_date__day": "2026-03-16", "store_days__promo_active": 2}),
            json!({"store_days__business_date__day": "2026-03-17", "store_days__promo_active": null}),
        ]
        .into_iter()
        .map(|v| v.as_object().unwrap().clone())
        .collect::<Vec<_>>();
        let exec: Box<QueryExecutor> = Box::new(move |req: &QueryRequest| {
            assert_eq!(req.measures, vec!["store_days.promo_active".to_string()]);
            assert_eq!(req.time_dimensions[0].granularity.as_deref(), Some("day"));
            assert!(req.filters.len() == 2 && req.limit.is_some(), "{req:?}");
            Ok(rows.clone())
        });
        let got = read_lag(&layer(), &*exec, "store_days.promo_active", None, today()).unwrap();
        assert_eq!(
            got.latest_day,
            Some(day(2026, 3, 16)),
            "a null value is not a day the measure holds"
        );
        assert_eq!(got.cadence.as_deref(), Some("weekly"));
        assert_eq!(
            got.complete_through.as_deref(),
            Some("prior full day (America/Los_Angeles)")
        );
    }

    #[test]
    fn read_lag_explains_why_a_measure_cannot_be_read() {
        let exec: Box<QueryExecutor> = Box::new(|_r: &QueryRequest| Ok(vec![]));
        let unknown = read_lag(&layer(), &*exec, "store_days.nope", None, today()).unwrap_err();
        assert!(unknown.contains("store_days.nope"), "{unknown}");
        let failing: Box<QueryExecutor> = Box::new(|_r: &QueryRequest| {
            Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                "connection reset".to_string(),
            ))
        });
        let broken = read_lag(
            &layer(),
            &*failing,
            "store_days.promo_active",
            None,
            today(),
        )
        .unwrap_err();
        assert!(broken.contains("connection reset"), "{broken}");
    }
}
