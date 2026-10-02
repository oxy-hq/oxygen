//! Ask the shipped model what it makes of the world so far.
//!
//! This is the thing under test, and there is deliberately nothing
//! simulation-shaped in it: the layer is parsed from the materialised
//! workspace by the real parser, the SQL is compiled by the real
//! `SemanticEngine`, and the coefficient comes from the same
//! `fit_driver_coefficients` a scenario panel calls. What the simulation
//! supplies is the rows and the question — never the answer.
//!
//! # Both halves of a baseline, because a coefficient is not enough
//!
//! A real baseline runs two reads, not one: `reachable_values_outcome` for the
//! current level of every reachable measure, and `fit_driver_coefficients` for
//! the slopes. The first is not optional here. The fitter picks a basis by AIC,
//! so an edge that declares nothing comes back **log-log** — an elasticity —
//! and `with_profile` needs the target's current aggregate before it can sample
//! the response into units a policy can act on. Skipping it once cost a factor
//! of ~43 that looked exactly like the fitter being wrong.
//!
//! The executor is built here rather than borrowed from
//! `agentic_wiring::build_query_executor` because that one exists to resolve
//! *secrets*, pool connectors and consult the pre-agg cache across a real
//! workspace. A run's warehouse is a directory of CSVs with no credentials, so
//! all of that machinery would be inert — and none of it is what a run is
//! measuring.

use std::sync::Arc;

use oxy_airlayer_compat::engine::metric_tree::{MetricEdge, MetricTree};
use oxy_airlayer_compat::engine::metric_tree_fit::{fit_driver_coefficients, fit_panel_dimensions};
use oxy_airlayer_compat::engine::metric_tree_ops::{QueryExecutor, reachable_values_outcome};
use oxy_airlayer_compat::schema::models::DriverForm;
use oxy_simulation::{EdgeFit, FitForm, Probe, SemanticProbe, SimulationError, SimulationSpec};
use serde_json::{Map, Value};

use super::world_dir::{DATASOURCE, TABLE, WorldDir};

/// The window a fit is run over. Wide enough to cover any declared world's
/// history plus its horizon — the world's own `history_days` is the real
/// bound, and narrowing here would silently truncate a fit's sample.
const WINDOW_START: &str = "1900-01-01";
const WINDOW_END: &str = "2999-12-31";

pub struct FitProbe {
    layer: oxy_airlayer_compat::SemanticLayer,
    tree: MetricTree,
    /// `Arc` because `SemanticEngine` is not `Clone` and the executor closure
    /// needs it by value. One engine per run, shared by every period's query.
    engine: Arc<oxy_airlayer_compat::SemanticEngine>,
    dataset_dir: String,
    roots: Vec<String>,
    /// The declared world's name, carried solely so a degraded or failed
    /// values read can say *which* run it happened in — a warn line naming
    /// only the measure is unactionable when a suite runs a dozen worlds.
    world: String,
}

/// What a values read's outcome means for the run.
///
/// # Why this is a decision and not a discard
///
/// The values read is not optional (see the module docstring), and every
/// consequence of losing it is silent. Empty `values` gives every `EdgeFit` a
/// `driver_value: None` and a `None` profile target; `EdgeFit::level_slope`
/// then returns `None` for any non-linear form; `Machine::direction` returns
/// `None` and the arm **holds**; and `Outcome::classify` scores the period
/// `Refused`. So a broken read produces a run that completes cleanly and
/// stores `outcome = 'refused'` with `refusal` NULL — indistinguishable from
/// the model honestly declining, which is the one distinction this crate
/// exists to measure.
///
/// Split out of [`SemanticProbe::probe`] so every variant can be asserted
/// without a warehouse.
#[derive(Debug, PartialEq, Eq)]
enum ValuesVerdict {
    /// Every reachable measure was valued. The ordinary case.
    Complete,
    /// The read ran, but came back short. The run continues — a partial basis
    /// still measures something — and the string names what was lost.
    Degraded(String),
    /// The read never produced rows at all. A run whose levels never arrived
    /// is not measuring the product, so it must fail rather than score.
    Fatal(String),
}

/// Classify a values read, without deciding what to do about it.
///
/// `BaselineOutcome` is `#[non_exhaustive]`, so a variant added upstream lands
/// in the wildcard. It is treated as `Degraded` rather than `Complete`: an
/// unknown outcome is by definition not a clean read, and a warn line on a
/// healthy run is a far cheaper mistake than a silent `refused`.
fn classify_values(
    outcome: &oxy_airlayer_compat::engine::metric_tree_ops::BaselineOutcome,
) -> ValuesVerdict {
    use oxy_airlayer_compat::engine::metric_tree_ops::BaselineOutcome as O;

    match outcome {
        O::Valued { unreadable } if unreadable.is_empty() => ValuesVerdict::Complete,
        O::Valued { unreadable } => ValuesVerdict::Degraded(format!(
            "{} came back unreadable (not a number) and were left unvalued",
            unreadable.join(", ")
        )),
        O::ExecutorError(msg) => {
            ValuesVerdict::Fatal(format!("the warehouse rejected the query: {msg}"))
        }
        // Not fatal, deliberately: an empty window is a legitimate state for
        // an early period of a short world, and failing the run would make a
        // warm-up artefact look like an engine bug.
        O::NoRows => ValuesVerdict::Degraded(
            "no rows in the fit window — every level is unknown, so the arm can only hold"
                .to_string(),
        ),
        O::NoMatchingColumns => ValuesVerdict::Degraded(
            "the query returned rows but none carried these measures — the world's view and \
             its metric tree disagree on names"
                .to_string(),
        ),
        O::UnreadableValues(ids) => ValuesVerdict::Degraded(format!(
            "the query returned rows, but {} held values that are not numbers",
            ids.join(", ")
        )),
        // Nothing was reachable, so nothing was asked. Distinct from `NoRows`:
        // the tree, not the window, is what came back empty.
        O::NothingRequested => ValuesVerdict::Degraded(
            "nothing was reachable from the run's roots, so no level was requested".to_string(),
        ),
        _ => ValuesVerdict::Degraded(format!("unrecognised values outcome: {outcome:?}")),
    }
}

impl FitProbe {
    /// Parse the materialised workspace and build everything a fit needs, once.
    ///
    /// Once, not per period: the layer and the tree are fixed for a run, and
    /// re-parsing each period would make the loop's cost quadratic in the
    /// horizon for no gain. The *rows* are what change, and those are re-read
    /// on every query because the pool evicts on mtime.
    pub fn new(world: &WorldDir, spec: &SimulationSpec) -> Result<Self, SimulationError> {
        // The same loader `OxyMetricTreeRunner` uses, so the run reads its
        // world through the path a customer's workspace takes.
        let layer = oxy_airlayer_compat::load_layer_from_dir(world.root())
            .map_err(|e| SimulationError::Read(format!("parse the world's semantic layer: {e}")))?;
        let tree = MetricTree::build(&layer);
        // Built from the run's `databases:` exactly as the scenario panel does.
        // `Default::default()` looks harmless and is not: with no dialect for
        // `sim` the engine cannot generate SQL, and the failure surfaces as a
        // *refusal* on the edge — "panel query failed" — which reads as the
        // world being unidentified rather than as the engine never having run.
        let dialects = oxy_airlayer_compat::DatasourceDialectMap::from_config_databases(&[
            oxy_airlayer_compat::DatabaseConfig {
                name: DATASOURCE.to_string(),
                db_type: "duckdb".to_string(),
            },
        ]);
        let engine =
            oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer.clone(), dialects)
                .map_err(|e| SimulationError::Read(format!("build the semantic engine: {e}")))?;

        Ok(Self {
            layer,
            tree,
            engine: Arc::new(engine),
            dataset_dir: world.dataset_dir().to_string_lossy().into_owned(),
            // The **lever**, not the target. `fittable_edges` walks *forward*
            // from its roots — a lever's delta has to actually cross the edge —
            // so rooting on the target finds nothing and the run reports a
            // world with no fittable edges rather than an error.
            roots: vec![format!("{TABLE}.{}", spec.mechanism.driver)],
            world: spec.name.clone(),
        })
    }

    fn executor(&self) -> Box<QueryExecutor> {
        let engine = Arc::clone(&self.engine);
        let dataset_dir = self.dataset_dir.clone();
        Box::new(move |request| {
            let compiled = engine.compile_query(request)?;
            let sql = oxy_airlayer_compat::substitute_params(
                &oxy_airlayer_compat::request_dialect(&engine, request),
                &compiled.sql,
                &compiled.params,
            );
            run_local_duckdb(&dataset_dir, &sql).map_err(|e| {
                oxy_airlayer_compat::engine::EngineError::QueryError(format!(
                    "simulation warehouse: {e}"
                ))
            })
        })
    }
}

impl SemanticProbe for FitProbe {
    fn probe(&mut self) -> Result<Probe, SimulationError> {
        let edges: Vec<&MetricEdge> =
            oxy_airlayer_compat::engine::metric_tree_fit::fittable_edges(&self.tree, &self.roots);
        let panel_dimensions = fit_panel_dimensions(&self.layer, &edges);
        let executor = self.executor();
        let time_dimension = format!("{TABLE}.date");

        // The target's and lever's current aggregates. Same call, same
        // executor, same window as the fit — so the profile is sampled at the
        // level the world is actually sitting at.
        let (values, outcome) = reachable_values_outcome(
            &self.tree,
            &self.roots,
            &time_dimension,
            (WINDOW_START, WINDOW_END),
            &[],
            &executor,
        );
        // Never discarded: see `ValuesVerdict` for what losing this read costs.
        match classify_values(&outcome) {
            ValuesVerdict::Complete => {}
            ValuesVerdict::Degraded(what) => tracing::warn!(
                world = %self.world,
                roots = ?self.roots,
                "simulation values read came back short: {what}"
            ),
            ValuesVerdict::Fatal(what) => {
                return Err(SimulationError::Read(format!(
                    "read the world's current levels: {what}"
                )));
            }
        }

        let fitted = fit_driver_coefficients(
            &self.tree,
            &self.roots,
            &panel_dimensions,
            &time_dimension,
            (WINDOW_START, WINDOW_END),
            &[],
            &executor,
        )
        .map_err(|e| SimulationError::Read(format!("fit: {e}")))?
        .into_iter()
        // Sample each response here, exactly as the scenario baseline does:
        // this is the only place that holds both the fit and the target's
        // current aggregate, and a log link needs the latter.
        .map(|f| {
            let target = values.get(&f.to).copied();
            let space = crate::server::api::metric_tree_groups::space_of(&self.tree, &f.to);
            f.with_profile(target, space)
        })
        .collect::<Vec<_>>();

        Ok(Probe {
            fits: fitted
                .into_iter()
                .map(|f| {
                    (
                        format!("{} -> {}", f.from, f.to),
                        EdgeFit {
                            coefficient: f.coefficient,
                            form: if f.form == DriverForm::Linear {
                                FitForm::Linear
                            } else {
                                FitForm::NonLinear
                            },
                            form_name: f.form.to_string(),
                            driver_value: values.get(&f.from).copied(),
                            profile: f.profile,
                            se: f.se,
                            t_stat: f.t_stat,
                            n: f.n,
                            n_panels: f.n_panels,
                            refusal: f.refusal,
                        },
                    )
                })
                .collect(),
            // Phase 1 carries one linear edge off the root, so an impact is
            // sized whenever the fit produced a coefficient. A world with a
            // multiplicative parent is where this stops being true, and it is
            // the reason the flag exists rather than being assumed.
            impact_quantified: true,
        })
    }
}

/// Run one compiled query against the run's dataset directory.
///
/// Goes through `checkout_local_connection` — the pooled path — rather than
/// opening DuckDB directly, because the pool is what evicts the cached
/// in-memory copy when a period's rows land. Opening our own handle would read
/// a frozen world, and a frozen series is still a well-formed series: the
/// fitter would report a confidently converging estimate of a world that
/// stopped moving.
fn run_local_duckdb(dataset_dir: &str, sql: &str) -> Result<Vec<Map<String, Value>>, String> {
    let conn = oxy::connector::checkout_local_connection(dataset_dir).map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

    // Column names are read off the executed statement, not the prepared one:
    // DuckDB does not know the result schema until the query has run, and
    // asking early panics inside the driver rather than returning an error.
    let mut column_names: Vec<String> = Vec::new();
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if column_names.is_empty() {
            column_names = row
                .as_ref()
                .column_names()
                .iter()
                .map(|c| c.to_string())
                .collect();
        }
        let mut map = Map::new();
        for (i, name) in column_names.iter().enumerate() {
            map.insert(name.clone(), cell_to_json(row, i));
        }
        out.push(map);
    }
    Ok(out)
}

/// DuckDB cell → JSON, in the shape airlayer's fitter expects.
///
/// Matched on DuckDB's own type rather than attempted conversions in order.
/// That is not style: `row.get::<f64>` on a DATE **succeeds**, handing back the
/// epoch day as a number, and the fitter then has no string to pair day `d`
/// with day `d + lag` on. Every edge comes back refused for "0 paired
/// observations" — a refusal that reads as an unidentified world rather than as
/// a type bug three layers down.
///
/// Anything unrecognised becomes `null` rather than a guess: a mis-typed cell
/// coerced to 0.0 would land in the regression as a real observation.
fn cell_to_json(row: &duckdb::Row<'_>, i: usize) -> Value {
    use duckdb::types::Value as Duck;
    let Ok(value) = row.get::<_, Duck>(i) else {
        return Value::Null;
    };
    match value {
        Duck::Null => Value::Null,
        Duck::Boolean(v) => Value::Bool(v),
        Duck::TinyInt(v) => Value::from(v),
        Duck::SmallInt(v) => Value::from(v),
        Duck::Int(v) => Value::from(v),
        Duck::BigInt(v) => Value::from(v),
        Duck::UTinyInt(v) => Value::from(v),
        Duck::USmallInt(v) => Value::from(v),
        Duck::UInt(v) => Value::from(v),
        Duck::UBigInt(v) => Value::from(v),
        Duck::Float(v) => json_number(v as f64),
        Duck::Double(v) => json_number(v),
        Duck::Text(v) => Value::String(v),
        Duck::Date32(days) => epoch_day_to_iso(days),
        _ => Value::Null,
    }
}

/// DuckDB's DATE is days since the Unix epoch; the fitter wants an ISO date.
fn epoch_day_to_iso(days: i32) -> Value {
    chrono::DateTime::from_timestamp(days as i64 * 86_400, 0)
        .map(|dt| Value::String(dt.date_naive().to_string()))
        .unwrap_or(Value::Null)
}

fn json_number(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests;
