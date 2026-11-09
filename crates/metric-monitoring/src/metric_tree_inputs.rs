//! What the scenario baseline has to read: the set airlayer's
//! `reachable_values_outcome_in` reads, for `grouped_values` to read one view
//! at a time.
//!
//! `predict` recomputes a reached composite from its expression, so it needs
//! the level of EVERY input that expression reads — not only the ones on a
//! lever's path. Forward reachability alone misses two kinds:
//!
//! - **A node beside the path.** `guests * covers` with `guests` pinned reaches
//!   the product, but no edge from `guests` leads to `covers`.
//! - **An input that is no node at all.** A composite can read
//!   `{{stores.net_sales}}`, a measure `sales` declares and `stores` only
//!   inherits through a shared entity. It is queryable, but induced measures
//!   get no tree node, so no edge leads to it; the expression is the only
//!   evidence. And it is read back under a different name: `compile_query`
//!   rewrites it to its source before compiling and patches the requested name
//!   onto the column metadata alone, so the row is keyed `sales__net_sales`.
//!
//! Missing either leaves the composite without a level, and its impact
//! unquantifiable — the bug airlayer PR #125 fixed in its own fetch and
//! oxygen-internal PR #3617 picked up for the simulation probe.
//!
//! The source resolution mirrors airlayer's `cohort::resolve_measure_source`,
//! which is crate-private there, through the public [`Promotions`] it reads.

use std::collections::{HashMap, HashSet};

use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::composite_expr::CompositeExpr;
use oxy_airlayer_compat::engine::metric_tree::{EdgeKind, MetricTree};
use oxy_airlayer_compat::engine::promotions::Promotions;

use super::metric_tree_baseline::forward_reachable;

/// A composite's input that no tree node stands for, asked for by the name the
/// expression reads and read back under the alias the engine projects it as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprInput {
    /// The `view.measure` the expression names — the key its value goes under.
    pub requested: String,
    /// The `view.measure` the engine compiles it to; its alias keys the row.
    pub source: String,
}

impl ExprInput {
    /// The column the executor's row carries this input's value under.
    pub fn alias(&self) -> String {
        self.source.replace('.', "__")
    }
}

/// Everything a baseline over `roots` must read.
#[derive(Debug, Default)]
pub struct BaselineInputs {
    /// Tree nodes: the forward-reachable set, then every other input a reached
    /// node reads. In that order, so the reachable ones come first as before.
    pub nodes: Vec<String>,
    /// Expression refs that are not nodes. Read in a request of their own: a
    /// ref may be a dimension a `type: custom` expression reads, and one of
    /// those in the nodes' batch would fail it for every measure.
    pub extras: Vec<ExprInput>,
}

/// The inputs a baseline over `roots` reads — see the module docs.
///
/// A root is a lever, pinned rather than recomputed, so its own inputs are not
/// needed. Every edge kind but a driver counts as a read: even an input read
/// only by a guard can make the measure above it undefined.
pub fn baseline_inputs(
    tree: &MetricTree,
    layer: &SemanticLayer,
    roots: &[String],
) -> BaselineInputs {
    let mut nodes = forward_reachable(tree, roots);
    let reached: Vec<String> = nodes
        .iter()
        .filter(|id| !roots.contains(id))
        .cloned()
        .collect();
    let mut seen: HashSet<String> = nodes.iter().cloned().collect();

    let mut reads: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in tree.edges.iter().filter(|e| e.kind != EdgeKind::Driver) {
        reads
            .entry(e.to.as_str())
            .or_default()
            .push(e.from.as_str());
    }
    for id in &reached {
        for &input in reads.get(id.as_str()).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(input.to_string()) {
                nodes.push(input.to_string());
            }
        }
    }

    let extras = expression_extras(tree, layer, &reached, &mut seen);
    BaselineInputs { nodes, extras }
}

/// Refs in the reached composites' expressions that are neither tree nodes
/// nor already wanted, resolved to the source the engine compiles them to.
fn expression_extras(
    tree: &MetricTree,
    layer: &SemanticLayer,
    reached: &[String],
    seen: &mut HashSet<String>,
) -> Vec<ExprInput> {
    // A layer whose promotions cannot be built reads every ref as itself —
    // what airlayer does without a layer — rather than reading none.
    let promotions = Promotions::build(&layer.views).ok();
    let parsed = tree
        .nodes
        .iter()
        .filter(|n| n.is_composite && reached.contains(&n.id))
        .filter_map(|n| CompositeExpr::parse(n.expr.as_deref()?).ok());
    let mut extras = Vec::new();
    for expr in parsed {
        for r in expr.refs() {
            if r.starts_with("variables.") || !seen.insert(r.clone()) {
                continue;
            }
            if let Some(source) = measure_source(layer, promotions.as_ref(), r) {
                extras.push(ExprInput {
                    requested: r.clone(),
                    source,
                });
            }
        }
    }
    extras
}

/// The measure `member` compiles to, or `None` when it is an induced name
/// reachable from more than one source view.
///
/// Declared on its own view, or neither declared nor induced (a dimension, a
/// typo): itself, so the engine's own error is what a failure says. Induced
/// from exactly one source: that source. Induced from several: no value
/// rather than a guess — picking one would answer a different question, and
/// the engine refuses the name, so asking for it would only fail the request.
fn measure_source(
    layer: &SemanticLayer,
    promotions: Option<&Promotions>,
    member: &str,
) -> Option<String> {
    let as_requested = Some(member.to_string());
    let (Some(promotions), Some((view, measure))) = (promotions, member.split_once('.')) else {
        return as_requested;
    };
    let declared = layer
        .views
        .iter()
        .find(|v| v.name == view)
        .is_some_and(|v| v.measures_list().iter().any(|m| m.name == measure));
    if declared {
        return as_requested;
    }
    match promotions.candidates(view, measure) {
        [] => as_requested,
        [only] => Some(format!("{}.{}", only.source_view, only.source_measure)),
        _ => None,
    }
}
