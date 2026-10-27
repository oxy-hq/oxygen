//! The prior coefficient: the World Model's slope on the driver → target edge at
//! pricing, declared or fitted, frozen into the file (`internal-docs/experiments.md`,
//! "Predicted against measured").

use chrono::{Duration, NaiveDate};
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::metric_tree_fit::FittedDriver;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::schema::models::DriverForm;
use oxy_experiment::{PriorCoefficient, PriorSource};

use crate::server::api::metric_tree_groups::grouped_fit;

/// How far back the fit looks, up to and including today.
pub const PRIOR_FIT_DAYS: i64 = 365;

/// The edge the experiment's lever moves: `outcome.driver` → `outcome.target`.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorEdge {
    pub driver: String,
    pub target: String,
    /// The design filters the target or the driver: the semantic model's edge
    /// is between the unfiltered measures, so no slope of it is a prior.
    pub filtered: bool,
}

/// What the semantic model says about the edge.
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeFact {
    /// The target declares no driver entry from that measure.
    Missing,
    /// An edge with a direction and no magnitude: the fit may size it.
    Qualitative,
    /// A declared linear slope.
    Declared(f64),
    /// A form or coefficient vector that is not a per-unit slope.
    NotALinearSlope { form: String },
}

/// The target measure's `drivers:` entry for `driver`, as a fact.
pub fn edge_fact(layer: &SemanticLayer, target: &str, driver: &str) -> EdgeFact {
    let Some((view_name, measure_name)) = target.split_once('.') else {
        return EdgeFact::Missing;
    };
    let edge = layer
        .views
        .iter()
        .find(|v| v.name == view_name)
        .and_then(|v| v.measures_list().iter().find(|m| m.name == measure_name))
        .and_then(|m| m.drivers.as_ref())
        .and_then(|ds| ds.iter().find(|d| d.measure == driver));
    let Some(edge) = edge else {
        return EdgeFact::Missing;
    };
    if edge.coefficients.is_some() || !matches!(edge.form, None | Some(DriverForm::Linear)) {
        let form = edge.form.as_ref().map_or_else(
            || "multi-term coefficients".to_string(),
            ToString::to_string,
        );
        return EdgeFact::NotALinearSlope { form };
    }
    match edge.coefficient {
        Some(c) if c.is_finite() => EdgeFact::Declared(c),
        _ => EdgeFact::Qualitative,
    }
}

/// The usable linear slope among `fits` for `driver -> target`, or why there is none.
pub fn pick_fit(fits: &[FittedDriver], driver: &str, target: &str) -> Result<f64, String> {
    let fit = fits
        .iter()
        .find(|f| f.from == driver && f.to == target)
        .ok_or_else(|| "the fit returned no fit for the edge".to_string())?;
    if let Some(refusal) = &fit.refusal {
        return Err(refusal.clone());
    }
    if fit.form != DriverForm::Linear {
        return Err(format!(
            "it was fitted as {}, which is not a per-unit slope",
            fit.form
        ));
    }
    match fit.coefficient {
        Some(c) if c.is_finite() => Ok(c),
        _ => Err("the fit produced no coefficient".to_string()),
    }
}

/// What pricing learned about the prior.
#[derive(Debug, Clone, PartialEq)]
pub enum PriorReading {
    /// The experiment names no `outcome.driver`: there is nothing to look for.
    NotAsked,
    Frozen(PriorCoefficient),
    Absent {
        why: String,
    },
}

impl PriorReading {
    pub fn frozen(&self) -> Option<PriorCoefficient> {
        match self {
            Self::Frozen(p) => Some(p.clone()),
            _ => None,
        }
    }

    /// The proposal warning when a driver was named but no slope could be frozen.
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Absent { why } => Some(format!("no prior_coefficient was frozen: {why}")),
            _ => None,
        }
    }
}

fn absent(why: String) -> PriorReading {
    PriorReading::Absent { why }
}

/// Freeze the edge's slope: declared if it is, else fitted over the last
/// [`PRIOR_FIT_DAYS`]; absent, with the reason, if neither.
pub fn read_prior(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    edge: &PriorEdge,
    time_dimension: &str,
    today: NaiveDate,
) -> PriorReading {
    let PriorEdge {
        driver,
        target,
        filtered,
    } = edge;
    if *filtered {
        return absent(
            "the design filters its target or driver, and the semantic model's edge is between \
             the unfiltered measures"
                .to_string(),
        );
    }
    match edge_fact(layer, target, driver) {
        EdgeFact::Declared(value) => PriorReading::Frozen(PriorCoefficient {
            value,
            source: PriorSource::Declared,
        }),
        EdgeFact::Missing => absent(format!(
            "`{target}` declares no driver edge from `{driver}`, so the report has no prediction to test"
        )),
        EdgeFact::NotALinearSlope { form } => absent(format!(
            "the driver edge `{driver}` -> `{target}` declares {form}, which is not a per-unit slope, so the report has no prediction to test"
        )),
        EdgeFact::Qualitative => fitted(layer, executor, edge, time_dimension, today),
    }
}

fn fitted(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    edge: &PriorEdge,
    time_dimension: &str,
    today: NaiveDate,
) -> PriorReading {
    let tree = oxy_semantic::build_metric_tree(layer);
    let (from, to) = (
        (today - Duration::days(PRIOR_FIT_DAYS)).to_string(),
        today.to_string(),
    );
    // Rooted at the driver: the fit walks forward from its roots.
    let fits = grouped_fit(
        &tree,
        layer,
        std::slice::from_ref(&edge.driver),
        time_dimension,
        (&from, &to),
        &[],
        executor,
    );
    match pick_fit(&fits, &edge.driver, &edge.target) {
        Ok(value) => PriorReading::Frozen(PriorCoefficient {
            value,
            source: PriorSource::Fitted,
        }),
        Err(why) => absent(format!(
            "the driver edge `{}` -> `{}` declares no coefficient and the fit over the last {PRIOR_FIT_DAYS} days gave none ({why})",
            edge.driver, edge.target
        )),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use oxy_airlayer_compat::SemanticLayer;
    use oxy_airlayer_compat::engine::metric_tree_fit::FittedDriver;
    use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
    use oxy_airlayer_compat::engine::query::QueryRequest;
    use oxy_experiment::{PriorCoefficient, PriorSource};
    use serde_json::json;

    use super::*;

    const ADS: &str = r#"
name: ads
datasource: warehouse
table: ads
entities:
  - name: ad_day
    type: primary
    key: ad_day_id
dimensions:
  - name: ad_date
    type: date
    expr: ad_date
  - name: ad_day_id
    type: string
    expr: ad_day_id
measures:
  - name: spend
    type: sum
    expr: spend
  - name: rev_declared
    type: sum
    expr: revenue
    drivers:
      - measure: ads.spend
        form: linear
        coefficient: 4.2
  - name: rev_bare_coefficient
    type: sum
    expr: revenue
    drivers:
      - measure: ads.spend
        coefficient: 2.5
  - name: rev_qualitative
    type: sum
    expr: revenue
    drivers:
      - measure: ads.spend
        direction: positive
  - name: rev_loglog
    type: sum
    expr: revenue
    drivers:
      - measure: ads.spend
        form: log-log
        coefficient: 0.3
  - name: rev_quadratic
    type: sum
    expr: revenue
    drivers:
      - measure: ads.spend
        form: quadratic
        coefficients: [1.0, -0.1]
  - name: rev_unlinked
    type: sum
    expr: revenue
"#;

    fn layer() -> SemanticLayer {
        SemanticLayer::new(
            vec![oxy_airlayer_compat::parse_view_yaml(ADS).expect("view parses")],
            None,
        )
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 20).unwrap()
    }

    fn edge(target: &str) -> PriorEdge {
        PriorEdge {
            driver: "ads.spend".to_string(),
            target: format!("ads.{target}"),
            filtered: false,
        }
    }

    fn fit(v: serde_json::Value) -> FittedDriver {
        serde_json::from_value(v).expect("a FittedDriver defaults every field but from/to")
    }

    #[test]
    fn a_declared_linear_edge_is_a_declared_fact() {
        assert_eq!(
            edge_fact(&layer(), "ads.rev_declared", "ads.spend"),
            EdgeFact::Declared(4.2)
        );
        assert_eq!(
            edge_fact(&layer(), "ads.rev_bare_coefficient", "ads.spend"),
            EdgeFact::Declared(2.5),
            "no `form:` with one coefficient is linear"
        );
    }

    #[test]
    fn an_edge_with_a_direction_and_no_magnitude_is_qualitative() {
        assert_eq!(
            edge_fact(&layer(), "ads.rev_qualitative", "ads.spend"),
            EdgeFact::Qualitative
        );
    }

    #[test]
    fn a_non_linear_or_multi_term_edge_is_not_a_linear_slope() {
        let loglog = edge_fact(&layer(), "ads.rev_loglog", "ads.spend");
        assert_eq!(
            loglog,
            EdgeFact::NotALinearSlope {
                form: "log-log".to_string()
            }
        );
        let quad = edge_fact(&layer(), "ads.rev_quadratic", "ads.spend");
        assert!(matches!(quad, EdgeFact::NotALinearSlope { .. }), "{quad:?}");
    }

    #[test]
    fn no_edge_from_that_driver_is_missing() {
        assert_eq!(
            edge_fact(&layer(), "ads.rev_unlinked", "ads.spend"),
            EdgeFact::Missing
        );
        assert_eq!(
            edge_fact(&layer(), "ads.rev_declared", "ads.other"),
            EdgeFact::Missing
        );
        assert_eq!(
            edge_fact(&layer(), "ads.no_such_measure", "ads.spend"),
            EdgeFact::Missing
        );
        assert_eq!(
            edge_fact(&layer(), "nonsense", "ads.spend"),
            EdgeFact::Missing
        );
    }

    #[test]
    fn a_declared_edge_is_frozen_as_declared_without_touching_the_warehouse() {
        let exec: Box<QueryExecutor> =
            Box::new(|_r: &QueryRequest| panic!("a declared slope needs no query"));
        let got = read_prior(
            &layer(),
            &*exec,
            &edge("rev_declared"),
            "ads.ad_date",
            today(),
        );
        assert_eq!(
            got,
            PriorReading::Frozen(PriorCoefficient {
                value: 4.2,
                source: PriorSource::Declared
            })
        );
        assert_eq!(got.frozen().unwrap().value, 4.2);
        assert!(got.warning().is_none());
    }

    #[test]
    fn a_filtered_target_freezes_no_prior_even_when_the_edge_is_declared() {
        // The declared slope is between the unfiltered measures; one product's
        // sales per ad dollar is a different number.
        let exec: Box<QueryExecutor> =
            Box::new(|_r: &QueryRequest| panic!("no query for a filtered edge"));
        let filtered = PriorEdge {
            filtered: true,
            ..edge("rev_declared")
        };
        let got = read_prior(&layer(), &*exec, &filtered, "ads.ad_date", today());
        let PriorReading::Absent { why } = &got else {
            panic!("{got:?}")
        };
        assert!(why.contains("filtered"), "{why}");
    }

    #[test]
    fn a_missing_edge_is_absent_and_says_there_is_nothing_to_test() {
        let exec: Box<QueryExecutor> =
            Box::new(|_r: &QueryRequest| panic!("no query for a missing edge"));
        let got = read_prior(
            &layer(),
            &*exec,
            &edge("rev_unlinked"),
            "ads.ad_date",
            today(),
        );
        let PriorReading::Absent { why } = &got else {
            panic!("{got:?}")
        };
        assert!(
            why.contains("declares no driver edge") && why.contains("`ads.spend`"),
            "{why}"
        );
        let warning = got.warning().expect("a warning");
        assert!(warning.contains("no prediction to test"), "{warning}");
        assert!(got.frozen().is_none());
    }

    #[test]
    fn a_non_linear_edge_is_absent_naming_its_form() {
        let exec: Box<QueryExecutor> = Box::new(|_r: &QueryRequest| panic!("no query"));
        let got = read_prior(
            &layer(),
            &*exec,
            &edge("rev_loglog"),
            "ads.ad_date",
            today(),
        );
        let PriorReading::Absent { why } = got else {
            panic!("expected absent")
        };
        assert!(
            why.contains("log-log") && why.contains("not a per-unit slope"),
            "{why}"
        );
    }

    #[test]
    fn a_qualitative_edge_whose_fit_cannot_run_is_absent_with_the_reason() {
        let exec: Box<QueryExecutor> = Box::new(|_r: &QueryRequest| {
            Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                "connection reset".to_string(),
            ))
        });
        let got = read_prior(
            &layer(),
            &*exec,
            &edge("rev_qualitative"),
            "ads.ad_date",
            today(),
        );
        let PriorReading::Absent { why } = got else {
            panic!("expected absent")
        };
        assert!(
            why.contains("declares no coefficient") && why.contains("365 days"),
            "{why}"
        );
    }

    #[test]
    fn a_usable_fit_is_picked_by_its_endpoints() {
        let fits = vec![
            fit(
                json!({"from": "ads.other", "to": "ads.rev", "coefficient": 9.0, "coefficients": [9.0]}),
            ),
            fit(
                json!({"from": "ads.spend", "to": "ads.rev", "coefficient": 3.5, "coefficients": [3.5]}),
            ),
        ];
        assert_eq!(pick_fit(&fits, "ads.spend", "ads.rev"), Ok(3.5));
    }

    #[test]
    fn a_refused_non_linear_or_absent_fit_is_not_a_prior() {
        let refused = vec![fit(
            json!({"from": "ads.spend", "to": "ads.rev", "refusal": "too few paired rows"}),
        )];
        assert!(
            pick_fit(&refused, "ads.spend", "ads.rev")
                .unwrap_err()
                .contains("too few paired rows")
        );

        let curved = vec![fit(
            json!({"from": "ads.spend", "to": "ads.rev", "form": "log-log", "coefficient": 0.4, "coefficients": [0.4]}),
        )];
        assert!(
            pick_fit(&curved, "ads.spend", "ads.rev")
                .unwrap_err()
                .contains("log-log")
        );

        let none: Vec<FittedDriver> = vec![];
        assert!(
            pick_fit(&none, "ads.spend", "ads.rev")
                .unwrap_err()
                .contains("no fit")
        );

        let not_finite = vec![fit(
            json!({"from": "ads.spend", "to": "ads.rev", "coefficient": null}),
        )];
        assert!(pick_fit(&not_finite, "ads.spend", "ads.rev").is_err());
    }
}
