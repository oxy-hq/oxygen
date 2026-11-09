//! Asking the baseline's two reads one view at a time.
//!
//! Both reads batch: `reachable_values_outcome` puts every forward-reachable
//! node's measure in one `QueryRequest`, and `fit_driver_coefficients` puts
//! both endpoints of every fittable edge in one panel query. A batch spanning
//! two views is answered the only way SQL can — pick a base view and join the
//! rest in — so a single pair of views with no join path between them refuses
//! the whole request. Every other measure in the batch goes down with it,
//! including measures that are columns of the same row and needed no join at
//! all.
//!
//! Seen in a workspace whose P&L view (`quickbooks_pl`: one primary entity, no
//! foreign entities, monthly closed periods) carries driver edges down to daily
//! Toast aggregates. Pinning a labor lever produced nine "could not be sized
//! from history" refusals carrying an identical message — the signature of one
//! failed query attributed nine times. Exactly one of those edges genuinely
//! crossed the unjoinable pair; seven were single-view, three of them *inside*
//! the P&L.
//!
//! So group first, then batch. A group is one view, and the request it issues
//! names measures from that view alone — which is what makes the grouping safe
//! rather than merely narrower. `metric_tree_baseline` documents why an
//! arbitrary split is dangerous: a half that keeps the other view's time
//! dimension joins on a partial key and returns a whole-window total repeated
//! on every date, turning a loud refusal into a silent wrong number. A
//! single-view request cannot do that — there is no join to fan out — and the
//! time dimension is re-resolved against the group's own view rather than
//! carried across (see [`time_dimension_for`]).
//!
//! The cross-view remainder batches per view PAIR — the smallest unit a join
//! either supports or does not, and so the largest batch whose failure
//! implicates only the views named in it. A workspace where a cross-view fit
//! works today keeps it, since a pair asks for a subset of the joins the whole
//! remainder asked for; what changes is that one unjoinable pair no longer
//! takes down the pairs that join fine beside it. A pair owning neither end of
//! the window is refused for the window rather than queried, because the
//! anchor's view would otherwise join in as a third.
//!
//! Fitting edges that used to die in the batch exposed a second problem — a
//! per-row slope aggregates into a change in the target's SUM, which is not
//! what the target's window value means when the target is an average. That is
//! fixed where it belongs, in airlayer's `AggregateSpace` (PR #90): an identity
//! link converts into the target's space and a ratio refuses by name. This
//! module briefly carried a blanket refusal for it and no longer needs to.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree::{MetricEdge, MetricTree};
use oxy_airlayer_compat::engine::metric_tree_fit::{
    FittedDriver, fit_driver_coefficients, fit_panel_dimensions, fittable_edges,
};
use oxy_airlayer_compat::engine::metric_tree_ops::{BaselineOutcome, QueryExecutor};
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter, QueryRequest};
use oxy_airlayer_compat::schema::models::AggregateSpace;

use super::metric_tree_inputs::{ExprInput, baseline_inputs};

type Row = serde_json::Map<String, serde_json::Value>;

/// The view half of a `view.measure` node id.
pub(crate) fn view_of(node_id: &str) -> &str {
    node_id.split('.').next().unwrap_or(node_id)
}

/// What `node_id`'s value over a window is, for airlayer's response arithmetic.
///
/// Resolved by `MetricTree::build`, which is the only thing that can: a
/// composite's space follows its expression, not its measure type. An id with
/// no node refuses rather than defaulting to `Total` — the permissive answer is
/// the one that produces a number.
pub fn space_of(tree: &MetricTree, node_id: &str) -> AggregateSpace {
    tree.nodes
        .iter()
        .find(|n| n.id == node_id)
        .map(|n| n.aggregate_space)
        .unwrap_or(AggregateSpace::Unaggregatable)
}

/// How a view came to have no values.
///
/// The single [`BaselineOutcome`] the response carries cannot say this: it
/// describes the read, and a view left out of the read entirely is not a state
/// it has a word for. So the distinction travels per view instead. Getting it
/// wrong is the failure `classify_unvalued` already guards — telling someone to
/// lengthen a window that was never applied to their measure in the first
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipKind {
    /// No query was issued for this view at all.
    NotQueried,
    /// A query was issued and the executor returned an error.
    QueryFailed,
}

/// A view the values read produced nothing for, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkippedGroup {
    pub view: String,
    pub nodes: Vec<String>,
    pub reason: String,
    pub kind: SkipKind,
}

/// What one grouped values read produced.
pub(crate) struct GroupedValues {
    pub values: HashMap<String, f64>,
    pub outcome: BaselineOutcome,
    pub skipped: Vec<SkippedGroup>,
    /// Expression-only reads (see [`baseline_inputs`]) the executor refused.
    /// No note speaks for them — the composite above going unvalued already
    /// does — but a read missing an input it could have had must not be
    /// remembered as complete, so the caching caller checks this.
    pub input_errors: Vec<String>,
}

/// The time dimension to read `view` over, given the one the request named.
///
/// A metric tree spanning two views is queried under a single time dimension,
/// which necessarily belongs to one of them. Only that view is read over it.
/// Every other group refuses, because there is nothing to resolve the
/// dimension *against*.
///
/// This used to match by local name — `labor_daily.business_date` became
/// `sales_daily.business_date` wherever `sales_daily` had a date dimension so
/// named. Two dimensions sharing a name are not thereby the same calendar, and
/// the schema has no way to say that they are: [`Dimension`] carries `name`,
/// `type`, `expr`, `synonyms`, and nothing that declares conformance with a
/// dimension on another view. So the match was a guess, and it was checked in
/// the one direction that could not hurt — a *missing* name refused loudly,
/// while a *matching* name was trusted in silence. The refusal's own stated
/// reason (that picking a dimension would "silently re-anchor the window to a
/// different calendar") is a property of the calendar, not of the name. It
/// applies just as much to a `business_date` that is a different clock, a
/// different grain, or a `datetime` where the request meant a local business
/// `date`. That is the wrong-number failure this module exists to refuse, so
/// it is refused here too.
///
/// The cost is real and deliberate: a tree spanning views gets a baseline, and
/// driver fits, only for the view the window is anchored on — including for
/// edges that live entirely inside one of the others. Reaching those needs a
/// declaration, not a cleverer heuristic: a conformed-dimension identity in
/// the view schema, asserted by someone who knows the warehouse, which this
/// function could then check and honour. Until the schema can carry one, an
/// unread view and a note saying so is the honest answer.
///
/// [`Dimension`]: oxy_airlayer_compat::schema::models::Dimension
fn time_dimension_for(
    layer: &SemanticLayer,
    view: &str,
    requested: &str,
) -> Result<String, String> {
    if view_of(requested) == view {
        return Ok(requested.to_string());
    }
    if !layer.views.iter().any(|v| v.name == view) {
        return Err(format!("no view named `{view}` in the semantic layer"));
    }
    // Deliberately just the fact. The reasoning above is why this refuses;
    // reading it is not what an analyst needs from a panel, and every view a
    // tree cannot window is refused for this same reason — so a sentence
    // arguing the case gets repeated verbatim once per view. `baseline_note`
    // groups on this string, which only works while it names no view.
    Err(format!("no `{requested}` to anchor the window on"))
}

/// The entity `member` is a key column of, and that column's position in the
/// entity's key list.
///
/// Position, not name: a composite key arrives as one filter per column (see
/// `build_pk_filters`), and two views need not spell the columns alike.
/// Matching by position pairs `orders.(order_id, line_id)` with a child's
/// `(o_id, l_id)` correctly, where matching by name would pair nothing.
fn entity_key_position<'a>(layer: &'a SemanticLayer, member: &str) -> Option<(&'a str, usize)> {
    let (view_name, column) = member.split_once('.')?;
    let view = layer.views.iter().find(|v| v.name == view_name)?;
    view.entities.iter().find_map(|e| {
        e.get_keys()
            .iter()
            .position(|k| k == column)
            .map(|i| (e.name.as_str(), i))
    })
}

/// The column on `view` holding the same entity key, if the view declares one.
fn entity_key_on_view(
    layer: &SemanticLayer,
    view: &str,
    entity: &str,
    position: usize,
) -> Option<String> {
    let view = layer.views.iter().find(|v| v.name == view)?;
    let column = view
        .entities
        .iter()
        .find(|e| e.name == entity)?
        .get_keys()
        .get(position)
        .cloned()?;
    // The key must be a dimension the view actually exposes. An entity naming
    // a column the view never declares would build a filter the engine cannot
    // resolve — a query failure wearing a scope's clothes.
    view.dimensions
        .iter()
        .any(|d| d.name == column)
        .then_some(column)
}

/// Re-express `scope` against `views`, or `None` when it cannot be.
///
/// `instance_scope_filters` always builds an instance scope on the entity's
/// PRIMARY view — `restaurants.restaurant_id` — because that is the view whose
/// join path to the measure airlayer resolves. A group read one view at a time
/// never gets that join, so a fact view holding the same entity as a FOREIGN
/// key was refused for naming "another view" although it carries the very
/// column the scope means. Every per-store baseline on a fact view refused
/// that way, which is every measure in a star schema.
///
/// This is a rewrite, not the name-matching guess [`time_dimension_for`]
/// refuses to make. Two dimensions sharing a name are not thereby the same
/// thing and the schema cannot say that they are; an `entities:` block is
/// exactly that missing declaration — the author asserting this column holds
/// keys of that entity — so honouring it reads the schema rather than the
/// spelling.
///
/// Rewritten only when EVERY view in `views` declares the entity. For a single
/// view that is the view itself; for a pair it means the join between them
/// preserves the entity, so scoping either side scopes both. A pair with one
/// view that cannot carry the entity at all is still refused — filtering only
/// the half that can would answer an instance-scoped request with a
/// population-wide number on the other half, the wrong-number failure this
/// module exists to prevent.
///
/// Leaves already naming a view in `views` pass through untouched, so every
/// scope honoured before is honoured unchanged.
fn scope_onto(
    layer: &SemanticLayer,
    views: &BTreeSet<&str>,
    scope: &[QueryFilter],
) -> Option<Vec<QueryFilter>> {
    scope
        .iter()
        .map(|f| rewrite_filter(layer, views, f))
        .collect()
}

/// Rewrite one filter, recursing into `and:` / `or:` groups.
///
/// Per LEAF, not per disjunct. Deciding a whole filter against one view at a
/// time — `on_view(f, a) || on_view(f, b)` — looks equivalent for a pair and is
/// not: an `or:` group with one leaf on `a` and one on `b` fails both disjuncts
/// while naming nothing outside the pair, so it would be refused although the
/// pair expresses it exactly. Each leaf is resolved on its own and the group is
/// kept whole.
fn rewrite_filter(
    layer: &SemanticLayer,
    views: &BTreeSet<&str>,
    filter: &QueryFilter,
) -> Option<QueryFilter> {
    let member = match &filter.member {
        None => None,
        Some(m) if views.contains(view_of(m)) => Some(m.clone()),
        Some(m) => {
            let (entity, position) = entity_key_position(layer, m)?;
            // Resolve on every view before taking one: the whole set has to
            // carry the entity for the rewrite to be sound, and which member
            // is then named is immaterial — the join equates them.
            let columns: Option<Vec<String>> = views
                .iter()
                .map(|v| entity_key_on_view(layer, v, entity, position).map(|c| format!("{v}.{c}")))
                .collect();
            Some(columns?.into_iter().next()?)
        }
    };
    let group = |g: &Option<Vec<QueryFilter>>| -> Option<Option<Vec<QueryFilter>>> {
        match g {
            None => Some(None),
            Some(subs) => subs
                .iter()
                .map(|s| rewrite_filter(layer, views, s))
                .collect::<Option<Vec<_>>>()
                .map(Some),
        }
    };
    Some(QueryFilter {
        member,
        operator: filter.operator.clone(),
        values: filter.values.clone(),
        and: group(&filter.and)?,
        or: group(&filter.or)?,
    })
}

/// Why a scope naming a member outside the group is refused rather than dropped.
///
/// Shared verbatim by the single-view and pair branches because `baseline_note`
/// groups skips on the reason string by exact match: a paraphrase splits one
/// group into two and prints the same refusal twice under different words.
const SCOPE_NEEDS_A_JOIN: &str = "the scope names a member from another view, which would need a join this \
     one has no path for";

/// The time dimension for `view` and the scope to read it under, or why it
/// cannot be read at all.
///
/// The scope is all-or-nothing on purpose. A scope naming a member the group's
/// view does not carry, and cannot be re-expressed onto (see [`scope_onto`]),
/// could only be honoured by joining the view that does — the join being
/// avoided — and dropping it instead would return a population-wide number
/// under a request that named one instance. That is the wrong-number failure
/// `baseline_scope_core` already refuses to make; refuse the group instead.
fn group_plan(
    layer: &SemanticLayer,
    view: &str,
    time_dimension: &str,
    scope: &[QueryFilter],
) -> Result<(String, Vec<QueryFilter>), String> {
    let time_dim = time_dimension_for(layer, view, time_dimension)?;
    let scope = scope_onto(layer, &BTreeSet::from([view]), scope)
        .ok_or_else(|| SCOPE_NEEDS_A_JOIN.to_string())?;
    Ok((time_dim, scope))
}

/// The window filters `reachable_values_outcome` builds, against `time_dim`.
fn window_filters(time_dim: &str, period: (&str, &str), scope: &[QueryFilter]) -> Vec<QueryFilter> {
    let bound = |op: FilterOperator, value: &str| QueryFilter {
        member: Some(time_dim.to_string()),
        operator: Some(op),
        values: vec![value.to_string()],
        and: None,
        or: None,
    };
    let mut filters = vec![
        bound(FilterOperator::AfterOrOnDate, period.0),
        bound(FilterOperator::BeforeOrOnDate, period.1),
    ];
    filters.extend_from_slice(scope);
    filters
}

/// A measure value out of a returned row, in either shape a warehouse sends.
fn measure_value(row: &Row, alias: &str) -> Option<f64> {
    match row.get(alias)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Current value of every input a baseline reads, one request per view.
///
/// Mirrors `reachable_values_outcome_in`'s contract — same input set (see
/// [`baseline_inputs`]), same aliasing, same [`BaselineOutcome`] vocabulary —
/// but issues one request per view instead of one for the tree. These are
/// independent scalars over one window: nothing about the question needs them
/// to meet in a row.
///
/// A view's expression-only inputs go in a second request of their own, as in
/// airlayer, so a ref the engine cannot answer leaves only itself unvalued.
/// They are filed under the view the expression names, whose window and scope
/// they are read over, and they stay out of the outcome and the skip list:
/// both speak about tree nodes, and an unread input already shows up as the
/// composite above it going unvalued.
pub(crate) fn grouped_values(
    tree: &MetricTree,
    layer: &SemanticLayer,
    roots: &[String],
    time_dimension: &str,
    period: (&str, &str),
    scope: &[QueryFilter],
    executor: &QueryExecutor,
) -> GroupedValues {
    let inputs = baseline_inputs(tree, layer, roots);
    let mut by_view: BTreeMap<&str, (Vec<String>, Vec<&ExprInput>)> = BTreeMap::new();
    for id in &inputs.nodes {
        by_view.entry(view_of(id)).or_default().0.push(id.clone());
    }
    for input in &inputs.extras {
        by_view
            .entry(view_of(&input.requested))
            .or_default()
            .1
            .push(input);
    }

    let mut reads = Reads::default();
    for (view, (nodes, extras)) in by_view {
        let (time_dim, group_scope) = match group_plan(layer, view, time_dimension, scope) {
            Ok(plan) => plan,
            Err(reason) => {
                if !nodes.is_empty() {
                    reads.skipped.push(SkippedGroup {
                        view: view.to_string(),
                        nodes,
                        reason,
                        kind: SkipKind::NotQueried,
                    });
                }
                continue;
            }
        };
        let filters = window_filters(&time_dim, period, &group_scope);
        if !nodes.is_empty() {
            reads.read_nodes(view, nodes, &filters, executor);
        }
        reads.read_extras(view, &extras, &filters, executor);
    }

    let outcome = values_outcome(
        &reads.values,
        reads.ran,
        reads.any_rows,
        &reads.errors,
        inputs.nodes.is_empty(),
    );
    GroupedValues {
        values: reads.values,
        outcome,
        skipped: reads.skipped,
        input_errors: reads.input_errors,
    }
}

/// What `grouped_values` has read so far, across views.
#[derive(Default)]
struct Reads {
    values: HashMap<String, f64>,
    skipped: Vec<SkippedGroup>,
    errors: Vec<String>,
    input_errors: Vec<String>,
    ran: usize,
    any_rows: bool,
}

impl Reads {
    /// One view's tree nodes, in one request.
    fn read_nodes(
        &mut self,
        view: &str,
        nodes: Vec<String>,
        filters: &[QueryFilter],
        executor: &QueryExecutor,
    ) {
        let query = QueryRequest {
            measures: nodes.clone(),
            filters: filters.to_vec(),
            ..QueryRequest::new()
        };
        self.ran += 1;
        match executor(&query) {
            Err(e) => {
                self.errors.push(format!("`{view}`: {e}"));
                self.skipped.push(SkippedGroup {
                    view: view.to_string(),
                    nodes,
                    reason: e.to_string(),
                    kind: SkipKind::QueryFailed,
                });
            }
            Ok(rows) => {
                let Some(row) = rows.first() else { return };
                self.any_rows = true;
                for id in nodes {
                    let alias = id.replace('.', "__");
                    if let Some(v) = measure_value(row, &alias) {
                        self.values.insert(id, v);
                    }
                }
            }
        }
    }

    /// One view's expression-only inputs, asked for by the name the expression
    /// reads and read back under their source alias. A failure leaves these
    /// inputs, and nothing else, without a value, and adds no note; it is
    /// logged and recorded in `input_errors` so the read is not cached.
    fn read_extras(
        &mut self,
        view: &str,
        extras: &[&ExprInput],
        filters: &[QueryFilter],
        executor: &QueryExecutor,
    ) {
        if extras.is_empty() {
            return;
        }
        let names: Vec<String> = extras.iter().map(|m| m.requested.clone()).collect();
        let query = QueryRequest {
            measures: names.clone(),
            filters: filters.to_vec(),
            ..QueryRequest::new()
        };
        let rows = match executor(&query) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    view,
                    inputs = ?names,
                    error = %e,
                    "metric-tree baseline: expression-only input read failed"
                );
                self.input_errors.push(format!("`{view}` {names:?}: {e}"));
                return;
            }
        };
        let Some(row) = rows.first() else { return };
        for input in extras {
            if let Some(v) = measure_value(row, &input.alias()) {
                self.values.insert(input.requested.clone(), v);
            }
        }
    }
}

/// Fold the per-group results into the one outcome the response carries.
///
/// `NothingRequested` covers the all-skipped case as well as the empty tree:
/// literally no query was issued, and the *reasons* travel on
/// [`SkippedGroup`], where they say something more useful than any of the
/// engine's four outcomes could.
fn values_outcome(
    values: &HashMap<String, f64>,
    ran: usize,
    any_rows: bool,
    errors: &[String],
    nothing_wanted: bool,
) -> BaselineOutcome {
    if nothing_wanted || ran == 0 {
        return BaselineOutcome::NothingRequested;
    }
    if !values.is_empty() {
        // Empty `unreadable`: this path merges per-group reads and never sees
        // a raw cell, so it cannot name a measure whose column held something
        // unreadable — the group's own read already turned that into either a
        // value or an absence. Claiming a name we do not have would be worse
        // than the silence.
        return BaselineOutcome::Valued {
            unreadable: Vec::new(),
        };
    }
    if !errors.is_empty() {
        return BaselineOutcome::ExecutorError(errors.join("; "));
    }
    if any_rows {
        BaselineOutcome::NoMatchingColumns
    } else {
        BaselineOutcome::NoRows
    }
}

/// `tree` reduced to `edges`, with the roots that make exactly those edges
/// fittable.
///
/// `fit_driver_coefficients` derives its own candidate set from a tree and
/// roots, so a subset is expressed by handing it a subtree. Every edge's
/// `from` is a root, so `fittable_edges` returns the group and nothing else —
/// the edges kept their `kind` and their absent `coefficient`, which are the
/// only other things it filters on.
fn subtree(tree: &MetricTree, edges: &[&MetricEdge]) -> (MetricTree, Vec<String>) {
    let mut sub = tree.clone();
    sub.edges = edges.iter().map(|e| (*e).clone()).collect();
    let roots = edges.iter().map(|e| e.from.clone()).collect();
    (sub, roots)
}

/// Whether a refusal is airlayer saying it could not connect the views.
fn is_join_refusal(refusal: &str) -> bool {
    refusal.contains("No valid join tree found") || refusal.contains("No join path found")
}

/// Say what a join-tree refusal means in the terms the author can act on.
///
/// Airlayer reports it as `No valid join tree found; using 'x' as base view`,
/// which names an implementation step (base-view selection) and a view that is
/// not actually used — the fallback is computed to fill the message and then
/// discarded. What the reader needs is that these two measures cannot be
/// paired from history at all, and that declaring the magnitude is the way
/// past it.
///
/// Rewritten on the returned refusal rather than on the executor's error
/// because `QueryExecutor` is a `'static` trait object: a wrapper closure
/// borrowing the view names could not be handed to airlayer at all.
fn unforecastable_note(views: &BTreeSet<&str>) -> String {
    let named = views
        .iter()
        .map(|v| format!("`{v}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "no join path across {named} — the measures share no key or grain, so history \
         cannot pair them and this edge is not forecastable. Declare a `coefficient:` on \
         the driver entry to state the magnitude directly."
    )
}

/// Fit one group of edges, or refuse them all with `reason` without querying.
///
/// The refusal goes through an executor that fails immediately rather than
/// through a hand-built [`FittedDriver`], so a refused edge is shaped by the
/// same code that shapes every other one — there is no second definition of
/// what a refusal looks like to drift from airlayer's.
fn fit_group(
    tree: &MetricTree,
    layer: &SemanticLayer,
    edges: &[&MetricEdge],
    plan: Result<(String, Vec<QueryFilter>), String>,
    period: (&str, &str),
    executor: &QueryExecutor,
) -> Vec<FittedDriver> {
    let (sub, roots) = subtree(tree, edges);
    let panel_dimensions = fit_panel_dimensions(layer, edges);
    let (time_dim, scope, refusal) = match plan {
        Ok((d, s)) => (d, s, None),
        // Neither the time dimension nor the scope is read on this path — the
        // executor below refuses before a request is built — but both have to
        // be passed.
        Err(reason) => (String::new(), Vec::new(), Some(reason)),
    };
    let refuse = refusal.map(|r| move |_: &QueryRequest| Err(EngineError::QueryError(r.clone())));
    let run = |exec: &QueryExecutor| {
        fit_driver_coefficients(
            &sub,
            &roots,
            &panel_dimensions,
            &time_dim,
            period,
            &scope,
            exec,
        )
        .unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                edges = edges.len(),
                "metric-tree group fit failed outright; its edges degrade to qualitative"
            );
            Vec::new()
        })
    };
    match &refuse {
        Some(r) => run(r),
        None => run(executor),
    }
}

/// Coefficients for every fittable edge, one panel query per single-view group
/// plus one per cross-view PAIR.
pub fn grouped_fit(
    tree: &MetricTree,
    layer: &SemanticLayer,
    roots: &[String],
    time_dimension: &str,
    period: (&str, &str),
    scope: &[QueryFilter],
    executor: &QueryExecutor,
) -> Vec<FittedDriver> {
    let candidates = fittable_edges(tree, roots);
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut by_view: BTreeMap<&str, Vec<&MetricEdge>> = BTreeMap::new();
    let mut cross: Vec<&MetricEdge> = Vec::new();
    for edge in &candidates {
        let (from, to) = (view_of(&edge.from), view_of(&edge.to));
        if from == to {
            by_view.entry(from).or_default().push(edge);
        } else {
            cross.push(edge);
        }
    }

    let mut fits: Vec<FittedDriver> = Vec::with_capacity(candidates.len());
    for (view, edges) in by_view {
        let plan = group_plan(layer, view, time_dimension, scope);
        fits.extend(fit_group(tree, layer, &edges, plan, period, executor));
    }
    // One batch PER VIEW PAIR, not one batch for the whole remainder.
    //
    // A pair is the smallest unit a join either supports or does not, so it is
    // the largest batch whose failure implicates only the views named in it.
    // Batched together instead, one island took down every cross-view edge:
    // `sales_daily -> daily_operations` joins on `restaurant_id` at a shared
    // daily grain and fits, but riding in with `sales_daily -> quickbooks_pl`
    // — a view with no foreign entity at all — it was refused as "not
    // forecastable" and its author told to declare a `coefficient:` for a
    // magnitude history could have measured. The note was wrong twice over: it
    // named three views as sharing "no key or grain" when two of them share
    // both, and it prescribed a fix for the wrong edge.
    //
    // Splitting cannot lose a fit that worked before: a pair's batch asks for
    // a subset of the joins the whole remainder asked for, so a join that held
    // across the batch still holds across the pair.
    let mut by_pair: BTreeMap<(&str, &str), Vec<&MetricEdge>> = BTreeMap::new();
    for edge in &cross {
        let (a, b) = (view_of(&edge.from), view_of(&edge.to));
        by_pair
            .entry(if a <= b { (a, b) } else { (b, a) })
            .or_default()
            .push(edge);
    }
    for ((a, b), edges) in by_pair {
        let views: BTreeSet<&str> = [a, b].into_iter().collect();
        // A pair owning neither end of the window is not a two-view request.
        // The anchor's own view arrives as a THIRD, carried in by the panel
        // dimension and both window filters, and the join it needs is one no
        // edge in the pair asked for. When that join is what fails, the
        // refusal below rewrites it against the pair — the misattribution
        // this grouping exists to prevent, reintroduced one view further out.
        //
        // Refused in `group_plan`'s words rather than as a join failure: the
        // pair may well join, and `baseline_note` groups on this string, so a
        // pair unreachable by the window is named alongside every other view
        // the window cannot reach instead of accusing the pair of sharing no
        // key.
        //
        // Scope is checked after the window, as `group_plan` checks it after
        // `time_dimension_for`, and for the same reason it refuses rather than
        // drops: `instance_scope_filters` builds the scope on whichever view is
        // primary for the pinned entity, which has no relation to either view
        // here, so a pair not containing it would join it in as a third exactly
        // as the window did. Dropping the filter instead would answer an
        // instance-scoped request with a population-wide number.
        //
        // [`scope_onto`] can re-express it onto the pair, but only when BOTH
        // views declare the entity — a pair where one view cannot carry it is
        // still refused here, for the reason just given.
        let plan = time_dimension_for(layer, a, time_dimension)
            .or_else(|_| time_dimension_for(layer, b, time_dimension))
            .and_then(|time_dim| {
                scope_onto(layer, &views, scope)
                    .map(|scope| (time_dim, scope))
                    .ok_or_else(|| SCOPE_NEEDS_A_JOIN.to_string())
            });
        let mut cross_fits = fit_group(tree, layer, &edges, plan, period, executor);
        for fit in &mut cross_fits {
            if fit.refusal.as_deref().is_some_and(is_join_refusal) {
                fit.refusal = Some(unforecastable_note(&views));
            }
        }
        fits.extend(cross_fits);
    }
    // Airlayer sorts its fits this way; the groups arrive in view order, so
    // without this the response order would depend on the grouping.
    fits.sort_by(|a, b| (&a.to, &a.from).cmp(&(&b.to, &b.from)));
    fits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Airlayer's base-view refusal, verbatim from `sql_generator` — the one a
    /// view with no foreign entity provokes the moment it shares a request.
    /// Copied rather than paraphrased because `is_join_refusal` matches on the
    /// text; an invented fixture would let the matcher drift from the engine
    /// without a test noticing.
    const NO_JOIN_TREE: &str = "No valid join tree found; using 'labor_daily' as base view";

    const WINDOW: (&str, &str) = ("2026-01-01", "2026-02-19");

    /// The real-workspace shape that motivated this module, minimised: a daily view
    /// with a `restaurant` foreign entity, and a second view that declares
    /// none — an island in the join graph — carrying its own internal driver
    /// chain plus one edge bridging into it.
    ///
    /// `ops_summary` deliberately carries a `business_date` of its own — same
    /// name, same type, and nothing declaring it the same calendar. That is
    /// the near-miss the old name-matching resolver read anyway, so the
    /// fixture carries both the join island and the tempting coincidence.
    fn island_layer() -> SemanticLayer {
        let labor_daily = r#"
name: labor_daily
table: public.labor_daily
dialect: postgres
entities:
  - name: labor_day
    type: primary
    key: labor_day_key
  - name: restaurant
    type: foreign
    key: restaurant_id
dimensions:
  - name: labor_day_key
    type: string
    expr: labor_day_key
  - name: restaurant_id
    type: string
    expr: restaurant_id
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: total_regular_hours
    type: sum
    expr: regular_hours
  - name: total_labor_hours
    type: sum
    expr: labor_hours
    drivers:
      - measure: labor_daily.total_regular_hours
        direction: positive
  - name: total_labor_cost
    type: sum
    expr: labor_cost
    drivers:
      - measure: labor_daily.total_labor_hours
        direction: positive
"#;
        let ops_summary = r#"
name: ops_summary
table: public.ops_summary
dialect: postgres
entities:
  - name: ops_line
    type: primary
    key: ops_line_key
dimensions:
  - name: ops_line_key
    type: string
    expr: ops_line_key
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: labor_cost
    type: sum
    expr: labor_cost
    drivers:
      - measure: labor_daily.total_labor_cost
        direction: positive
  - name: total_operating_expenses
    type: sum
    expr: opex
    drivers:
      - measure: ops_summary.labor_cost
        direction: positive
"#;
        SemanticLayer::new(
            vec![
                oxy_airlayer_compat::parse_view_yaml(labor_daily).unwrap(),
                oxy_airlayer_compat::parse_view_yaml(ops_summary).unwrap(),
            ],
            None,
        )
    }

    fn tree(layer: &SemanticLayer) -> MetricTree {
        oxy_semantic::build_metric_tree(layer)
    }

    fn roots() -> Vec<String> {
        vec!["labor_daily.total_regular_hours".to_string()]
    }

    /// Every view named anywhere in a request — measures, dimensions and
    /// filter members alike, since any of the three pulls its view in.
    fn views_touched(req: &QueryRequest) -> BTreeSet<String> {
        let mut views: BTreeSet<String> = BTreeSet::new();
        for m in req.measures.iter().chain(req.dimensions.iter()) {
            views.insert(view_of(m).to_string());
        }
        for f in &req.filters {
            if let Some(member) = &f.member {
                views.insert(view_of(member).to_string());
            }
        }
        views
    }

    /// Refuses any request naming two views, exactly as a join graph with an
    /// island does, and otherwise answers. Records every request it was given.
    ///
    /// Panel rows are linear in the day ordinal with a per-measure wobble, so
    /// a fit that runs has both a slope to find and a non-zero residual — a
    /// perfectly collinear fixture divides by a zero standard error and
    /// refuses for a reason that has nothing to do with what is under test.
    fn island_executor(seen: Arc<Mutex<Vec<QueryRequest>>>) -> Box<QueryExecutor> {
        Box::new(move |req: &QueryRequest| {
            seen.lock().unwrap().push(req.clone());
            if views_touched(req).len() > 1 {
                return Err(EngineError::QueryError(NO_JOIN_TREE.to_string()));
            }
            let value = |k: usize, day: usize| {
                (k as f64 + 1.0) * (day as f64 + 1.0) + ((day * (k + 3)) % 7) as f64 * 0.05
            };
            if req.dimensions.is_empty() {
                let mut row = Row::new();
                for (k, m) in req.measures.iter().enumerate() {
                    row.insert(m.replace('.', "__"), serde_json::json!(value(k, 1)));
                }
                return Ok(vec![row]);
            }
            let rows = (0..40)
                .map(|day| {
                    let mut row = Row::new();
                    for dim in &req.dimensions {
                        let alias = dim.replace('.', "__");
                        let v = if dim.ends_with("_date") {
                            serde_json::json!(format!("2026-01-{:02}", day + 1))
                        } else {
                            serde_json::json!("r1")
                        };
                        row.insert(alias, v);
                    }
                    for (k, m) in req.measures.iter().enumerate() {
                        row.insert(m.replace('.', "__"), serde_json::json!(value(k, day)));
                    }
                    row
                })
                .collect();
            Ok(rows)
        })
    }

    fn refusal_of<'f>(fits: &'f [FittedDriver], from: &str, to: &str) -> Option<&'f str> {
        let fit = fits
            .iter()
            .find(|f| f.from == from && f.to == to)
            .unwrap_or_else(|| panic!("no fit for {from} -> {to} in {fits:?}"));
        fit.refusal.as_deref()
    }

    #[test]
    fn a_cross_view_edge_failing_does_not_reach_the_anchor_view_s_own_edges() {
        let layer = island_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        // The bridge is the one edge that genuinely cannot be paired, and it
        // says so in the author's terms rather than airlayer's.
        let bridge = refusal_of(
            &fits,
            "labor_daily.total_labor_cost",
            "ops_summary.labor_cost",
        )
        .expect("the cross-view edge cannot be fitted");
        assert!(bridge.contains("not forecastable"), "{bridge}");
        assert!(
            bridge.contains("`labor_daily` and `ops_summary`"),
            "{bridge}"
        );
        assert!(bridge.contains("coefficient:"), "{bridge}");
        assert!(!bridge.contains("join tree"), "{bridge}");

        // The anchor view's own edges survive it — the case a batched query
        // took down for a join they never asked for.
        for (from, to) in [
            (
                "labor_daily.total_regular_hours",
                "labor_daily.total_labor_hours",
            ),
            (
                "labor_daily.total_labor_hours",
                "labor_daily.total_labor_cost",
            ),
        ] {
            assert_eq!(
                refusal_of(&fits, from, to),
                None,
                "{from} -> {to} should have been fitted on its own view"
            );
        }

        // The island's own edge is refused too, and this is the deliberate
        // cost of not guessing a calendar: it needs no join and crosses
        // nothing, but the window cannot be expressed on `ops_summary` at all.
        // It must not read as the join refusal above — that would send someone
        // to declare a `coefficient:` on an edge whose problem is the window.
        let inside = refusal_of(
            &fits,
            "ops_summary.labor_cost",
            "ops_summary.total_operating_expenses",
        )
        .expect("the island cannot be read over a window it does not carry");
        assert!(inside.contains("anchor the window on"), "{inside}");
        assert!(!inside.contains("not forecastable"), "{inside}");

        // The guarantee that makes the grouping safe rather than merely
        // narrower: no request the fit issued named two views except the
        // cross-view batch, which is the only one entitled to.
        let requests = seen.lock().unwrap().clone();
        let multi = requests
            .iter()
            .filter(|r| views_touched(r).len() > 1)
            .count();
        assert_eq!(multi, 1, "{requests:?}");
    }

    #[test]
    fn values_are_read_one_view_at_a_time() {
        let layer = island_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        assert!(matches!(read.outcome, BaselineOutcome::Valued { .. }));
        for id in [
            "labor_daily.total_regular_hours",
            "labor_daily.total_labor_cost",
        ] {
            assert!(
                read.values.contains_key(id),
                "{id} unvalued: {read:?}",
                read = read.values
            );
        }
        // The island carrying a `business_date` of its own is not enough — see
        // `time_dimension_for`. One request, naming one view, and the island
        // reported as skipped rather than as an empty window.
        assert_eq!(read.skipped.len(), 1, "{:?}", read.skipped);
        assert_eq!(read.skipped[0].view, "ops_summary");
        assert_eq!(read.skipped[0].kind, SkipKind::NotQueried);
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(views_touched(&requests[0]).len(), 1, "{requests:?}");
    }

    #[test]
    fn a_view_that_cannot_express_the_window_is_skipped_not_guessed() {
        let layer = island_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        // The labor side still answers, so the outcome is `Valued` — which is
        // exactly why the skip has to travel separately: the outcome alone
        // would leave the island's nodes indistinguishable from "no rows".
        assert!(matches!(read.outcome, BaselineOutcome::Valued { .. }));
        assert!(read.values.contains_key("labor_daily.total_labor_cost"));
        assert_eq!(read.skipped.len(), 1, "{:?}", read.skipped);
        let skipped = &read.skipped[0];
        assert_eq!(skipped.view, "ops_summary");
        assert!(skipped.reason.contains("business_date"), "{skipped:?}");
        assert!(
            skipped
                .nodes
                .contains(&"ops_summary.labor_cost".to_string())
        );
        // Nothing of the island's is ever substituted — not a dimension by
        // another name, and not the `business_date` it does carry. Either
        // would re-anchor the window to an unverified calendar in silence.
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(
            !requests[0]
                .measures
                .iter()
                .any(|m| view_of(m) == "ops_summary")
        );
    }

    #[test]
    fn a_window_a_view_cannot_express_refuses_its_fits_without_querying() {
        let layer = island_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        let refusal = refusal_of(
            &fits,
            "ops_summary.labor_cost",
            "ops_summary.total_operating_expenses",
        )
        .expect("the island cannot be read over a window it has no dimension for");
        assert!(refusal.contains("anchor the window on"), "{refusal}");
        // The labor group is untouched by its neighbour's problem.
        assert_eq!(
            refusal_of(
                &fits,
                "labor_daily.total_regular_hours",
                "labor_daily.total_labor_hours"
            ),
            None
        );
        // Refused before a request was built, not by spending a round trip.
        let requests = seen.lock().unwrap().clone();
        assert!(
            !requests
                .iter()
                .any(|r| r.measures.iter().all(|m| view_of(m) == "ops_summary")),
            "{requests:?}"
        );
    }

    #[test]
    fn a_scope_the_view_cannot_carry_refuses_the_group_rather_than_dropping() {
        let layer = island_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        // Scoped to a member of the *other* view, so the group that owns the
        // window is the one that cannot carry the filter.
        let scope = vec![QueryFilter {
            member: Some("ops_summary.ops_line_key".to_string()),
            operator: Some(FilterOperator::Equals),
            values: vec!["l1".to_string()],
            and: None,
            or: None,
        }];
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &scope,
            &*executor,
        );

        // Dropping the filter would answer a request that named one ops line
        // with a number covering all of them — the wrong-number failure the
        // scope exists to prevent. Refused before a round trip, and named
        // separately from the island's own window refusal.
        let labor = read
            .skipped
            .iter()
            .find(|s| s.view == "labor_daily")
            .unwrap_or_else(|| panic!("{:?}", read.skipped));
        assert!(labor.reason.contains("scope"), "{labor:?}");
        assert!(read.values.is_empty(), "{:?}", read.values);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn only_the_view_that_owns_the_requested_dimension_resolves_it() {
        let layer = island_layer();
        assert_eq!(
            time_dimension_for(&layer, "labor_daily", "labor_daily.business_date").unwrap(),
            "labor_daily.business_date"
        );
        assert!(time_dimension_for(&layer, "ops_summary", "labor_daily.restaurant_id").is_err());
        assert!(time_dimension_for(&layer, "nonexistent", "labor_daily.business_date").is_err());
    }

    /// The coincidence this refuses: `labor_daily.business_date` and
    /// `ops_summary.business_date` are both `date`, and nothing in the schema
    /// says whether they are one calendar or two. Reading the island under the
    /// other view's window answers with a number no one can check.
    #[test]
    fn a_same_named_dimension_on_another_view_is_not_the_same_calendar() {
        let layer = island_layer();
        let err = time_dimension_for(&layer, "ops_summary", "labor_daily.business_date")
            .expect_err("a shared name is not proof of a shared calendar");
        // Names the dimension that cannot reach this view, and nothing else —
        // the caller supplies the view, and `baseline_note` groups every view
        // refused this way under one copy of this string.
        assert_eq!(
            err,
            "no `labor_daily.business_date` to anchor the window on"
        );
    }

    /// The two skips name different fixes, so they must never collapse into
    /// one: a warehouse that was asked and refused says to fix the warehouse,
    /// a view that was never asked says the window does not reach it.
    #[test]
    fn a_failed_query_and_an_unread_view_are_different_skips() {
        let layer = island_layer();
        let executor: Box<QueryExecutor> = Box::new(|_req: &QueryRequest| {
            Err(EngineError::QueryError("connection refused".to_string()))
        });
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &roots(),
            "labor_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        let by_view = |view: &str| {
            read.skipped
                .iter()
                .find(|s| s.view == view)
                .unwrap_or_else(|| panic!("{view} not skipped: {:?}", read.skipped))
                .clone()
        };

        // Asked, and the warehouse said no.
        let labor = by_view("labor_daily");
        assert_eq!(labor.kind, SkipKind::QueryFailed);
        assert!(labor.reason.contains("connection refused"), "{labor:?}");

        // Never asked, and for a reason that has nothing to do with the
        // warehouse being up.
        let ops = by_view("ops_summary");
        assert_eq!(ops.kind, SkipKind::NotQueried);
        assert!(ops.reason.contains("anchor the window on"), "{ops:?}");
    }

    /// One view, one `average` target and one `sum` target driven by the same
    /// measure — the shape that made a `coefficient 1.00` move a 27.50 measure
    /// to 8.3k.
    fn mixed_aggregation_layer() -> SemanticLayer {
        let shop = r#"
name: shop
table: public.shop
dialect: postgres
entities:
  - name: shop_day
    type: primary
    key: shop_day_key
  - name: store
    type: foreign
    key: store_id
dimensions:
  - name: shop_day_key
    type: string
    expr: shop_day_key
  - name: store_id
    type: string
    expr: store_id
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: spend_per_guest
    type: average
    expr: spend_per_guest
  - name: avg_order_value
    type: average
    expr: avg_order_value
    drivers:
      - measure: shop.spend_per_guest
        direction: positive
  - name: total_sales
    type: sum
    expr: net_sales
    drivers:
      - measure: shop.spend_per_guest
        direction: positive
"#;
        SemanticLayer::new(
            vec![oxy_airlayer_compat::parse_view_yaml(shop).unwrap()],
            None,
        )
    }

    #[test]
    fn an_average_target_fits_and_lands_in_its_own_space() {
        let layer = mixed_aggregation_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = island_executor(seen.clone());
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &["shop.spend_per_guest".to_string()],
            "shop.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        // Both targets fit. What differs is the space the coefficient is
        // applied in, and that is airlayer's `AggregateSpace` — not something
        // this module refuses on. A `coefficient 1.00` onto a 27.50 average
        // used to arrive as 8.3k; the tree now carries the space that stops it.
        for to in ["shop.avg_order_value", "shop.total_sales"] {
            assert_eq!(
                refusal_of(&fits, "shop.spend_per_guest", to),
                None,
                "{to} should fit on its own single-view panel"
            );
        }
        let mut tree = tree(&layer);
        oxy_airlayer_compat::engine::metric_tree_fit::apply_fitted_coefficients(&mut tree, &fits);
        let space_of = |id: &str| {
            tree.nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.aggregate_space)
                .expect("node in tree")
        };
        assert_eq!(
            space_of("shop.avg_order_value"),
            oxy_airlayer_compat::schema::models::AggregateSpace::Mean
        );
        assert_eq!(
            space_of("shop.total_sales"),
            oxy_airlayer_compat::schema::models::AggregateSpace::Total
        );
    }

    // ---------------------------------------------------------------------
    // The three-view, mixed-grain shape
    // ---------------------------------------------------------------------

    /// Airlayer's base-view refusal as `quickbooks_pl` provokes it.
    const NO_JOIN_TREE_QB: &str = "No valid join tree found; using 'sales_daily' as base view";

    /// The real workspace scenario, minimised but structurally faithful: THREE
    /// views at TWO grains, which is the case `island_layer` cannot express.
    ///
    /// `island_layer` has one anchor view and one island, so both flavours of
    /// "cannot window this view" collapse into the same test. Here they are
    /// separate views and they fail for different reasons, which is the point:
    ///
    /// - `daily_operations` is restaurant-day like `sales_daily`, shares its
    ///   `restaurant` foreign entity, and declares a `business_date` of its own
    ///   — same name, same `type: date`, same grain, joinable. Every surface
    ///   signal says "conform these" and the schema still declares nothing, so
    ///   it is left unread. This is the tempting coincidence at its most
    ///   tempting, and the cost of refusing is highest here.
    /// - `quickbooks_pl` declares no foreign entity at all (a join island) and
    ///   names its dates `report_start_date`/`report_end_date` over monthly
    ///   reporting periods. It has TWO date dimensions and neither is the
    ///   requested one, so there is not even a single unambiguous candidate to
    ///   fall back to — the shape that rules out "just use the view's own date
    ///   when it has exactly one".
    ///
    /// Driver chains and measure types are copied from the workspace, so the
    /// edges below are the ones a real `sales_daily.discount_rate` scenario
    /// reports on.
    fn mixed_grain_layer() -> SemanticLayer {
        let sales_daily = r#"
name: sales_daily
table: public.sales_daily
dialect: postgres
entities:
  - name: sales_day
    type: primary
    key: sales_day_key
  - name: restaurant
    type: foreign
    key: restaurant_id
dimensions:
  - name: sales_day_key
    type: string
    expr: sales_day_key
  - name: restaurant_id
    type: string
    expr: restaurant_id
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: discount_rate
    type: number
    expr: discount_rate
  - name: total_gross_sales
    type: sum
    expr: gross_sales
  - name: total_net_sales
    type: sum
    expr: net_sales
    drivers:
      - measure: sales_daily.total_gross_sales
        direction: positive
      - measure: sales_daily.discount_rate
        direction: negative
"#;
        let daily_operations = r#"
name: daily_operations
table: public.daily_operations
dialect: postgres
entities:
  - name: operations_day
    type: primary
    key: operations_day_key
  - name: restaurant
    type: foreign
    key: restaurant_id
dimensions:
  - name: operations_day_key
    type: string
    expr: operations_day_key
  - name: restaurant_id
    type: string
    expr: restaurant_id
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: total_net_sales
    type: sum
    expr: net_sales
    drivers:
      - measure: sales_daily.total_net_sales
        direction: positive
  - name: total_labor_cost
    type: sum
    expr: labor_cost
    drivers:
      - measure: daily_operations.total_net_sales
        direction: positive
"#;
        let quickbooks_pl = r#"
name: quickbooks_pl
table: public.quickbooks_pl
dialect: postgres
entities:
  - name: pl_line_item
    type: primary
    key: pl_line_item_key
dimensions:
  - name: pl_line_item_key
    type: string
    expr: pl_line_item_key
  - name: report_start_date
    type: date
    expr: report_start_date
  - name: report_end_date
    type: date
    expr: report_end_date
measures:
  - name: store_sales
    type: sum
    expr: store_sales
    drivers:
      - measure: sales_daily.total_net_sales
        direction: positive
  - name: merchant_fees
    type: sum
    expr: merchant_fees
    drivers:
      - measure: sales_daily.total_net_sales
        direction: positive
  - name: total_revenue
    type: sum
    expr: total_revenue
    drivers:
      - measure: quickbooks_pl.store_sales
        direction: positive
  - name: total_cogs
    type: sum
    expr: total_cogs
    drivers:
      - measure: quickbooks_pl.store_sales
        direction: positive
  - name: labor_cost
    type: sum
    expr: labor_cost
    drivers:
      - measure: daily_operations.total_labor_cost
        direction: positive
  - name: gross_profit
    type: sum
    expr: gross_profit
    drivers:
      - measure: quickbooks_pl.total_revenue
        direction: positive
      - measure: quickbooks_pl.total_cogs
        direction: negative
  - name: net_operating_income
    type: sum
    expr: net_operating_income
    drivers:
      - measure: quickbooks_pl.gross_profit
        direction: positive
  - name: net_income
    type: sum
    expr: net_income
    drivers:
      - measure: quickbooks_pl.net_operating_income
        direction: positive
"#;
        SemanticLayer::new(
            vec![
                oxy_airlayer_compat::parse_view_yaml(sales_daily).unwrap(),
                oxy_airlayer_compat::parse_view_yaml(daily_operations).unwrap(),
                oxy_airlayer_compat::parse_view_yaml(quickbooks_pl).unwrap(),
            ],
            None,
        )
    }

    fn mixed_grain_roots() -> Vec<String> {
        vec!["sales_daily.discount_rate".to_string()]
    }

    /// Refuses only requests that pull `quickbooks_pl` in beside another view.
    ///
    /// Deliberately narrower than `island_executor`: `sales_daily` and
    /// `daily_operations` really do share `restaurant_id`, so a request naming
    /// both would succeed against a warehouse. Modelling that lets the test
    /// tell apart the two reasons a view goes unread — a join that cannot be
    /// built, and a window that cannot be expressed — instead of letting a
    /// blanket refusal make every cross-view case look like a join failure.
    fn mixed_grain_executor(seen: Arc<Mutex<Vec<QueryRequest>>>) -> Box<QueryExecutor> {
        Box::new(move |req: &QueryRequest| {
            seen.lock().unwrap().push(req.clone());
            let views = views_touched(req);
            if views.len() > 1 && views.contains("quickbooks_pl") {
                return Err(EngineError::QueryError(NO_JOIN_TREE_QB.to_string()));
            }
            let value = |k: usize, day: usize| {
                (k as f64 + 1.0) * (day as f64 + 1.0) + ((day * (k + 3)) % 7) as f64 * 0.05
            };
            if req.dimensions.is_empty() {
                let mut row = Row::new();
                for (k, m) in req.measures.iter().enumerate() {
                    row.insert(m.replace('.', "__"), serde_json::json!(value(k, 1)));
                }
                return Ok(vec![row]);
            }
            let rows = (0..40)
                .map(|day| {
                    let mut row = Row::new();
                    for dim in &req.dimensions {
                        let alias = dim.replace('.', "__");
                        let v = if dim.ends_with("_date") {
                            serde_json::json!(format!("2026-01-{:02}", day + 1))
                        } else {
                            serde_json::json!("r1")
                        };
                        row.insert(alias, v);
                    }
                    for (k, m) in req.measures.iter().enumerate() {
                        row.insert(m.replace('.', "__"), serde_json::json!(value(k, day)));
                    }
                    row
                })
                .collect();
            Ok(rows)
        })
    }

    fn mixed_grain_fits() -> Vec<FittedDriver> {
        let layer = mixed_grain_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen);
        grouped_fit(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        )
    }

    /// The anchor view's own chain fits, and every other view's refusal names
    /// the window rather than the join.
    ///
    /// The two are different fixes and the distinction is the whole reason
    /// this module groups at all: "declare a `coefficient:`" is the answer to
    /// an unjoinable pair and the WRONG answer to an unwindowed one, where the
    /// edge is perfectly fittable the moment the window moves to its view.
    #[test]
    fn a_three_view_tree_fits_the_anchor_view_and_refuses_the_rest_by_window() {
        let fits = mixed_grain_fits();

        // The one edge the real scenario sized: both endpoints on the anchor
        // view, so nothing is crossed and the window is expressible.
        assert_eq!(
            refusal_of(
                &fits,
                "sales_daily.discount_rate",
                "sales_daily.total_net_sales"
            ),
            None,
            "the anchor view's own edge should fit: {fits:?}"
        );

        // Both endpoints inside `quickbooks_pl`. These need NO join and cross
        // nothing — they are refused purely because the window cannot be put
        // on a view whose dates are `report_start_date`/`report_end_date`.
        for (from, to) in [
            ("quickbooks_pl.store_sales", "quickbooks_pl.total_revenue"),
            ("quickbooks_pl.store_sales", "quickbooks_pl.total_cogs"),
            ("quickbooks_pl.total_cogs", "quickbooks_pl.gross_profit"),
            ("quickbooks_pl.total_revenue", "quickbooks_pl.gross_profit"),
            (
                "quickbooks_pl.gross_profit",
                "quickbooks_pl.net_operating_income",
            ),
            (
                "quickbooks_pl.net_operating_income",
                "quickbooks_pl.net_income",
            ),
        ] {
            let refusal = refusal_of(&fits, from, to)
                .unwrap_or_else(|| panic!("{from} -> {to} cannot be windowed"));
            assert!(
                refusal.contains("anchor the window on"),
                "{from} -> {to}: {refusal}"
            );
            assert!(
                !refusal.contains("not forecastable"),
                "{from} -> {to} needs no join; calling it unforecastable sends the author \
                 to declare a `coefficient:` on an edge whose problem is the calendar: \
                 {refusal}"
            );
        }
    }

    /// A joinable, same-grain neighbour still fits when an island shares the tree.
    ///
    /// `daily_operations` shares `restaurant_id` with `sales_daily` and buckets
    /// by the same day, so history CAN pair them — this edge is fittable and
    /// must stay fittable no matter what else the tree reaches. It regressed
    /// because every cross-view edge was fitted as one batch: `quickbooks_pl`
    /// declares no foreign entity, the batch failed the join, and all of it
    /// came back "not forecastable — declare a `coefficient:`", which is the
    /// wrong instruction for an edge whose magnitude is measurable.
    ///
    /// The refusal it wrongly received also named three views as sharing "no
    /// key or grain" when two of them share both, so the message could not
    /// even be read as a hint about the real island.
    #[test]
    fn a_joinable_pair_still_fits_when_an_unjoinable_one_shares_the_tree() {
        let fits = mixed_grain_fits();
        assert_eq!(
            refusal_of(
                &fits,
                "sales_daily.total_net_sales",
                "daily_operations.total_net_sales",
            ),
            None,
            "a pair that joins on `restaurant_id` at one grain must not inherit \
             the island's refusal: {fits:?}"
        );
    }

    /// A pair owning neither end of the window is refused for the window.
    ///
    /// `daily_operations -> quickbooks_pl` is a cross pair with the window on
    /// `sales_daily`. Queried, the panel dimension and both window filters name
    /// `sales_daily`, so the request carries THREE views and needs a join no
    /// edge in the pair implies — and when that join is what fails, the
    /// rewrite below reports it against the pair. That is the same
    /// misattribution this module exists to prevent, one view further out: an
    /// author reads "share no key or grain" about two views whose real problem
    /// is that neither carries the window.
    ///
    /// So it refuses unqueried, in the words a view that cannot be windowed
    /// already uses — which is also what lets `baseline_note` group it with
    /// them instead of printing a second, contradictory diagnosis.
    #[test]
    fn a_cross_pair_owning_neither_end_of_the_window_is_refused_for_the_window() {
        let layer = mixed_grain_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen.clone());
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );

        let refusal = refusal_of(
            &fits,
            "daily_operations.total_labor_cost",
            "quickbooks_pl.labor_cost",
        )
        .expect("neither view carries the window");
        assert!(refusal.contains("anchor the window on"), "{refusal}");
        assert!(
            !refusal.contains("not forecastable"),
            "the pair is accused of sharing no key when the window is what it \
             lacks, which sends the author to declare a `coefficient:` for a \
             magnitude a moved window could measure: {refusal}"
        );

        // Refused without asking, so the third view is never joined in at all.
        let requests = seen.lock().unwrap().clone();
        assert!(
            !requests
                .iter()
                .any(|r| r.measures.iter().any(|m| m == "quickbooks_pl.labor_cost")),
            "the pair should not have reached the warehouse: {requests:?}"
        );
    }

    /// An island's refusal names its own two views and no others.
    ///
    /// The note is the author's whole diagnosis, so a view listed in it is an
    /// accusation. Naming every view in the remainder pointed at innocent ones
    /// and buried the one view that actually has no key.
    #[test]
    fn an_unforecastable_note_names_only_the_pair_that_could_not_be_joined() {
        let fits = mixed_grain_fits();
        let refusal = refusal_of(
            &fits,
            "sales_daily.total_net_sales",
            "quickbooks_pl.store_sales",
        )
        .expect("the island bridge cannot be paired");
        assert!(refusal.contains("`quickbooks_pl`"), "{refusal}");
        assert!(refusal.contains("`sales_daily`"), "{refusal}");
        assert!(
            !refusal.contains("daily_operations"),
            "a view that joins fine must not be named in another pair's refusal: {refusal}"
        );
    }

    /// The genuine island bridges refuse as unforecastable, and point at the fix.
    ///
    /// `quickbooks_pl` declares no foreign entity, so `sales_daily.total_net_sales`
    /// and a QuickBooks measure share no key and no grain — monthly reporting
    /// periods against restaurant-days. No window helps; the magnitude has to
    /// be declared. The message says so in the author's terms.
    #[test]
    fn the_bridges_into_the_join_island_are_unforecastable_not_unwindowed() {
        let fits = mixed_grain_fits();
        for target in ["quickbooks_pl.store_sales", "quickbooks_pl.merchant_fees"] {
            let refusal = refusal_of(&fits, "sales_daily.total_net_sales", target)
                .unwrap_or_else(|| panic!("{target} cannot be paired from history"));
            assert!(refusal.contains("not forecastable"), "{target}: {refusal}");
            assert!(refusal.contains("coefficient:"), "{target}: {refusal}");
            assert!(
                refusal.contains("`quickbooks_pl`") && refusal.contains("`sales_daily`"),
                "{target}: {refusal}"
            );
            // Airlayer's own wording names a base view that is computed and
            // then discarded; it must not reach the panel.
            assert!(!refusal.contains("join tree"), "{target}: {refusal}");
        }
    }

    /// Declaring the coefficient removes the edge from the fit set entirely.
    ///
    /// This is the escape hatch the unforecastable refusal points at, and the
    /// reason the refusal is a complete answer rather than a dead end: an edge
    /// carrying a `coefficient:` is not a fittable edge, so it is never
    /// batched, never queried and never refused — the window and the join stop
    /// mattering. Pinned here because the refusal's advice is only honest
    /// while this holds.
    #[test]
    fn a_declared_coefficient_takes_an_unfittable_edge_out_of_the_fit_set() {
        let before = mixed_grain_fits();
        assert!(
            before
                .iter()
                .any(|f| f.from == "sales_daily.total_net_sales"
                    && f.to == "quickbooks_pl.store_sales"),
            "the undeclared edge is fittable, so it shows up refused"
        );

        let declared = mixed_grain_layer_with_declared_store_sales();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen);
        let after = grouped_fit(
            &tree(&declared),
            &declared,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        assert!(
            !after
                .iter()
                .any(|f| f.from == "sales_daily.total_net_sales"
                    && f.to == "quickbooks_pl.store_sales"),
            "a declared coefficient should leave nothing to fit or refuse: {after:?}"
        );
    }

    /// `mixed_grain_layer` with the island bridge carrying a declared magnitude.
    fn mixed_grain_layer_with_declared_store_sales() -> SemanticLayer {
        let views = mixed_grain_layer()
            .views
            .into_iter()
            .map(|mut v| {
                if v.name == "quickbooks_pl" {
                    for m in v.measures.iter_mut().flatten() {
                        if m.name == "store_sales" {
                            for d in m.drivers.iter_mut().flatten() {
                                d.coefficient = Some(0.92);
                            }
                        }
                    }
                }
                v
            })
            .collect();
        SemanticLayer::new(views, None)
    }

    /// [`mixed_grain_layer`] plus the dimension table that owns `restaurant`.
    ///
    /// The fact views declare `restaurant` as a FOREIGN key; nothing in that
    /// fixture declares it PRIMARY, so `instance_scope_filters` would have no
    /// view to build a scope on. Adding it reproduces the real shape: an
    /// instance scope lands on `restaurants`, and every measure worth valuing
    /// lives somewhere else.
    fn star_layer() -> SemanticLayer {
        let restaurants = r#"
name: restaurants
table: public.restaurants
dialect: postgres
entities:
  - name: restaurant
    type: primary
    key: restaurant_id
dimensions:
  - name: restaurant_id
    type: string
    expr: guid
"#;
        let mut views = mixed_grain_layer().views;
        views.push(oxy_airlayer_compat::parse_view_yaml(restaurants).unwrap());
        SemanticLayer::new(views, None)
    }

    /// A scope pinned to one instance, built as `instance_scope_filters` builds
    /// it: on whichever view is primary for the entity, which bears no relation
    /// to the view the window is anchored on.
    fn instance_scope(view: &str) -> Vec<QueryFilter> {
        vec![QueryFilter {
            member: Some(format!("{view}.restaurant_id")),
            operator: Some(FilterOperator::Equals),
            values: vec!["r1".to_string()],
            and: None,
            or: None,
        }]
    }

    /// No panel query reaches past a single view, or a single pair.
    ///
    /// The grouping exists so that one view a warehouse cannot join does not
    /// take down the batch every other view was riding in. That holds only
    /// while a request names at most the two views one edge connects — a third
    /// view in the request is a join nobody asked for, and the first thing it
    /// can do is fail.
    ///
    /// Asserted WITH a scope as well as without. A third view reaches a request
    /// two ways — the window and the scope — and an unscoped call exercises
    /// only the first, so the unscoped form of this test named an invariant the
    /// code did not hold for any scoped caller.
    #[test]
    fn three_views_are_never_queried_together() {
        for scope in [Vec::new(), instance_scope("daily_operations")] {
            let layer = mixed_grain_layer();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let executor = mixed_grain_executor(seen.clone());
            let _ = grouped_fit(
                &tree(&layer),
                &layer,
                &mixed_grain_roots(),
                "sales_daily.business_date",
                WINDOW,
                &scope,
                &*executor,
            );
            let requests = seen.lock().unwrap().clone();
            assert!(
                !requests.is_empty(),
                "the anchor view should have been read"
            );
            for req in &requests {
                let views = views_touched(req);
                assert!(
                    views.len() <= 2,
                    "a request pulled in {} views under scope {scope:?}, so it needs a \
                     join no single edge implies: {req:?}",
                    views.len()
                );
            }
        }
    }

    /// A pair that cannot carry the scope refuses instead of joining it in.
    ///
    /// The window gate guards the anchor's view only. The scope arrives from
    /// `instance_scope_filters` on a third view entirely, and `sales_daily ->
    /// quickbooks_pl` owns the window while owning nothing of the scope — so
    /// this pair passes the window check and would still have issued a
    /// three-view request.
    ///
    /// Dropping the filter is the alternative and is worse: it answers a
    /// request that named one restaurant with a number covering all of them.
    #[test]
    fn a_cross_pair_that_cannot_carry_the_scope_is_refused_not_joined() {
        let layer = mixed_grain_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen.clone());
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &instance_scope("daily_operations"),
            &*executor,
        );

        let refusal = refusal_of(
            &fits,
            "sales_daily.total_net_sales",
            "quickbooks_pl.store_sales",
        )
        .expect("the pair carries neither end of the scope");
        // Ends with, not equals: `fit_group` refuses through an executor that
        // fails, so airlayer shapes the message exactly as it shapes every
        // other refusal — which is the point of refusing that way.
        assert!(refusal.ends_with(SCOPE_NEEDS_A_JOIN), "{refusal}");
        // The same constant reaches `baseline_note` unprefixed from the values
        // path, where skips group by exact reason, so the two branches must not
        // paraphrase each other into two groups saying one thing.
        assert!(!refusal.contains("anchor the window on"), "{refusal}");
        assert!(!refusal.contains("not forecastable"), "{refusal}");
    }

    /// A scope naming only pair views is honoured, however it is nested.
    ///
    /// The tempting predicate is `on_view(f, a) || on_view(f, b)`, which is a
    /// conservative approximation and wrong here: an `or:` group with one leaf
    /// on each pair view fails both disjuncts while naming nothing outside the
    /// pair. `rewrite_filter` resolves per LEAF, so this pair still fits — a
    /// refusal would be the module inventing a join problem where the scope is
    /// exactly expressible.
    #[test]
    fn a_scope_spanning_both_pair_views_is_honoured_not_refused() {
        let layer = mixed_grain_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen.clone());
        let spanning = vec![QueryFilter {
            member: None,
            operator: None,
            values: Vec::new(),
            and: None,
            or: Some(vec![
                instance_scope("sales_daily").remove(0),
                instance_scope("daily_operations").remove(0),
            ]),
        }];
        let fits = grouped_fit(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &spanning,
            &*executor,
        );
        assert_eq!(
            refusal_of(
                &fits,
                "sales_daily.total_net_sales",
                "daily_operations.total_net_sales",
            ),
            None,
            "every leaf names a pair view, so the pair can express this scope: {fits:?}"
        );
    }

    /// An instance scope is re-expressed onto the fact view that declares the
    /// entity, instead of being refused for naming the dimension table.
    ///
    /// The bug this fixes: `instance_scope_filters` always builds the scope on
    /// the entity's PRIMARY view, and a group read one view at a time never
    /// gets the join that would carry it. So every per-store baseline on a
    /// fact view was refused — which, in a star schema, is every measure worth
    /// baselining. The panel's scenario line went missing on a store whose
    /// curve had just drawn, because `projection` resolves that join and
    /// `baseline` does not.
    #[test]
    fn an_instance_scope_is_rewritten_onto_the_view_declaring_the_entity() {
        let layer = star_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "daily_operations.business_date",
            WINDOW,
            &instance_scope("restaurants"),
            &*executor,
        );

        assert!(
            read.values
                .contains_key("daily_operations.total_labor_cost"),
            "the fact view declares `restaurant`, so the scope is expressible: {read:?}",
            read = read.skipped
        );

        let request = seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.measures.iter().any(|m| view_of(m) == "daily_operations"))
            .cloned()
            .expect("daily_operations was queried");
        // The rewrite has to leave a ONE-view request. Carrying
        // `restaurants.restaurant_id` through would join the dimension table in
        // as a second view — exactly the join this module reads one view at a
        // time to avoid.
        assert_eq!(
            views_touched(&request),
            BTreeSet::from(["daily_operations".to_string()]),
            "{request:?}"
        );
        assert!(
            request.filters.iter().any(|f| {
                f.member.as_deref() == Some("daily_operations.restaurant_id")
                    && f.values == vec!["r1".to_string()]
            }),
            "the instance is still pinned, on this view's own column: {request:?}"
        );
    }

    /// A view with no declaration of the entity is still refused, not widened.
    ///
    /// `quickbooks_pl` carries no `restaurant` entity at all, so there is no
    /// column the scope could be rewritten onto. Dropping it instead would
    /// answer a request naming one restaurant with the whole chain's P&L —
    /// the wrong-number failure the refusal exists for.
    #[test]
    fn a_view_that_does_not_declare_the_entity_is_still_refused() {
        let layer = star_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            // Anchored on `quickbooks_pl`'s own calendar so it clears the
            // window gate and the scope is what decides it.
            "quickbooks_pl.report_start_date",
            WINDOW,
            &instance_scope("restaurants"),
            &*executor,
        );

        let skip = read
            .skipped
            .iter()
            .find(|s| s.view == "quickbooks_pl")
            .expect("quickbooks_pl cannot carry a restaurant scope");
        assert_eq!(skip.reason, SCOPE_NEEDS_A_JOIN, "{skip:?}");
        assert_eq!(skip.kind, SkipKind::NotQueried);
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.measures.iter().any(|m| view_of(m) == "quickbooks_pl")),
            "refused before querying, not filtered afterwards"
        );
    }

    /// The note names both unread views once, not once per view.
    ///
    /// Two views refused for the same reason is where the per-view sentence
    /// started reading as boilerplate; three views is the shape that made it
    /// unreadable on a real workspace.
    #[test]
    fn the_note_names_both_unread_views_together() {
        let layer = mixed_grain_layer();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = mixed_grain_executor(seen);
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &mixed_grain_roots(),
            "sales_daily.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        let skipped: BTreeSet<&str> = read.skipped.iter().map(|s| s.view.as_str()).collect();
        assert!(
            skipped.contains("quickbooks_pl") && skipped.contains("daily_operations"),
            "both non-anchor views should be reported unread: {:?}",
            read.skipped
        );
        // One reason, shared — which is what lets `baseline_note` group them.
        let reasons: BTreeSet<&str> = read.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(reasons.len(), 1, "{:?}", read.skipped);
        let reason = reasons.iter().next().unwrap();
        assert!(reason.contains("anchor the window on"), "{reason}");
        // Grouping only works while the reason names no view of its own.
        assert!(!reason.contains("daily_operations"), "{reason}");
        assert!(!reason.contains("quickbooks_pl"), "{reason}");
    }

    /// `stores.guest_covers` multiplies the root by `stores.covers`, a node
    /// no edge from the root reaches.
    ///
    /// `stores.net_per_guest` reads `{{stores.net_sales}}`, which `sales`
    /// declares and `stores` only inherits through the shared `store_id`
    /// entity — an induced measure, queryable but no tree node, so no edge
    /// leads to it. `extra_sources` adds views declaring `net_sales` too, so
    /// the induced name is reachable from several.
    fn induced_layer(extra_sources: &[&str]) -> SemanticLayer {
        let stores = r#"
name: stores
table: public.stores
dialect: postgres
entities:
  - name: store_id
    type: primary
    key: store_id
dimensions:
  - name: store_id
    type: string
    expr: store_id
  - name: business_date
    type: date
    expr: business_date
measures:
  - name: guests
    type: sum
    expr: guests
  - name: net_per_guest
    type: number
    expr: "{{stores.net_sales}} / NULLIF({{stores.guests}}, 0)"
  - name: covers
    type: sum
    expr: covers
  - name: guest_covers
    type: number
    expr: "{{stores.guests}} * {{stores.covers}}"
"#;
        let source = |name: &str| {
            format!(
                "name: {name}\ntable: public.{name}\ndialect: postgres\nentities:\n  - name: \
                 store_id\n    type: foreign\n    key: store_id\ndimensions:\n  - name: \
                 store_id\n    type: string\n    expr: store_id\nmeasures:\n  - name: \
                 net_sales\n    type: sum\n    expr: net_sales\n"
            )
        };
        let mut views = vec![
            oxy_airlayer_compat::parse_view_yaml(stores).unwrap(),
            oxy_airlayer_compat::parse_view_yaml(&source("sales")).unwrap(),
        ];
        for name in extra_sources {
            views.push(oxy_airlayer_compat::parse_view_yaml(&source(name)).unwrap());
        }
        SemanticLayer::new(views, None)
    }

    /// Answers the way `compile_query` does: every measure under its own
    /// alias, except the induced `stores.net_sales`, which is rewritten to its
    /// source and so comes back keyed `sales__net_sales`.
    fn induced_executor(seen: Arc<Mutex<Vec<QueryRequest>>>) -> Box<QueryExecutor> {
        Box::new(move |req: &QueryRequest| {
            seen.lock().unwrap().push(req.clone());
            let mut row = Row::new();
            for m in &req.measures {
                let (alias, v) = match m.as_str() {
                    "stores.net_sales" => ("sales__net_sales".to_string(), 40.0),
                    "stores.guests" => (m.replace('.', "__"), 2.0),
                    _ => (m.replace('.', "__"), 20.0),
                };
                row.insert(alias, serde_json::json!(v));
            }
            Ok(vec![row])
        })
    }

    /// The predict baseline's half of airlayer's
    /// `reachable_values_outcome_reads_an_induced_input_by_its_source_alias`:
    /// a reached composite is recomputed from every input its expression
    /// reads, and an induced one is read back under its source alias. Without
    /// either, the composite has no level and its impact is unquantifiable.
    #[test]
    fn an_induced_input_is_read_by_its_source_alias() {
        let layer = induced_layer(&[]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = induced_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &["stores.guests".to_string()],
            "stores.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        let requests = seen.lock().unwrap().clone();
        assert_eq!(
            read.values.get("stores.net_sales"),
            Some(&40.0),
            "values {:?}, requests {requests:?}",
            read.values
        );
        assert_eq!(read.values.get("stores.guests"), Some(&2.0));
    }

    /// A reached composite is recomputed from every input, so the one beside
    /// the lever's path is read too: `guest_covers` moves with `guests`, and
    /// sizing that needs `covers`' level although nothing pushes into it.
    #[test]
    fn a_reached_composite_s_other_input_is_read() {
        let layer = induced_layer(&[]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = induced_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &["stores.guests".to_string()],
            "stores.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        assert_eq!(
            read.values.get("stores.covers"),
            Some(&20.0),
            "{:?}",
            read.values
        );
    }

    /// The induced read is its own request, so a refusal there — the engine
    /// cannot compile it, or the ref was a dimension — leaves only that input
    /// unvalued, never the view's own measures beside it.
    #[test]
    fn a_failed_induced_read_leaves_the_view_s_own_values() {
        let layer = induced_layer(&[]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let inner = induced_executor(seen.clone());
        let log = seen.clone();
        let executor: Box<QueryExecutor> = Box::new(move |req: &QueryRequest| {
            if req.measures.iter().any(|m| m == "stores.net_sales") {
                log.lock().unwrap().push(req.clone());
                return Err(EngineError::QueryError("refused".to_string()));
            }
            inner(req)
        });
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &["stores.guests".to_string()],
            "stores.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        assert!(
            !read.values.contains_key("stores.net_sales"),
            "{:?}",
            read.values
        );
        assert_eq!(read.values.get("stores.guests"), Some(&2.0));
        assert!(matches!(read.outcome, BaselineOutcome::Valued { .. }));
        assert!(read.skipped.is_empty(), "{:?}", read.skipped);
        // Silent in the response, not in the read: the caller must know not
        // to cache a baseline missing an input it could have had.
        assert_eq!(read.input_errors.len(), 1, "{:?}", read.input_errors);
        assert!(
            read.input_errors[0].contains("stores.net_sales")
                && read.input_errors[0].contains("refused"),
            "{:?}",
            read.input_errors
        );
        let requests = seen.lock().unwrap().clone();
        assert!(
            requests
                .iter()
                .any(|r| r.measures.iter().any(|m| m == "stores.net_sales")),
            "the induced input should have been asked for: {requests:?}"
        );
    }

    /// Reachable from `sales` and `refunds`: picking one would answer a
    /// different question, and asking for it fails the request in the engine,
    /// so it is neither asked for nor valued — and the view's own measures are
    /// still read.
    #[test]
    fn an_ambiguous_induced_input_is_left_without_a_value() {
        let layer = induced_layer(&["refunds"]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = induced_executor(seen.clone());
        let read = grouped_values(
            &tree(&layer),
            &layer,
            &["stores.guests".to_string()],
            "stores.business_date",
            WINDOW,
            &[],
            &*executor,
        );
        assert!(
            !read.values.contains_key("stores.net_sales"),
            "{:?}",
            read.values
        );
        assert_eq!(read.values.get("stores.guests"), Some(&2.0));
        let requests = seen.lock().unwrap().clone();
        assert!(
            requests
                .iter()
                .all(|r| !r.measures.iter().any(|m| m == "stores.net_sales")),
            "{requests:?}"
        );
    }
}
