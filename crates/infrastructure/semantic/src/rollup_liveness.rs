//! The live-rollup set the query path hands to `try_resolve_preagg`.
//!
//! airlayer refuses a rollup that names a measure its view does not declare
//! (airlayer #119) rather than resolving it one measure short, so building the
//! set is fallible. On the query path that refusal must not become a query
//! failure: every read falls back to the warehouse, so the right degradation
//! is to decline every rollup — an EMPTY set, which is the opposite of `None`
//! (`None` skips the liveness check and matches on names alone).
//!
//! Three of the four callers — the analytics catalog, `compile_against`, and
//! the metric-tree `build_query_executor` — derive their views from a
//! `SemanticEngine`, whose construction already ran airlayer's
//! pre-aggregation validation, so the refusal cannot reach here from them.
//! The fourth, `build_drill_query_executor`, computes its set from the shared
//! layer BEFORE any engine exists (one is built per query, inside its
//! closure), so there the decline is reachable — and harmless: that per-query
//! engine build refuses the same layer, so nothing stale is served. This is
//! the one place that decides what happens.

use crate::{View, preagg};

/// `(view, rollup_hash)` for every rollup `views` declares — or an empty set,
/// declining them all, when any rollup refuses to resolve.
///
/// All, not just the offending view's: the set is only trustworthy as a whole,
/// and a partial one is the "shortened, silently" shape #119 removed.
pub fn live_rollups_or_decline(views: &[&View]) -> preagg::LiveRollups {
    preagg::live_rollups(views).unwrap_or_else(|e| {
        tracing::warn!(
            target: "preagg",
            error = %e,
            "a rollup refused to resolve; declining every rollup, so queries answer from the warehouse"
        );
        preagg::LiveRollups::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(yaml: &str) -> View {
        crate::parse_view_yaml(yaml).expect("fixture view parses")
    }

    const ORDERS: &str = r#"
name: orders
datasource: local
table: orders.csv
dimensions:
  - name: status
    type: string
    expr: status
measures:
  - name: total_orders
    type: count
pre_aggregations:
  - name: by_status
    dimensions: [status]
    measures: [total_orders]
"#;

    /// `total_orderz` is not a measure of `returns`. Before #119 airlayer
    /// dropped it and resolved the rollup one measure short, so both views'
    /// rollups reached the live set and a rollup that could never answer for
    /// the measure it was declared for was treated as current.
    const RETURNS_WITH_TYPO: &str = r#"
name: returns
datasource: local
table: returns.csv
dimensions:
  - name: reason
    type: string
    expr: reason
measures:
  - name: total_returns
    type: count
pre_aggregations:
  - name: by_reason
    dimensions: [reason]
    measures: [total_returns, total_orderz]
"#;

    #[test]
    fn a_healthy_layer_declares_every_rollup_live() {
        let orders = view(ORDERS);
        assert_eq!(live_rollups_or_decline(&[&orders]).len(), 1);
    }

    #[test]
    fn an_unresolvable_rollup_measure_declines_every_rollup() {
        let orders = view(ORDERS);
        let returns = view(RETURNS_WITH_TYPO);

        let live = live_rollups_or_decline(&[&orders, &returns]);

        assert!(
            live.is_empty(),
            "one refused rollup must decline the whole set, got {live:?}"
        );
    }
}
