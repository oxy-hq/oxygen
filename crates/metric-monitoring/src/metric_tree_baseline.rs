//! Rescuing the scenario queries one batched request cannot answer.
//!
//! Airlayer refuses a query that puts an additive measure (`sum`/`count`/
//! `min`/`max`) in the same view's CTE as a non-additive one (`custom`/`avg`/
//! `count_distinct`/…) when a dimension, filter or segment drags a one-to-many
//! join into that CTE — the additive side would be double-counted by the
//! fan-out and nothing would say so. The refusal is correct. What it takes
//! down with it is not: a scenario batches measures for its own reasons, and
//! the mixture is an artefact of the metric tree's shape, not something the
//! analyst asked for.
//!
//! Two separate call sites batch this way, and BOTH hit it:
//!
//!   * the baseline (`reachable_values_outcome`) asks for every node forward-
//!     reachable from the levers in one query;
//!   * the fit (`fit_driver_coefficients`) asks for both endpoints of every
//!     fittable driver edge in one panel query.
//!
//! Which one breaks depends only on which lever is pinned, so fixing them
//! one at a time just moves the symptom. This module fixes the shape instead:
//! it decorates the `QueryExecutor` itself, so any batched request that a
//! mixture refuses gets retried as unmixed halves and merged back, and neither
//! caller needs to know.
//!
//! The split is by **(view, additivity)** — one request per pair. Additivity
//! is what clears the guard; the view is what stops the split from causing a
//! worse problem than it solves. An additivity-only split co-locates measures
//! from views at different grains, and asking for a store-grain sum under
//! check-grain dimensions joins the coarse view in on a partial key, fanning
//! out BOTH measures. `checks.total_guests` came back 3,100x inflated and
//! constant across every day of a panel, which the fit reports as "the driver
//! does not vary within any panel" — a silent wrong number where there had
//! been a loud refusal. Grouping by view too means a request only ever carries
//! measures that already shared a CTE, so the split can never invent a join
//! the original query did not have.
//!
//! Deliberately a RETRY, not the default path. A single-view tree batches
//! fine, and must not pay a second round trip for a problem it doesn't have.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree::MetricTree;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_airlayer_compat::schema::models::MeasureType;

type Row = serde_json::Map<String, serde_json::Value>;

/// Soft cap on the baseline read as a whole — not one query but a fan-out of
/// them. `baseline_reads` asks `grouped_values` for one query per view in the
/// forward-reachable set, then `grouped_fit` for one more per single-view
/// group plus up to one per cross-view pair (worst case N + V(V-1)/2), all
/// against the same warehouse a cold scan can already hang on its own — same
/// reasoning as `metric_tree::EXPLAIN_TIMEOUT`, on a shorter clock because this handler
/// exists to show a lever's current value, not to explain a query plan.
///
/// The outer `tokio::time::timeout` around this handler's `spawn_blocking`
/// only bounds how long the CLIENT waits — it cannot cancel the OS thread, so
/// a fan-out with no deadline of its own would keep issuing warehouse queries
/// after the response had already gone back as a timeout. What actually
/// bounds the fan-out is the shared, absolute deadline `baseline_reads`
/// builds from this constant and threads through
/// `metric_tree_baseline::splitting_executor` — checked before every single
/// query the executor is asked to run, not only inside its additivity-split
/// retry — so the read degrades to partial values and a recorded budget
/// refusal instead of outliving the request that asked for it.
///
/// `pub(crate)` so the `projects/` twin shares this exact number instead of
/// keeping its own copy: two handlers hitting the same warehouse with two
/// silently diverging deadlines is a bug waiting for someone to tune one.
pub const BASELINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The tail of [`BASELINE_TIMEOUT`] that [`BASELINE_QUERY_BUDGET`] deliberately
/// does NOT spend — reserved for *finishing*, not for *starting*.
///
/// The deadline cannot make the response beat `BASELINE_TIMEOUT`: nothing
/// bounds a single warehouse query, and the outer `tokio::time::timeout` wraps
/// a `spawn_blocking` it cannot cancel. What it can do is stop the fan-out
/// issuing a query so late that the request is already doomed when it lands.
/// So the budget is `BASELINE_TIMEOUT` MINUS what one last query plus the work
/// after it plausibly needs — a subtraction, not a fraction of the whole.
///
/// Ten seconds covers, in order of cost: the one query already in flight when
/// the deadline passes — uncancellable, and the only genuinely unbounded term
/// here — plus everything `baseline_reads` still has to do after it, which is
/// `FittedDriver::with_profile`, `classify_unvalued`, a sort and the JSON.
/// That tail is in-memory over a few hundred rows: microseconds against the
/// query's seconds. So this is effectively all in-flight-query allowance, and
/// generous at that — more than ten times the per-call allowance the fan-out
/// itself implies (see [`BASELINE_QUERY_BUDGET`]).
pub const BASELINE_QUERY_HEADROOM: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a scenario baseline may go on ISSUING warehouse queries.
///
/// **What it is.** `BASELINE_TIMEOUT - BASELINE_QUERY_HEADROOM`, as ONE
/// absolute deadline shared by both of `baseline_reads`' reads and every
/// query either of them provokes. Absolute and shared because a per-call
/// duration would restart for each read and so bound nothing.
///
/// **What it has to fit — the whole fan-out, not one query.** For a
/// forward-reachable set spanning `V` views, `baseline_reads` issues
///
/// ```text
///   V              grouped_values  — one request per view
/// + V              grouped_fit     — one panel request per single-view group
/// + V(V-1)/2       grouped_fit     — one panel request per cross-view PAIR
/// = V(V+3)/2 executor calls
/// ```
///
/// plus one more `grouped_values` request for each view whose reached
/// composites read an input that is no tree node (an induced measure — see
/// `metric_tree_inputs`), so at most `V` more and none on a tree without one;
/// and each of those may fan out again, into one query per (view, additivity)
/// group, when airlayer refuses the batch. `example_new` — the workspace this
/// repo's fixtures and `tests/platform/metric_tree_fit_panel` run against —
/// ships six views, so `V = 6` gives `6 + 6 + 15 = 27` executor calls. Twenty
/// seconds across 27 of them is ~740ms per warehouse round trip.
///
/// **Why not half of `BASELINE_TIMEOUT`.** It was `BASELINE_TIMEOUT / 2` when
/// the deadline gated only the additivity-split RETRY: a rare path, a handful
/// of queries, where spending half the request was plainly fine. It now gates
/// every query the executor serves — `split_groups` checks it before issuing
/// `req` at all — so that same 15s had to cover all 27 calls: ~550ms each,
/// under a cold warehouse round trip. That made the DEADLINE, not the
/// warehouse, the thing that failed the read: a six-view tree that used to
/// answer in ~20s would start returning partial values and `BUDGET_SPENT`.
/// Widening the remit without re-sizing the number was the regression.
///
/// **What happens when it is hit.** `split_groups` returns `BUDGET_SPENT`
/// rather than issuing the query, so the read degrades to values for the views
/// that answered plus a refusal naming the budget for the rest — instead of
/// outliving the request that asked for it. That degradation is the point:
/// pushing this any closer to `BASELINE_TIMEOUT` trades an honest partial
/// answer for a timed-out response with queries still hitting the warehouse.
pub const BASELINE_QUERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(
    BASELINE_TIMEOUT
        .as_secs()
        .saturating_sub(BASELINE_QUERY_HEADROOM.as_secs()),
);

/// The shared deadline a baseline fan-out starts against.
///
/// A function rather than an `Instant::now() + BASELINE_QUERY_BUDGET` at each
/// call site: `metric_tree_projection` kept its own copy of the old
/// `BASELINE_TIMEOUT / 2`, so tuning one of them tuned half the surface. Same
/// reasoning that makes `BASELINE_TIMEOUT` itself `pub(crate)`.
pub fn baseline_query_deadline() -> std::time::Instant {
    std::time::Instant::now() + BASELINE_QUERY_BUDGET
}

/// Node ids reachable *forward* from `roots`, roots included, in BFS order.
///
/// The same traversal `reachable_values_outcome` runs to decide what to ask
/// for, mirrored here because `classify_unvalued` has to diff against exactly
/// that set and airlayer does not expose it.
pub fn forward_reachable(tree: &MetricTree, roots: &[String]) -> Vec<String> {
    let mut fwd: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &tree.edges {
        fwd.entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }

    let mut wanted: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for root in roots {
        if seen.insert(root.as_str()) {
            wanted.push(root.clone());
            queue.push_back(root.as_str());
        }
    }
    while let Some(node) = queue.pop_front() {
        for &next in fwd.get(node).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(next) {
                wanted.push(next.to_string());
                queue.push_back(next);
            }
        }
    }
    wanted
}

/// Whether `id` (a `view.measure` path) is additive in airlayer's sense.
///
/// The list mirrors the guard in airlayer's `sql_generator`, which is the only
/// thing that matters here: a measure this returns `true` for must never share
/// a request with one it returns `false` for. A measure that isn't in the
/// layer at all counts as non-additive — the conservative side, since a wrong
/// `true` is what re-forms the mixture the split exists to break up.
fn is_additive(layer: &SemanticLayer, id: &str) -> bool {
    let Some((view_name, measure_name)) = id.split_once('.') else {
        return false;
    };
    let Some(view) = layer.views.iter().find(|v| v.name == view_name) else {
        return false;
    };
    view.measures_list()
        .iter()
        .find(|m| m.name == measure_name)
        .is_some_and(|m| {
            matches!(
                m.measure_type,
                MeasureType::Sum | MeasureType::Count | MeasureType::Min | MeasureType::Max
            )
        })
}

/// A row's identity for merging: everything that is not one of `measures`.
///
/// Deliberately derived by exclusion rather than from the request's
/// dimensions, so this needs to know nothing about how airlayer aliases a
/// dimension — only which columns it just asked for. A request with no
/// dimensions (the baseline) collapses every row to the same empty key, which
/// is right: it returns one row.
fn row_identity(row: &Row, measures: &HashSet<String>) -> String {
    let dims: BTreeMap<&str, &serde_json::Value> = row
        .iter()
        .filter(|(k, _)| !measures.contains(k.as_str()))
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    serde_json::to_string(&dims).unwrap_or_default()
}

/// Measure paths as they come back in a row.
fn aliases(measures: &[String]) -> HashSet<String> {
    measures.iter().map(|m| m.replace('.', "__")).collect()
}

/// Stitch the group answers back into the rows one query would have returned.
///
/// Every group asks for the same dimensions, so their rows are keyed
/// identically and a row of the whole is the union of its parts. A key present
/// in only some groups still yields a row — with the measures we got, and
/// without the ones we didn't, which is exactly how a caller reads a measure
/// that came back missing anyway.
fn merge_groups(groups: Vec<(Vec<Row>, HashSet<String>)>) -> Vec<Row> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, Row> = HashMap::new();
    for (rows, measures) in groups {
        for row in rows {
            let key = row_identity(&row, &measures);
            match merged.get_mut(&key) {
                Some(existing) => existing.extend(row),
                None => {
                    order.push(key.clone());
                    merged.insert(key, row);
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|k| merged.remove(&k))
        .collect()
}

/// The group a measure belongs to: its view, and whether it is additive.
///
/// Splitting on additivity alone is NOT enough, and getting that wrong is
/// worse than the refusal it replaces. Additivity-only groups co-locate
/// measures from views at different grains — a check-grain `sum` with a
/// store-grain `sum` — and asking for both under check-grain dimensions joins
/// the coarser view in on a partial key. The coarse measure fans out, and so
/// does the fine one: `checks.total_guests` came back as 12,071 per row
/// instead of 3.886, identical on every day of a panel, which reads downstream
/// as "the driver does not vary within any panel". No guard fires, because
/// within each view nothing is mixed.
///
/// Grouping by view as well means a request only ever carries measures that
/// already shared a CTE, so the split cannot invent a join that the original
/// query never had.
fn measure_group(layer: &SemanticLayer, id: &str) -> (String, bool) {
    let view = id.split_once('.').map(|(v, _)| v).unwrap_or(id);
    (view.to_string(), is_additive(layer, id))
}

/// Whether a group came back at the row cap, i.e. possibly truncated.
///
/// Groups truncated at different boundaries would merge into rows whose
/// measures belong to different sets of groups — a silently misaligned panel,
/// which is a worse answer than the refusal we are trying to replace. Bail
/// instead. In practice the batched callers ask for an unbounded limit, so
/// this is a backstop rather than a live case.
fn possibly_truncated(rows: &[Row], limit: Option<u64>) -> bool {
    limit.is_some_and(|n| rows.len() as u64 >= n)
}

/// Whether `err` is the additivity refusal — the only error a split can fix.
///
/// Retrying on *any* error is not a wider safety net, it is a correctness bug.
/// The case that matters is a request spanning two views with no join path
/// between them: airlayer refuses it as a whole ("No join path found between
/// 'checks' and 'store_days'"), and `store_days.view.yml` documents that
/// refusal as the safe outcome — the values come back empty and the impacts
/// degrade to unquantifiable. Split, each half succeeds on its own: the
/// store-grain half runs under a check-grain time dimension, joins on
/// `location_id` with nothing tying the dates together, and returns a
/// whole-window total repeated on every date. The fit catches that on zero
/// within-panel variance; the values path has no such check, so a documented
/// empty becomes a populated wrong number.
///
/// So match the refusal, not the failure. All three of airlayer's additivity
/// refusals (the mixed-CTE guard, the non-additive fan-out, the mixed fan-out
/// with time dimensions) name `non-additive` in their message; nothing else in
/// the engine does. Matching on the text is what a `QueryError(String)` allows
/// — if airlayer ever grows a typed variant for it, this is the one place to
/// change.
///
/// What makes a text match acceptable across a git pin is that BOTH failure
/// directions are safe. A false positive spends one extra round trip and
/// returns the original error anyway. A false negative just keeps today's
/// refusal, which is the pre-split behaviour. Neither can produce a wrong
/// number — the outcome the whole module exists to avoid.
fn is_additivity_refusal(err: &EngineError) -> bool {
    err.to_string().contains("non-additive")
}

/// The original refusal, with the error that stopped the retry attached.
///
/// The original is still what gets reported — the split is all-or-nothing, and
/// a mixture is what the caller has to fix — but reporting it *alone* hides
/// the only evidence that something else was also wrong. That cost a real
/// triage: a projection panel came back naming a `daily_operations` additivity
/// mixture when every one of its split groups had actually been rejected for a
/// ClickHouse type error in the shared time dimension. The groups are the only
/// place that second error is ever raised, so if it is not carried out here it
/// is lost to a `warn!` nobody reads.
///
/// The original leads, so a matcher keyed on its text (like
/// [`is_additivity_refusal`], if this is ever nested) still sees what it saw.
fn with_group_cause(original: &EngineError, cause: &EngineError, group: &[String]) -> EngineError {
    EngineError::QueryError(format!(
        "{original} (splitting it did not help: the group [{}] then failed with: {cause})",
        group.join(", ")
    ))
}

/// Said when the read's shared budget is spent before a group could be asked.
///
/// A whole sentence, and deliberately not phrased as a rejection: nothing was
/// asked, so nothing was refused. The caller that tolerates a partial answer
/// shows this verbatim, and "the query failed" would send someone reading it
/// after a warehouse problem that does not exist.
pub const BUDGET_SPENT: &str = "the read's shared query budget ran out before this measure could be queried; \
     try a shorter period or a coarser granularity";

/// A group the split asked for and did not get, and why.
///
/// Carried out of the split rather than folded into one error because a caller
/// that survives a failed group has to attribute the loss to the measures that
/// suffered it. A measure with no series and no reason renders as 0, which is
/// the single outcome this surface exists to prevent.
#[derive(Debug, Clone)]
pub struct GroupFailure {
    /// The measures that were in the group.
    pub measures: Vec<String>,
    /// Why it did not answer, as a sentence a caller can show.
    pub reason: String,
}

/// What one group's failure does to the groups that succeeded.
///
/// Threaded explicitly rather than decided here, because it is a property of
/// the CALLER and not of the split: whether a half-answer is safe depends
/// entirely on whether the surface reading it can say, per measure, that the
/// measure was never successfully asked for. See [`run_with_split`] and
/// [`PartialSplitExecutor`] for the two answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnGroupFailure {
    /// Discard the whole retry and surface the original refusal.
    Abandon,
    /// Keep what answered and report the rest as [`GroupFailure`]s.
    Keep,
}

/// One group's rows, or the reason they are not usable.
///
/// Truncation is a failure like any other from the split's point of view: a
/// group at the row cap cannot be merged (see [`possibly_truncated`]), so it
/// did not answer.
fn run_group(
    inner: &QueryExecutor,
    req: &QueryRequest,
    measures: &[String],
) -> Result<Vec<Row>, EngineError> {
    let part = QueryRequest {
        measures: measures.to_vec(),
        ..req.clone()
    };
    let rows = inner(&part)?;
    if possibly_truncated(&rows, req.limit) {
        return Err(EngineError::QueryError(format!(
            "it came back at the row cap ({} rows); merging a truncated group could \
             misalign the panel",
            rows.len()
        )));
    }
    Ok(rows)
}

/// The (view, additivity) groups `req`'s measures fall into.
///
/// A BTreeMap so the request order is deterministic run to run, which is what
/// lets an executor cache in front of this ever hit.
fn measure_groups(
    layer: &SemanticLayer,
    req: &QueryRequest,
) -> BTreeMap<(String, bool), Vec<String>> {
    let mut grouped: BTreeMap<(String, bool), Vec<String>> = BTreeMap::new();
    for m in &req.measures {
        grouped
            .entry(measure_group(layer, m))
            .or_default()
            .push(m.clone());
    }
    grouped
}

/// Run `req` through `inner`, and if additivity refused it, as one request
/// per group.
///
/// Returns the merged rows and, under [`OnGroupFailure::Keep`], the groups
/// that did not answer. Under [`OnGroupFailure::Abandon`] the failure list is
/// always empty — the first failed group returns instead.
///
/// Checks `deadline` BEFORE issuing `req` itself, not only inside the retry
/// loop below. This is the executor `baseline_reads` hands to both of its
/// reads, and each of those issues this call once per view (`grouped_values`)
/// or once per group/pair (`grouped_fit`) — most of which never trip the
/// additivity guard and so never reach the loop at all. A deadline that only
/// the loop consulted bounded the retry but nothing else: the ordinary,
/// unsplit fan-out across N views (and up to N + V(V-1)/2 fit queries) ran to
/// completion on the `spawn_blocking` thread regardless of how much of
/// `BASELINE_TIMEOUT` was already spent, because the outer
/// `tokio::time::timeout` cannot cancel that thread once it is running.
fn split_groups(
    req: &QueryRequest,
    layer: &SemanticLayer,
    inner: &QueryExecutor,
    deadline: std::time::Instant,
    on_failure: OnGroupFailure,
) -> Result<(Vec<Row>, Vec<GroupFailure>), EngineError> {
    if std::time::Instant::now() >= deadline {
        return Err(EngineError::QueryError(BUDGET_SPENT.to_string()));
    }
    let original = match inner(req) {
        Ok(rows) => return Ok((rows, Vec::new())),
        Err(e) => e,
    };
    if !is_additivity_refusal(&original) {
        tracing::debug!(
            error = %original,
            measures = req.measures.len(),
            "metric-tree batched query failed for a reason a split cannot fix; not retrying"
        );
        return Err(original);
    }

    let grouped = measure_groups(layer, req);
    // One group means the request was already as narrow as the split can make
    // it, so a retry would spend a round trip to fail the same way.
    if grouped.len() < 2 {
        tracing::warn!(
            error = %original,
            measures = req.measures.len(),
            "metric-tree query refused for mixed additivity but is already one group; \
             nothing to split"
        );
        return Err(original);
    }

    // The split costs one round trip per (view, additivity) group, and nothing
    // bounds the group count but the workspace's own shape — a six-view tree
    // reaches ~12 per read, x2 reads, all inside `BASELINE_TIMEOUT`. The outer
    // `tokio::time::timeout` cannot help: it wraps a `spawn_blocking` and
    // cannot cancel the thread, so it turns a slow split into a timed-out
    // response with the queries still running. Stop issuing them instead.
    //
    // An ABSOLUTE deadline, shared by every call this executor serves. A
    // per-call duration would restart here, and the same executor is handed to
    // both of `baseline_reads`' reads — so two splits could each spend the
    // whole allowance and blow the outer timeout anyway, which is precisely
    // what this is here to prevent.
    let mut groups: Vec<(Vec<Row>, HashSet<String>)> = Vec::with_capacity(grouped.len());
    let mut failures: Vec<GroupFailure> = Vec::new();
    for measures in grouped.into_values() {
        let answered = if std::time::Instant::now() >= deadline {
            Err(EngineError::QueryError(BUDGET_SPENT.to_string()))
        } else {
            run_group(inner, req, &measures)
        };
        match answered {
            Ok(rows) => groups.push((rows, aliases(&measures))),
            Err(e) => {
                let keep_partial = on_failure == OnGroupFailure::Keep;
                tracing::warn!(
                    original = %original,
                    error = %e,
                    group = ?measures,
                    keep_partial,
                    "metric-tree split group did not answer"
                );
                if on_failure == OnGroupFailure::Abandon {
                    return Err(with_group_cause(&original, &e, &measures));
                }
                failures.push(GroupFailure {
                    measures,
                    reason: e.to_string(),
                });
            }
        }
    }
    Ok((merge_groups(groups), failures))
}

/// The all-or-nothing split: one group failing takes the retry down with it.
///
/// If any group still errors, the ORIGINAL error is what surfaces. Returning
/// the groups that worked would mean the rest read as "the warehouse returned
/// no such column" — indistinguishable from an empty window, when they were in
/// fact never asked for successfully. The scenario baseline treats those two
/// very differently and only one is true.
///
/// A caller that CAN tell them apart, because every measure it returns carries
/// its own refusal string, wants [`PartialSplitExecutor`] instead.
fn run_with_split(
    req: &QueryRequest,
    layer: &SemanticLayer,
    inner: &QueryExecutor,
    deadline: std::time::Instant,
) -> Result<Vec<Row>, EngineError> {
    let (rows, failures) = split_groups(req, layer, inner, deadline, OnGroupFailure::Abandon)?;
    debug_assert!(
        failures.is_empty(),
        "the abandoning policy returns on the first failed group"
    );
    Ok(rows)
}

/// Wrap `inner` so a request refused for mixing additivity is retried split.
///
/// Applied once around the executor the scenario baseline hands to BOTH of its
/// reads, so the values query and the fit's panel query are covered by the
/// same rule — they batch for different reasons but break identically, and
/// fixing them separately is how the second one stayed broken after the first
/// was fixed.
///
/// Takes ownership rather than borrowing because airlayer's `QueryExecutor`
/// alias carries an implicit `'static` bound, so a wrapper that borrowed its
/// layer could not be passed back in as one.
///
/// `deadline` is checked twice, for two different fan-outs. First, in
/// [`split_groups`], before `req` is attempted AT ALL — this is what bounds
/// `grouped_values`' one call per view and `grouped_fit`'s one call per
/// group/pair, neither of which goes anywhere near the retry loop on the
/// common, unmixed path. Second, inside that retry loop itself, once a
/// request IS being split — a split issues one query per (view, additivity)
/// group under a single call to this executor, and nothing bounds that group
/// count but the workspace's own shape. Whichever check fires first stops the
/// call cold: the outer `tokio::time::timeout` wraps a `spawn_blocking` and
/// cannot cancel the thread, so anything this executor lets through keeps
/// querying the warehouse after the HTTP client has already been told the
/// request timed out.
///
/// Absolute rather than a duration because ONE executor serves both of
/// `baseline_reads`' reads: a per-call duration would give each of them the
/// full allowance, which is not a bound on the request.
pub fn splitting_executor(
    layer: SemanticLayer,
    inner: Box<QueryExecutor>,
    deadline: std::time::Instant,
) -> Box<QueryExecutor> {
    Box::new(move |req: &QueryRequest| run_with_split(req, &layer, &*inner, deadline))
}

/// The same split, for a caller that can survive a group it did not get.
///
/// [`splitting_executor`] is all-or-nothing because the scenario baseline
/// cannot tell a measure that was never successfully asked for from one the
/// warehouse returned nothing for. The projection can: it returns a panel of
/// independent curves and every one of them carries its OWN refusal string, so
/// a failed group costs exactly its own measures their curves and says why.
/// Discarding the halves that DID answer there is a strictly worse answer than
/// a partial one.
///
/// A struct rather than a `Box<QueryExecutor>` because both of the things such
/// a caller needs are outside what airlayer's executor signature can express:
/// WHICH groups failed (`run`), and whether the shared budget is already spent
/// (`out_of_budget`).
pub struct PartialSplitExecutor {
    layer: SemanticLayer,
    inner: Box<QueryExecutor>,
    deadline: std::time::Instant,
}

impl PartialSplitExecutor {
    /// `deadline` is when queries must stop — see [`splitting_executor`] for
    /// why it is absolute and shared rather than per call.
    pub fn new(
        layer: SemanticLayer,
        inner: Box<QueryExecutor>,
        deadline: std::time::Instant,
    ) -> Self {
        Self {
            layer,
            inner,
            deadline,
        }
    }

    /// Whether the shared budget is spent, so no further query may be issued.
    ///
    /// THE deadline the split itself checks between groups, exposed rather than
    /// copied: a caller looping over its own retries has to read the same clock,
    /// or it goes on issuing the queries the split has just stopped issuing —
    /// which is exactly what a second, private timer would let it do.
    pub fn out_of_budget(&self) -> bool {
        std::time::Instant::now() >= self.deadline
    }

    /// Run `req`, splitting a refused mixture, keeping the groups that answered.
    ///
    /// The `Err` case is unchanged from the all-or-nothing executor: a failure
    /// the split cannot fix, or a request already narrow enough that there is
    /// nothing to split. Only a group failing *within* a split is tolerated.
    pub fn run(&self, req: &QueryRequest) -> Result<(Vec<Row>, Vec<GroupFailure>), EngineError> {
        split_groups(
            req,
            &self.layer,
            &*self.inner,
            self.deadline,
            OnGroupFailure::Keep,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Airlayer's mixed-additivity refusal, verbatim from `sql_generator`.
    ///
    /// Copied rather than paraphrased because `is_additivity_refusal` matches
    /// on the text: a fixture with an invented message would let the matcher
    /// drift from the engine without a single test noticing.
    const ADDITIVITY_REFUSAL: &str = "Cannot combine additive (sum/count/min/max) and \
         non-additive (avg/count_distinct/median/number/custom/etc.) measures from view \
         'checks' in one query when a requested dimension, filter, or segment requires a \
         one-to-many join into that view — the additive measure(s) would be double-counted \
         by the fan-out. Query them in separate requests.";

    /// Airlayer's cross-grain refusal — the one a split must NOT try to fix.
    const NO_JOIN_PATH: &str = "No join path found between 'checks' and 'store_days'";

    /// Far enough out that no test is racing it; the deadline's own behaviour
    /// is asserted separately with one already past.
    fn far_deadline() -> std::time::Instant {
        std::time::Instant::now() + std::time::Duration::from_secs(300)
    }

    /// The example_new shape that started this: a `sum` leaf whose forward
    /// reach climbs through `custom` composites in the same view and then
    /// crosses a driver edge into a second view at a coarser grain.
    fn mixed_grain_layer() -> SemanticLayer {
        let checks = r#"
name: checks
table: public.checks
dialect: postgres
measures:
  - name: alcoholic_revenue
    type: sum
    expr: alcoholic_revenue
  - name: total_guests
    type: sum
    expr: party_size
  - name: net_revenue
    type: custom
    expr: "{{checks.alcoholic_revenue}}"
    drivers:
      - measure: checks.total_guests
        direction: positive
"#;
        let store_days = r#"
name: store_days
table: public.store_days
dialect: postgres
measures:
  - name: net_sales
    type: sum
    expr: net_sales
    drivers:
      - measure: checks.net_revenue
        direction: positive
"#;
        SemanticLayer::new(
            vec![
                oxy_airlayer_compat::parse_view_yaml(checks).unwrap(),
                oxy_airlayer_compat::parse_view_yaml(store_days).unwrap(),
            ],
            None,
        )
    }

    fn tree(layer: &SemanticLayer) -> MetricTree {
        oxy_semantic::build_metric_tree(layer)
    }

    fn req(measures: &[&str], dimensions: &[&str]) -> QueryRequest {
        QueryRequest {
            measures: measures.iter().map(|m| m.to_string()).collect(),
            dimensions: dimensions.iter().map(|d| d.to_string()).collect(),
            ..QueryRequest::new()
        }
    }

    /// One row per `(dim value)` per measure group, so a merge that fails to
    /// line the halves up is visible as a missing column rather than a
    /// coincidence.
    fn rows_for(measures: &[String], days: &[&str]) -> Vec<Row> {
        days.iter()
            .map(|day| {
                let mut row = Row::new();
                row.insert("checks__check_date".to_string(), serde_json::json!(*day));
                for (i, m) in measures.iter().enumerate() {
                    row.insert(m.replace('.', "__"), serde_json::json!(i as f64 + 1.0));
                }
                row
            })
            .collect()
    }

    /// Refuses any request mixing additive and non-additive measures, exactly
    /// as airlayer's fan-out guard does. Records every request it was handed.
    fn guarded(
        layer: SemanticLayer,
        seen: std::sync::Arc<Mutex<Vec<Vec<String>>>>,
        days: Vec<&'static str>,
    ) -> Box<QueryExecutor> {
        Box::new(move |r: &QueryRequest| {
            seen.lock().unwrap().push(r.measures.clone());
            let additive = r.measures.iter().any(|m| is_additive(&layer, m));
            let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
            if additive && non_additive {
                return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
            }
            Ok(rows_for(&r.measures, &days))
        })
    }

    #[test]
    fn forward_reachable_climbs_components_and_crosses_the_grain_bridge() {
        let layer = mixed_grain_layer();
        let reachable = forward_reachable(&tree(&layer), &["checks.total_guests".to_string()]);
        assert!(reachable.contains(&"checks.total_guests".to_string()));
        assert!(reachable.contains(&"checks.net_revenue".to_string()));
        assert!(reachable.contains(&"store_days.net_sales".to_string()));
        assert_eq!(reachable[0], "checks.total_guests");
        let unique: HashSet<&String> = reachable.iter().collect();
        assert_eq!(unique.len(), reachable.len());
    }

    #[test]
    fn additivity_follows_the_measure_type_the_guard_reads() {
        let layer = mixed_grain_layer();
        assert!(is_additive(&layer, "checks.total_guests"));
        assert!(is_additive(&layer, "store_days.net_sales"));
        assert!(!is_additive(&layer, "checks.net_revenue"));
        // Unknown ids fall to the conservative side rather than rejoining the
        // additive group and re-forming the mixture.
        assert!(!is_additive(&layer, "checks.nonexistent"));
        assert!(!is_additive(&layer, "nonexistent.measure"));
        assert!(!is_additive(&layer, "unqualified"));
    }

    #[test]
    fn a_refused_single_row_request_comes_back_whole() {
        // The baseline's shape: no dimensions, one row, every measure on it.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let rows = split(&req(
            &[
                "checks.total_guests",
                "checks.net_revenue",
                "store_days.net_sales",
            ],
            &[],
        ))
        .expect("the split recovers a mixture one query refuses");

        assert_eq!(rows.len(), 1);
        for alias in [
            "checks__total_guests",
            "checks__net_revenue",
            "store_days__net_sales",
        ] {
            assert!(
                rows[0].contains_key(alias),
                "missing {alias}: {:?}",
                rows[0]
            );
        }
        // The refused original, then one request per (view, additivity):
        // checks/additive, checks/non-additive, store_days/additive.
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 4, "{requests:?}");
        for measures in &requests[1..] {
            let additive = measures.iter().any(|m| is_additive(&layer, m));
            let non_additive = measures.iter().any(|m| !is_additive(&layer, m));
            assert!(
                !(additive && non_additive),
                "group still mixed: {measures:?}"
            );
            // The half that mattered: a group must never span two views, or it
            // re-creates the cross-grain join that corrupts both measures.
            let views: HashSet<&str> = measures
                .iter()
                .filter_map(|m| m.split_once('.').map(|(v, _)| v))
                .collect();
            assert_eq!(views.len(), 1, "group spans two views: {measures:?}");
        }
    }

    #[test]
    fn a_group_never_pairs_measures_from_two_views() {
        // The regression this cost a round to find. Splitting on additivity
        // alone put `checks.total_guests` (check grain) in one request with
        // `store_days.net_sales` (store grain); against real SQL that joined
        // the coarse view in on `location_id` alone and fanned BOTH measures
        // out — guests came back 3,100x inflated and identical on every day of
        // a panel, which the fit reports as "the driver does not vary".
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["d1", "d2"]);
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        split(&req(
            &[
                "checks.total_guests",
                "checks.net_revenue",
                "store_days.net_sales",
            ],
            &["checks.check_date"],
        ))
        .expect("recovers");

        let requests = seen.lock().unwrap().clone();
        let guests_with = requests
            .iter()
            .skip(1)
            .find(|m| m.iter().any(|x| x == "checks.total_guests"))
            .expect("guests was queried");
        assert_eq!(
            guests_with,
            &vec!["checks.total_guests".to_string()],
            "the check-grain sum must travel alone, not with a store-grain one"
        );
    }

    #[test]
    fn a_refused_panel_request_merges_row_for_row() {
        // The fit's shape, and the one that stayed broken after the baseline
        // was fixed: many rows, grouped by a dimension, both endpoints of each
        // candidate edge in one batch. Every row must carry BOTH halves'
        // measures or the fit sees no paired observations.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["d1", "d2", "d3"]);
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let rows = split(&req(
            &["checks.total_guests", "checks.net_revenue"],
            &["checks.check_date"],
        ))
        .expect("the panel query recovers too");

        assert_eq!(rows.len(), 3, "one row per day, not two halves stacked");
        for row in &rows {
            assert!(row.contains_key("checks__total_guests"));
            assert!(row.contains_key("checks__net_revenue"));
            assert!(row.contains_key("checks__check_date"));
        }
        // Rows lined up by their dimension value, not by position.
        let days: Vec<&str> = rows
            .iter()
            .map(|r| r["checks__check_date"].as_str().unwrap())
            .collect();
        assert_eq!(days, vec!["d1", "d2", "d3"]);
    }

    #[test]
    fn a_request_that_succeeds_is_never_split() {
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        split(&req(&["checks.total_guests", "store_days.net_sales"], &[]))
            .expect("an all-additive request is fine as it is");

        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the happy path must still cost exactly one round trip"
        );
    }

    #[test]
    fn an_unmixed_request_that_fails_keeps_its_error() {
        // Additivity was never what refused it, so a retry would just spend
        // two more round trips to fail the same way.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let calls = seen.clone();
        let inner: Box<QueryExecutor> = Box::new(move |r: &QueryRequest| {
            calls.lock().unwrap().push(r.measures.clone());
            Err(EngineError::QueryError("connection refused".to_string()))
        });
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let err = split(&req(&["checks.total_guests"], &[])).unwrap_err();
        assert!(err.to_string().contains("connection refused"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_already_spent_deadline_stops_a_query_that_never_needed_splitting() {
        // The gap this closes: `deadline` was only ever consulted INSIDE the
        // additivity-retry loop, so a request that succeeds on its first try —
        // the common case, and exactly what `grouped_values` issues once per
        // view and `grouped_fit` issues once per group/pair — never looked at
        // the clock at all. A workspace with enough views for the earlier
        // ones to exhaust `BASELINE_TIMEOUT` still let every later view's
        // query run to completion on the `spawn_blocking` thread the outer
        // `tokio::time::timeout` cannot cancel.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = splitting_executor(layer, inner, std::time::Instant::now());

        // A single additive measure never trips the additivity guard, so this
        // request succeeds outright — no refusal, no split, no retry loop,
        // and therefore (before this fix) no place that ever read `deadline`.
        let err = split(&req(&["checks.total_guests"], &[]))
            .expect_err("an already-spent budget must refuse the query, not run it");

        assert!(err.to_string().contains("budget"), "got: {err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            0,
            "the inner executor must not be called at all once the shared budget is \
             spent, even for a request that would have succeeded outright"
        );
    }

    #[test]
    fn the_shipped_budget_serves_the_whole_reference_fan_out() {
        // The companion to the two already-spent-deadline tests above: those
        // pin that the budget REFUSES, this pins that it does not refuse too
        // early. `baseline_reads` hands ONE executor to both of its reads and
        // every query either provokes — `V(V+3)/2` executor calls, 27 for the
        // six-view `example_new` — so a budget that is live for the first call
        // and spent by the twenty-seventh turns a wide workspace into partial
        // values. The number itself is derived in
        // `BASELINE_QUERY_BUDGET`; what this asserts is that the
        // production wiring hands the executor that budget rather than an
        // already-spent instant, and that the per-call check does not consume
        // any of it on its own.
        const REFERENCE_FAN_OUT: usize = 27;

        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = splitting_executor(layer, inner, baseline_query_deadline());

        for i in 0..REFERENCE_FAN_OUT {
            split(&req(&["checks.total_guests"], &[])).unwrap_or_else(|e| {
                panic!("call {i} of {REFERENCE_FAN_OUT} was refused under the shipped budget: {e}")
            });
        }
        assert_eq!(
            seen.lock().unwrap().len(),
            REFERENCE_FAN_OUT,
            "every call in the reference fan-out must reach the warehouse"
        );
    }

    #[test]
    fn a_passed_deadline_stops_the_split_rather_than_merging_part_of_it() {
        // The split costs one round trip per (view, additivity) group and
        // nothing bounds the group count but the workspace's shape. The outer
        // `tokio::time::timeout` wraps a `spawn_blocking` and cannot cancel
        // the thread, so it would report a timeout with every query still
        // running. A deadline of `now` stands in for "already spent".
        //
        // The request below WOULD need splitting (it mixes additivity), but
        // that must never be discovered: the budget is checked before `req`
        // is even attempted unsplit, so the mixture is irrelevant here — this
        // is the same refusal an already-spent budget produces for a request
        // that would have succeeded outright.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = splitting_executor(layer.clone(), inner, std::time::Instant::now());

        let err = split(&req(
            &[
                "checks.total_guests",
                "checks.net_revenue",
                "store_days.net_sales",
            ],
            &[],
        ))
        .unwrap_err();

        assert!(err.to_string().contains("budget"), "got: {err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            0,
            "no request at all; the read must not start past its deadline"
        );
    }

    #[test]
    fn a_cross_grain_refusal_is_not_retried_split() {
        // The reason the retry is keyed on the refusal and not on failure.
        // `store_days.view.yml` documents this exact request — a lever pinned
        // on the `checks` side reaches `store_days.net_sales`, the two views
        // have no join path, and the batch fails as a whole so the values come
        // back EMPTY and the impacts degrade to unquantifiable. That is the
        // safe, documented outcome.
        //
        // Split, it stops being safe: each half succeeds alone, and the
        // `store_days` half runs under `checks.check_date` — joining on
        // `location_id` with nothing tying the dates — so it returns a
        // whole-window total repeated on every date. The fit refuses that on
        // zero within-panel variance; the values path has no variance check,
        // so a documented empty would become a populated wrong number.
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let calls = seen.clone();
        let inner: Box<QueryExecutor> = Box::new(move |r: &QueryRequest| {
            calls.lock().unwrap().push(r.measures.clone());
            Err(EngineError::QueryError(NO_JOIN_PATH.to_string()))
        });
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        // The exact shape: two views, mixed additivity, so the split WOULD
        // have fired had it been keyed on failure rather than on the refusal.
        let err = split(&req(
            &[
                "checks.net_revenue",
                "checks.total_guests",
                "store_days.net_sales",
            ],
            &["checks.check_date"],
        ))
        .unwrap_err();

        assert!(err.to_string().contains("No join path"), "got: {err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "a no-join-path refusal must stay refused, not be split into halves \
             that each succeed on a partial key"
        );
    }

    #[test]
    fn a_half_that_still_fails_surfaces_the_original_error() {
        // Half an answer would leave the other half's measures looking like
        // columns the warehouse returned empty, which is a different and false
        // claim. The original refusal is the honest thing to report.
        let layer = mixed_grain_layer();
        let inner: Box<QueryExecutor> = {
            let layer = layer.clone();
            Box::new(move |r: &QueryRequest| {
                let additive = r.measures.iter().any(|m| is_additive(&layer, m));
                let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
                if additive && non_additive {
                    return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
                }
                if non_additive {
                    return Err(EngineError::QueryError("custom measures broke".to_string()));
                }
                Ok(rows_for(&r.measures, &["x"]))
            })
        };
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let err = split(&req(&["checks.total_guests", "checks.net_revenue"], &[])).unwrap_err();
        assert!(err.to_string().contains("non-additive"), "got: {err}");
        // ...and it must not stop there. The group's own error is raised
        // nowhere else, and a surface that reports only the mixture sends
        // triage after the wrong problem — which is exactly what a ClickHouse
        // type error hiding behind an additivity refusal did.
        assert!(
            err.to_string().contains("custom measures broke"),
            "the retry's own failure must be carried out with it: {err}"
        );
    }

    #[test]
    fn a_half_at_the_row_cap_is_not_merged() {
        // Two halves truncated at different boundaries would merge into rows
        // whose measures come from different groups — a silently misaligned
        // panel, which is worse than the refusal it replaces.
        let layer = mixed_grain_layer();
        let inner: Box<QueryExecutor> = {
            let layer = layer.clone();
            Box::new(move |r: &QueryRequest| {
                let additive = r.measures.iter().any(|m| is_additive(&layer, m));
                let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
                if additive && non_additive {
                    return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
                }
                Ok(rows_for(&r.measures, &["d1", "d2"]))
            })
        };
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let capped = QueryRequest {
            limit: Some(2),
            ..req(
                &["checks.total_guests", "checks.net_revenue"],
                &["checks.check_date"],
            )
        };
        let err = split(&capped).unwrap_err();
        assert!(err.to_string().contains("non-additive"), "got: {err}");
    }

    #[test]
    fn the_rest_of_the_request_reaches_both_halves() {
        // A half that dropped the window or the scope would value a different
        // population than the one asked for, and the merge would hide it.
        let layer = mixed_grain_layer();
        let captured = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let inner: Box<QueryExecutor> = {
            let layer = layer.clone();
            Box::new(move |r: &QueryRequest| {
                sink.lock().unwrap().push(r.clone());
                let additive = r.measures.iter().any(|m| is_additive(&layer, m));
                let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
                if additive && non_additive {
                    return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
                }
                Ok(rows_for(&r.measures, &["d1"]))
            })
        };
        let split = splitting_executor(layer.clone(), inner, far_deadline());

        let original = QueryRequest {
            filters: vec![oxy_airlayer_compat::engine::query::QueryFilter {
                member: Some("locations.region".to_string()),
                operator: Some(oxy_airlayer_compat::engine::query::FilterOperator::Equals),
                values: vec!["west".to_string()],
                and: None,
                or: None,
            }],
            limit: Some(1_000_000),
            ..req(
                &["checks.total_guests", "checks.net_revenue"],
                &["checks.check_date"],
            )
        };
        split(&original).expect("values come back");

        let requests = captured.lock().unwrap().clone();
        assert_eq!(requests.len(), 3);
        for r in &requests[1..] {
            assert_eq!(r.dimensions, vec!["checks.check_date".to_string()]);
            assert_eq!(r.filters.len(), 1);
            assert_eq!(r.filters[0].member.as_deref(), Some("locations.region"));
            assert_eq!(r.limit, Some(1_000_000));
        }
    }
    /// FINDING 1, at this layer: the all-or-nothing rule is the BASELINE's
    /// need, not the split's. A caller that attributes a loss per measure gets
    /// the halves that answered instead of nothing.
    #[test]
    fn a_partial_caller_keeps_the_half_that_answered() {
        let layer = mixed_grain_layer();
        let inner: Box<QueryExecutor> = {
            let layer = layer.clone();
            Box::new(move |r: &QueryRequest| {
                let additive = r.measures.iter().any(|m| is_additive(&layer, m));
                let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
                if additive && non_additive {
                    return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
                }
                if non_additive {
                    return Err(EngineError::QueryError("custom measures broke".to_string()));
                }
                Ok(rows_for(&r.measures, &["x"]))
            })
        };
        let split = PartialSplitExecutor::new(layer, inner, far_deadline());

        let (rows, failed) = split
            .run(&req(&["checks.total_guests", "checks.net_revenue"], &[]))
            .expect("the group that answered survives its sibling's failure");

        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains_key("checks__total_guests"));
        // ...and the one that failed is absent rather than present-and-empty:
        // an empty series with no reason is what renders as a zero.
        assert!(!rows[0].contains_key("checks__net_revenue"));

        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(failed[0].measures, vec!["checks.net_revenue".to_string()]);
        assert!(
            failed[0].reason.contains("custom measures broke"),
            "the group's own error is what names why: {}",
            failed[0].reason
        );
    }

    /// The all-or-nothing caller must NOT have moved: its measures carry no
    /// individual refusal, so half an answer there is a wrong number.
    #[test]
    fn the_all_or_nothing_caller_still_gets_nothing() {
        let layer = mixed_grain_layer();
        let inner: Box<QueryExecutor> = {
            let layer = layer.clone();
            Box::new(move |r: &QueryRequest| {
                let additive = r.measures.iter().any(|m| is_additive(&layer, m));
                let non_additive = r.measures.iter().any(|m| !is_additive(&layer, m));
                if additive && non_additive {
                    return Err(EngineError::QueryError(ADDITIVITY_REFUSAL.to_string()));
                }
                if non_additive {
                    return Err(EngineError::QueryError("custom measures broke".to_string()));
                }
                Ok(rows_for(&r.measures, &["x"]))
            })
        };
        let split = splitting_executor(layer, inner, far_deadline());

        let err = split(&req(&["checks.total_guests", "checks.net_revenue"], &[])).unwrap_err();
        assert!(err.to_string().contains("non-additive"), "got: {err}");
    }

    /// FINDING 2, at this layer: a spent budget is reported as a spent budget.
    /// "The warehouse rejected it" would send triage after a query that was
    /// never issued.
    ///
    /// The per-GROUP attribution (one [`GroupFailure`] per view/additivity
    /// group) is a property of the caller that already knows the groups —
    /// `read_series` in `metric_tree_projection`, which checks
    /// [`PartialSplitExecutor::out_of_budget`] itself before issuing each
    /// view's own request and can therefore name every one of them. At THIS
    /// layer the budget is spent before `req` is even attempted unsplit, so
    /// there is no group list to attribute a failure to yet — the honest
    /// answer is one refusal for the whole call, not three guesses at what
    /// the split would have grouped had it run.
    #[test]
    fn a_partial_split_past_its_deadline_names_the_budget() {
        let layer = mixed_grain_layer();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let inner = guarded(layer.clone(), seen.clone(), vec!["x"]);
        let split = PartialSplitExecutor::new(layer, inner, std::time::Instant::now());

        let err = split
            .run(&req(
                &[
                    "checks.total_guests",
                    "checks.net_revenue",
                    "store_days.net_sales",
                ],
                &[],
            ))
            .expect_err("a spent budget refuses the whole call, not a bare warehouse error");

        assert!(err.to_string().contains("budget"), "got: {err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            0,
            "no request at all; the read must not start past its deadline"
        );
    }
}
