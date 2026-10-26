//! Pure cohort helpers — no HTTP, no database, no filesystem.
//!
//! Shared by both surfaces ([`super`] and
//! [`crate::server::api::projects::cohort`]) because the answers must not
//! differ: which statistic a string means, whether the truncation guard could
//! be installed, and which cohort a measure defaults to are facts about the
//! semantic model, not about who is asking.

use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::cohort::{augment_failure_reason, augment_layer_for_cohort};
use oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic;

/// Soft cap — a cohort resolution fires two entity-grain pulls; failing loud
/// beats hanging. Matches the metric-tree explain deadline.
///
/// One constant for both surfaces: they query the same warehouse, so the
/// deadline is one number in one place rather than two that can be tuned
/// apart — the same reasoning as `BASELINE_TIMEOUT` in `api::metric_tree`.
pub(crate) const COHORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// Parse the statistic, defaulting to `median`.
pub(crate) fn parse_statistic(raw: Option<&str>) -> Result<BenchmarkStatistic, String> {
    match raw.unwrap_or("median") {
        "median" => Ok(BenchmarkStatistic::Median),
        "p75" => Ok(BenchmarkStatistic::P75),
        "best_peer" => Ok(BenchmarkStatistic::BestPeer),
        other => Err(format!(
            "unknown statistic `{other}` — expected one of median, p75, best_peer"
        )),
    }
}

/// Clone `layer` and install the synthetic `__cohort_total__` measure that the
/// truncation guard depends on.
///
/// **This is the load-bearing call.** `augment_layer_for_cohort` returning
/// `false` does not fail loudly: skipping it silently disables the guard that
/// stops a truncated entity pull being medianed as if it were the whole
/// population. So a `false` becomes an `Err` here and the caller must not
/// build an engine.
pub(crate) fn prepare_cohort_layer(
    layer: &SemanticLayer,
    entity: &str,
) -> Result<SemanticLayer, String> {
    let mut augmented = layer.clone();
    if !augment_layer_for_cohort(&mut augmented, entity) {
        // Surface it. Proceeding here would resolve the cohort with the
        // truncation guard silently inert.
        return Err(augment_failure_reason(layer, entity));
    }
    Ok(augmented)
}

/// Resolve which cohort to use: the explicit request value, else the measure's
/// own `default_cohort:`.
///
/// `default_cohort` is stored as `"entity.cohort_name"`. Comparability varies
/// per measure, not per entity, which is why the fallback lives on the measure
/// — and why a mismatch between the declared entity and the requested one is
/// refused rather than coerced.
///
/// The `Err` here is the ordinary state of a workspace that has not authored
/// `cohorts:` yet, which is why both surfaces answer it as a recognisable
/// `cohort_not_resolved` rather than folding it into a generic failure: the
/// World Model panel renders it as "no peer cohort declared for this measure".
pub(crate) fn resolve_cohort_name(
    layer: &SemanticLayer,
    entity: &str,
    measure: &str,
    explicit: Option<&str>,
) -> Result<String, String> {
    if let Some(c) = explicit {
        // Accept either a bare name or a qualified `entity.cohort`.
        return match c.split_once('.') {
            None => Ok(c.to_string()),
            Some((decl_entity, name)) if decl_entity == entity => Ok(name.to_string()),
            Some((decl_entity, _)) => Err(format!(
                "cohort `{c}` is declared on entity `{decl_entity}`, but the request asks about `{entity}`"
            )),
        };
    }
    let (view_name, measure_name) = measure
        .split_once('.')
        .ok_or_else(|| format!("measure `{measure}` is not fully qualified as `view.measure`"))?;
    let declared = layer
        .views
        .iter()
        .find(|v| v.name == view_name)
        .and_then(|v| v.measures.as_ref())
        .and_then(|ms| ms.iter().find(|m| m.name == measure_name))
        .and_then(|m| m.default_cohort.clone())
        .ok_or_else(|| {
            format!(
                "no cohort given and measure `{measure}` declares no `default_cohort:` — \
                 name one explicitly"
            )
        })?;
    match declared.split_once('.') {
        Some((decl_entity, name)) if decl_entity == entity => Ok(name.to_string()),
        Some((decl_entity, _)) => Err(format!(
            "measure `{measure}` defaults to a cohort on entity `{decl_entity}`, \
             but the request asks about `{entity}`"
        )),
        None => Err(format!(
            "measure `{measure}` declares `default_cohort: {declared}`, which is not \
             in `entity.cohort_name` form"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_layer() -> SemanticLayer {
        SemanticLayer::new(vec![], None)
    }

    /// A layer with one measure carrying `default_cohort:`, built by
    /// deserializing rather than by hand: the struct has ~30 optional fields
    /// and a literal would say nothing about which two matter.
    fn layer_with_default_cohort(default_cohort: &str) -> SemanticLayer {
        serde_json::from_value(serde_json::json!({
            "views": [{
                "name": "sales",
                "measures": [{
                    "name": "wage_pct",
                    "type": "sum",
                    "default_cohort": default_cohort,
                }],
            }],
        }))
        .expect("fixture layer should deserialize")
    }

    #[test]
    fn statistic_accepts_the_three_values_and_defaults_to_median() {
        assert_eq!(parse_statistic(None).unwrap(), BenchmarkStatistic::Median);
        assert_eq!(
            parse_statistic(Some("median")).unwrap(),
            BenchmarkStatistic::Median
        );
        assert_eq!(
            parse_statistic(Some("p75")).unwrap(),
            BenchmarkStatistic::P75
        );
        assert_eq!(
            parse_statistic(Some("best_peer")).unwrap(),
            BenchmarkStatistic::BestPeer
        );
    }

    #[test]
    fn statistic_rejects_anything_else_naming_the_valid_values() {
        let err = parse_statistic(Some("mean")).unwrap_err();
        assert!(
            err.contains("median"),
            "error should name the valid values: {err}"
        );
        assert!(
            err.contains("p75"),
            "error should name the valid values: {err}"
        );
        assert!(
            err.contains("best_peer"),
            "error should name the valid values: {err}"
        );
    }

    /// An explicit cohort qualified with a different entity is refused, not
    /// coerced — comparability is declared per measure, and silently
    /// re-pointing it would benchmark against the wrong population.
    #[test]
    fn an_explicit_cohort_on_another_entity_is_refused() {
        let layer = empty_layer();
        let err = resolve_cohort_name(&layer, "restaurant", "sales.wage_pct", Some("store.sized"))
            .unwrap_err();
        assert!(err.contains("store"), "{err}");
        assert!(err.contains("restaurant"), "{err}");
    }

    /// A bare name is taken as-is; a name qualified with the requested entity
    /// is unqualified. Both are the caller saying which cohort to use.
    #[test]
    fn an_explicit_cohort_is_accepted_bare_or_qualified() {
        let layer = empty_layer();
        assert_eq!(
            resolve_cohort_name(&layer, "restaurant", "sales.wage_pct", Some("sized")).unwrap(),
            "sized"
        );
        assert_eq!(
            resolve_cohort_name(
                &layer,
                "restaurant",
                "sales.wage_pct",
                Some("restaurant.sized")
            )
            .unwrap(),
            "sized"
        );
    }

    /// The fallback the feature leans on: no cohort named, so the measure's
    /// own `default_cohort:` answers — and it comes back unqualified, ready
    /// to hand to `resolve_cohort`.
    #[test]
    fn a_measure_default_cohort_supplies_the_name_when_none_is_given() {
        let layer = layer_with_default_cohort("restaurant.size_matched");
        assert_eq!(
            resolve_cohort_name(&layer, "restaurant", "sales.wage_pct", None).unwrap(),
            "size_matched"
        );
    }

    /// A `default_cohort:` on a different entity than the one asked about is
    /// refused rather than coerced — the same rule as an explicit cohort.
    #[test]
    fn a_measure_default_cohort_on_another_entity_is_refused() {
        let layer = layer_with_default_cohort("store.size_matched");
        let err = resolve_cohort_name(&layer, "restaurant", "sales.wage_pct", None).unwrap_err();
        assert!(err.contains("store"), "{err}");
    }

    /// The ordinary state of a workspace that has not authored `cohorts:`:
    /// no explicit cohort, no `default_cohort:` on the measure. The message
    /// names the measure, because it is shown to the modeller who has to add
    /// the declaration.
    #[test]
    fn an_unauthored_measure_refuses_by_naming_the_measure() {
        let layer = empty_layer();
        let err = resolve_cohort_name(&layer, "restaurant", "sales.wage_pct", None).unwrap_err();
        assert!(err.contains("sales.wage_pct"), "{err}");
        assert!(err.contains("default_cohort"), "{err}");
    }
}
