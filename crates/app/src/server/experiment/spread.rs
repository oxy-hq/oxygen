//! Trigger spread: one threshold across units of very different size
//! (`internal-docs/experiments.md`, "Guardrails").

use std::collections::BTreeMap;

use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_experiment::expr::{Operand, Trigger, parse_trigger};
use oxy_experiment::{ExperimentConfig, GuardrailCriteria};

use super::panel::{PanelRequest, build_request, resolve_spec, rows_to_triples};

/// The spec's rule: warn when the largest unit's mean is *more than* this many
/// times the smallest's.
pub const TRIGGER_SPREAD_RATIO: f64 = 2.0;

/// One measure's mean per unit over the pricing history.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitMeans {
    pub measure: String,
    pub means: BTreeMap<String, f64>,
}

/// The measures single-measure `trigger` guardrails read, in file order,
/// deduplicated. A ratio trigger is left out: a ratio does not scale with the unit,
/// so one bar fits every unit and there is no spread to warn about.
pub fn trigger_measures(cfg: &ExperimentConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in &cfg.guardrails {
        if let GuardrailCriteria::Trigger { when } = &g.criteria
            && let Ok(Trigger {
                lhs: Operand::Measure(m),
                ..
            }) = parse_trigger(when)
            && !out.contains(&m)
        {
            out.push(m);
        }
    }
    out
}

fn spread_warning(s: &UnitMeans) -> Option<String> {
    let positive: Vec<(&String, f64)> = s
        .means
        .iter()
        .map(|(u, m)| (u, *m))
        .filter(|(_, m)| m.is_finite() && *m > 0.0)
        .collect();
    let small = positive.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let big = positive.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    (positive.len() >= 2 && big.1 > TRIGGER_SPREAD_RATIO * small.1).then(|| {
        format!(
            "the trigger guardrail on `{}` applies one threshold to every unit, but unit means over the pricing history differ by {:.1}x (`{}` {:.2} against `{}` {:.2}): the bar sits at a different place in each unit's range. An `anomaly` guardrail judges each unit against its own history",
            s.measure, big.1 / small.1, big.0, big.1, small.0, small.1
        )
    })
}

/// One warning per trigger measure whose largest unit mean is more than twice the smallest's.
pub fn spread_warnings(spreads: &[UnitMeans]) -> Vec<String> {
    spreads.iter().filter_map(spread_warning).collect()
}

fn means_of(triples: Vec<(String, i64, f64)>) -> BTreeMap<String, f64> {
    let mut acc: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for (unit, _day, value) in triples {
        let slot = acc.entry(unit).or_default();
        slot.0 += value;
        slot.1 += 1;
    }
    acc.into_iter()
        .map(|(u, (sum, n))| (u, sum / n as f64))
        .collect()
}

/// Read a measure's per-unit mean through the same resolution and query the
/// panel uses. `Err(why)` is a note for the proposal, never a fault.
pub fn read_unit_means(
    layer: &SemanticLayer,
    executor: &QueryExecutor,
    req: &PanelRequest,
) -> Result<UnitMeans, String> {
    let spec = resolve_spec(layer, req).map_err(|e| e.to_string())?;
    let rows = executor(&build_request(&spec, &req.history))
        .map_err(|e| format!("the query failed: {e}"))?;
    if rows.is_empty() {
        return Err(format!("it holds no history over {}", req.entity));
    }
    let triples = rows_to_triples(&rows, &spec).map_err(|e| e.to_string())?;
    Ok(UnitMeans {
        measure: req.measure.clone(),
        means: means_of(triples),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oxy_experiment::parse;

    use super::*;
    use crate::server::experiment::testing::{ANOMALY_AND_TRIGGER, with_guardrails, yaml};

    fn means(measure: &str, pairs: &[(&str, f64)]) -> UnitMeans {
        UnitMeans {
            measure: measure.to_string(),
            means: pairs
                .iter()
                .map(|(u, m)| (u.to_string(), *m))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn only_trigger_guardrails_contribute_a_measure() {
        let c = parse(&with_guardrails(
            &yaml("", "", 28, 28, 7),
            ANOMALY_AND_TRIGGER,
        ))
        .expect("parses");
        assert_eq!(
            trigger_measures(&c),
            vec!["store_days.labor_pct".to_string()]
        );
    }

    #[test]
    fn a_ratio_trigger_contributes_no_spread_measure() {
        // A ratio is already scale-free: one bar fits every unit, which is why it is written.
        let c = parse(&with_guardrails(
            &yaml("", "", 28, 28, 7),
            "  - criteria: { type: trigger, when: \"{{store_days.cost}} / {{store_days.sales}} > 0.6\" }\n    action: abort\n",
        ))
        .expect("parses");
        assert!(trigger_measures(&c).is_empty());
    }

    #[test]
    fn more_than_twice_the_smallest_warns_naming_both_units_and_the_alternative() {
        let w = spread_warnings(&[means(
            "store_days.labor_cost",
            &[("bondi", 10.0), ("manly", 25.0), ("newtown", 12.0)],
        )]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`store_days.labor_cost`"), "{}", w[0]);
        assert!(
            w[0].contains("`manly`") && w[0].contains("`bondi`") && w[0].contains("2.5"),
            "{}",
            w[0]
        );
        assert!(w[0].contains("anomaly"), "{}", w[0]);
    }

    #[test]
    fn exactly_twice_the_smallest_does_not_warn() {
        // Review Focus 8. "more than twice": 20 against 10 is exactly twice.
        assert!(spread_warnings(&[means("m.x", &[("a", 10.0), ("b", 20.0)])]).is_empty());
        assert_eq!(
            spread_warnings(&[means("m.x", &[("a", 10.0), ("b", 20.01)])]).len(),
            1
        );
    }

    #[test]
    fn units_at_or_below_zero_are_left_out_and_one_unit_gives_no_opinion() {
        assert!(
            spread_warnings(&[means("m.x", &[("a", 0.0), ("b", 50.0)])]).is_empty(),
            "a lone positive unit"
        );
        assert!(
            spread_warnings(&[means("m.x", &[("a", f64::NAN), ("b", 50.0), ("c", -3.0)])])
                .is_empty()
        );
        assert!(spread_warnings(&[means("m.x", &[])]).is_empty());
        assert_eq!(
            spread_warnings(&[means("m.x", &[("a", 0.0), ("b", 5.0), ("c", 50.0)])]).len(),
            1
        );
    }

    #[test]
    fn each_measure_is_judged_on_its_own() {
        let w = spread_warnings(&[
            means("m.wide", &[("a", 1.0), ("b", 9.0)]),
            means("m.flat", &[("a", 5.0), ("b", 6.0)]),
        ]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("`m.wide`"));
    }

    #[test]
    fn unit_means_average_each_units_days() {
        let triples = vec![
            ("a".to_string(), 1, 10.0),
            ("a".to_string(), 2, 20.0),
            ("b".to_string(), 1, 3.0),
        ];
        let m = means_of(triples);
        assert_eq!(m["a"], 15.0);
        assert_eq!(m["b"], 3.0);
    }
}
