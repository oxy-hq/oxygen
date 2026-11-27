//! `query`: the agent's only path to a customer warehouse.
//!
//! Runs one read statement (raw `sql`, a `semantic` query compiled by the
//! semantic model, or a `verified` query run as written) behind the read-only
//! [`gate`] and then the connector's database-level read-only path
//! (`execute_query_read_only`), stores the rows as `r<N>`, and leaves the same
//! `query_executed` row the FSM writes for each execution.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentic_connector::{DatabaseConnector, ExecutionResult};
use agentic_core::events::Event;
use agentic_core::react::{Artifact, Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::result::CellValue;
use agentic_core::tools::ToolDef;
use agentic_semantic::config::SemanticQueryConfig;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::semantic::{
    SEMANTIC_SHAPE_HINT, compile_semantic, semantic_datasource, semantic_parameter,
};
use crate::react::read_only::{Refusal, gate, sqlparser_dialect};
use crate::react::render::render_table;
use crate::react::verified::VerifiedQueries;
use crate::solver::lookup_connector;
use crate::{AnalyticsEvent, QuerySource, SemanticCatalog};

/// The most rows a result keeps: the platform's cap. The model sees only its
/// `limit`; the rest are for the answer's table.
const SAMPLE_LIMIT: u64 = 10_000;
/// Rows a `query_executed` row carries: enough for a glance in the trace.
/// The rest are in the stored result, which `result_table` pages
/// (`GET …/runs/{id}/results/{result_id}`); `row_count` is always the full
/// count, so a reader says "first 10 of N".
pub const EVENT_PREVIEW_ROWS: usize = 10;
const DEFAULT_SHOWN: usize = 20;
const MAX_SHOWN: usize = 100;
const MAX_CANDIDATES: usize = 5;
/// Below this Jaro-Winkler score a name is not offered as a candidate.
const MIN_SIMILARITY: f64 = 0.7;
/// The hint when the gate refuses a verified query: the model cannot fix
/// SQL it may not edit.
const VERIFIED_REFUSED_HINT: &str = "this verified query cannot run here; answer with your \
    own `sql` or `semantic` query";

pub struct QueryDeps {
    pub connectors: HashMap<String, Arc<dyn DatabaseConnector>>,
    pub catalog: Option<Arc<SemanticCatalog>>,
    /// Used when `query` names no `database` and several are configured.
    pub default_connector: Option<String>,
    /// Refuse raw `sql`; only `semantic` queries run (the agent's
    /// `semantic_only`). A `verified` query still runs: the model did not
    /// write it.
    pub semantic_only: bool,
    /// What `verified` may name; empty drops the argument from the schema.
    pub verified: Arc<VerifiedQueries>,
}

pub struct QueryTool {
    deps: QueryDeps,
}

/// The statement to run and the provenance its `query_executed` row records.
struct Prepared {
    sql: String,
    source: QuerySource,
    /// The connector the statement must run on: a semantic query's views'
    /// datasource, or a verified query's `database:`. `None` for raw `sql`
    /// and when neither names one.
    datasource: Option<String>,
}

impl Prepared {
    /// Compiled from a semantic query or read from a verified query's file,
    /// so the model may not have seen this SQL: the reply shows it.
    fn shows_sql(&self) -> bool {
        matches!(
            self.source,
            QuerySource::Semantic | QuerySource::VerifiedSql
        )
    }

    /// What named `datasource`, for a routing refusal.
    fn routed_by(&self) -> &'static str {
        match self.source {
            QuerySource::VerifiedSql => "the verified query's `database:`",
            _ => "the views' datasource",
        }
    }
}

impl QueryTool {
    pub fn new(deps: QueryDeps) -> Self {
        Self { deps }
    }

    /// The connector a statement runs on. A semantic query's views' datasource,
    /// or a verified query's `database:`, decides (the FSM's routing); the
    /// model's `database` may only agree with it. Otherwise the named
    /// connector, else the only one, else the agent's default.
    fn pick_connector(
        &self,
        p: &Prepared,
        requested: Option<&str>,
    ) -> Result<&Arc<dyn DatabaseConnector>, ToolFailure> {
        let Some(ds) = p.datasource.as_deref() else {
            return self.connector(requested);
        };
        let by = p.routed_by();
        let (registered, connector) = lookup_connector(&self.deps.connectors, ds)
            .ok_or_else(|| failure(&format!("no database {ds}, named by {by}")))?;
        match requested {
            Some(r) if !r.eq_ignore_ascii_case(registered) => Err(failure(&format!(
                "this query runs on {registered}, named by {by}; `database` {r} disagrees"
            ))),
            _ => Ok(connector),
        }
    }

    /// The named connector, else the only one, else the default.
    fn connector(&self, name: Option<&str>) -> Result<&Arc<dyn DatabaseConnector>, ToolFailure> {
        let mut names: Vec<&str> = self.deps.connectors.keys().map(String::as_str).collect();
        names.sort_unstable();
        match name {
            Some(n) => self.deps.connectors.get(n).ok_or_else(|| ToolFailure {
                error: format!("no database {n}"),
                hint: None,
                candidates: closest(n, names),
            }),
            None => match names.as_slice() {
                [only] => Ok(&self.deps.connectors[*only]),
                [] => Err(failure("no database is configured for this agent")),
                _ if self.default().is_some() => Ok(self.default().unwrap()),
                _ => Err(ToolFailure {
                    error: "database is required".into(),
                    hint: Some("pass `database`, one of the candidates".into()),
                    candidates: names.into_iter().map(str::to_owned).collect(),
                }),
            },
        }
    }

    fn default(&self) -> Option<&Arc<dyn DatabaseConnector>> {
        let name = self.deps.default_connector.as_deref()?;
        lookup_connector(&self.deps.connectors, name).map(|(_, c)| c)
    }

    /// A `verified` query, else exactly one of `sql` and `semantic`.
    fn prepare(&self, args: &Value) -> Result<Prepared, ToolFailure> {
        match args.get("verified").filter(|v| !v.is_null()) {
            Some(id) => self.prepare_verified(id, args),
            None => self.prepare_written(args),
        }
    }

    /// The verified query's SQL as written, on its declared database.
    fn prepare_verified(&self, id: &Value, args: &Value) -> Result<Prepared, ToolFailure> {
        let q = self.deps.verified.resolve(id, args)?;
        Ok(Prepared {
            sql: q.sql.clone(),
            source: QuerySource::VerifiedSql,
            datasource: q.database.clone(),
        })
    }

    /// Exactly one of `sql` and `semantic`; a semantic query compiles here.
    fn prepare_written(&self, args: &Value) -> Result<Prepared, ToolFailure> {
        let sql = args.get("sql").and_then(Value::as_str);
        let semantic = args
            .get("semantic")
            .filter(|v| !v.is_null())
            .or_else(|| flattened_semantic(args));
        match (sql, semantic) {
            (Some(_), _) if self.deps.semantic_only => Err(ToolFailure {
                error: "this agent queries through the semantic model only; raw `sql` is refused"
                    .into(),
                hint: Some(
                    "pass `semantic` naming members as `view.member`; read the views to find \
                     them"
                        .into(),
                ),
                candidates: Vec::new(),
            }),
            (Some(sql), None) => Ok(Prepared {
                sql: sql.to_owned(),
                source: QuerySource::Llm,
                datasource: None,
            }),
            (None, Some(v)) => {
                let catalog = self
                    .deps
                    .catalog
                    .as_deref()
                    .ok_or_else(|| failure("this agent has no semantic model; use `sql`"))?;
                let config: SemanticQueryConfig =
                    serde_json::from_value(v.clone()).map_err(|e| ToolFailure {
                        error: format!("invalid semantic query: {e}"),
                        hint: Some(SEMANTIC_SHAPE_HINT.into()),
                        candidates: Vec::new(),
                    })?;
                // Before compiling: views on two datasources have no one SQL.
                let datasource = semantic_datasource(catalog, &config)?;
                let sql = compile_semantic(catalog, &config)?;
                Ok(Prepared {
                    sql,
                    source: QuerySource::Semantic,
                    datasource,
                })
            }
            _ => Err(failure(&format!(
                "give exactly one of `sql` and `semantic`; got {}",
                received_keys(args)
            ))),
        }
    }
}

/// The arguments themselves, when they are a semantic query sent without the
/// `semantic` wrapper: no `sql` or `semantic` key, and a field that names
/// members. Models send this shape on a first call often enough to matter.
fn flattened_semantic(args: &Value) -> Option<&Value> {
    let obj = args.as_object()?;
    let names_members = ["topic", "measures", "dimensions", "time_dimensions"]
        .iter()
        .any(|k| obj.contains_key(*k));
    (names_members && !obj.contains_key("sql") && !obj.contains_key("semantic")).then_some(args)
}

/// What a call that gave neither (or both) sent, so the model and a reader of
/// `tool_errors` see the shape it tried: top-level keys, and the JSON type of
/// `semantic` when it is not an object.
fn received_keys(args: &Value) -> String {
    let Some(obj) = args.as_object().filter(|o| !o.is_empty()) else {
        return "no arguments".into();
    };
    let keys: Vec<String> = obj
        .iter()
        .map(|(k, v)| match v {
            Value::Object(_) => k.clone(),
            Value::String(_) => format!("{k} (a string)"),
            Value::Null => format!("{k} (null)"),
            _ => format!(
                "{k} ({})",
                if v.is_array() { "an array" } else { "a scalar" }
            ),
        })
        .collect();
    format!("keys: {}", keys.join(", "))
}

/// `verified`: the id of a verified query the system prompt lists.
fn verified_parameter() -> Value {
    json!({
        "type": "string",
        "description": "Instead of `sql` or `semantic`: the id of a verified query from the \
            system prompt. Its SQL runs as written, on the database it names."
    })
}

/// A gate refusal as the model sees it; a verified query's says to answer
/// another way, since its SQL may not be edited.
fn gate_failure(p: &Prepared, r: &Refusal) -> ToolFailure {
    let hint = match p.source {
        QuerySource::VerifiedSql => VERIFIED_REFUSED_HINT,
        _ => r.hint(),
    };
    ToolFailure {
        error: r.message().to_owned(),
        hint: Some(hint.into()),
        candidates: Vec::new(),
    }
}

fn failure(error: &str) -> ToolFailure {
    ToolFailure {
        error: error.to_owned(),
        hint: None,
        candidates: Vec::new(),
    }
}

/// Names in `pool` similar to `name` (Jaro-Winkler at least
/// [`MIN_SIMILARITY`]), best first; empty when none is close.
pub(super) fn closest<'a>(name: &str, pool: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut scored: Vec<(f64, &str)> = pool
        .into_iter()
        .map(|p| (strsim::jaro_winkler(name, p), p))
        .filter(|(score, _)| *score >= MIN_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, p)| p.to_owned())
        .collect()
}

/// Rows shown: default 20, above 100 clamped to 100; not a positive integer
/// is an error.
fn shown_limit(args: &Value) -> Result<usize, ToolFailure> {
    match args.get("limit").filter(|v| !v.is_null()) {
        None => Ok(DEFAULT_SHOWN),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok((n as usize).min(MAX_SHOWN)),
            _ => Err(failure("`limit` must be a positive integer")),
        },
    }
}

/// Cells as the FSM's executing stage converts them.
fn json_rows(exec: &ExecutionResult) -> Vec<Vec<Value>> {
    exec.result
        .rows
        .iter()
        .map(|row| {
            row.0
                .iter()
                .map(|cell| match cell {
                    CellValue::Text(s) => Value::String(s.clone()),
                    CellValue::Number(n) => json!(n),
                    CellValue::Null => Value::Null,
                })
                .collect()
        })
        .collect()
}

/// The `query_executed` row, serialized as the FSM's event is.
fn query_executed(
    p: &Prepared,
    duration_ms: u64,
    outcome: Result<(Vec<String>, Vec<Vec<Value>>, usize), String>,
) -> Artifact {
    let (success, error, (columns, rows, row_count)) = match outcome {
        Ok(data) => (true, None, data),
        Err(e) => (false, Some(e), (Vec::new(), Vec::new(), 0)),
    };
    let event = AnalyticsEvent::QueryExecuted {
        query: p.sql.clone(),
        row_count,
        duration_ms,
        success,
        error,
        columns,
        rows,
        source: p.source,
        is_preagg: false,
        sub_spec_index: None,
        semantic_query: None,
    };
    let (event_type, payload) = Event::Domain(event).serialize();
    Artifact {
        event_type,
        payload,
    }
}

fn success_reply(
    p: Prepared,
    exec: ExecutionResult,
    duration_ms: u64,
    limit: usize,
    cx: &ToolCx,
) -> ToolReply {
    let rows = json_rows(&exec);
    let columns = exec.result.columns.clone();
    let row_count = rows.len();
    let table = render_table(&columns, &rows, limit);
    let preview: Vec<Vec<Value>> = rows.iter().take(EVENT_PREVIEW_ROWS).cloned().collect();
    let artifact = query_executed(&p, duration_ms, Ok((columns.clone(), preview, row_count)));
    let id = cx.results.put(p.sql.clone(), columns, rows);
    let mut text = format!("result {id}: {}", row_count_text(&exec, row_count));
    if p.shows_sql() {
        text.push_str(&format!("\nsql: {}", p.sql));
    }
    text.push('\n');
    text.push_str(&table);
    ToolReply {
        text,
        value: json!({"result_id": id, "sql": p.sql}),
        artifacts: vec![artifact],
    }
}

/// `<n> rows`; when the connector cut the result, `<n> of <total> rows
/// (truncated)` if it counted the whole result, else `<n> rows (truncated)`.
fn row_count_text(exec: &ExecutionResult, shown: usize) -> String {
    let total = exec.result.total_row_count;
    match exec.result.truncated {
        false => format!("{shown} rows"),
        true if total > shown as u64 => format!("{shown} of {total} rows (truncated)"),
        true => format!("{shown} rows (truncated)"),
    }
}

/// An executed query that failed: `Ok` to the loop, with the FSM's failed
/// `query_executed` row, so only validation errors are `Err`.
fn failed_reply(p: Prepared, error: String, duration_ms: u64) -> ToolReply {
    let message = error.strip_prefix("query failed: ").unwrap_or(&error);
    let mut text = format!("query failed: {message}");
    if p.shows_sql() {
        text.push_str(&format!("\nsql: {}", p.sql));
    }
    let artifact = query_executed(&p, duration_ms, Err(error));
    ToolReply {
        text,
        value: Value::Null,
        artifacts: vec![artifact],
    }
}

#[async_trait]
impl Tool for QueryTool {
    fn def(&self) -> ToolDef {
        let mut properties = json!({
            "sql": {"type": "string"},
            "semantic": semantic_parameter(),
            "database": {"type": "string", "description": "Required when there are several"},
            "limit": {"type": "integer", "description": "Rows shown (default 20, max 100)"}
        });
        if !self.deps.verified.is_empty() {
            properties["verified"] = verified_parameter();
        }
        let description = if self.deps.semantic_only {
            properties.as_object_mut().map(|p| p.remove("sql"));
            "Run one read-only semantic-model query against a warehouse: `semantic` names \
             members as `view.member`. Raw SQL is not accepted. The full result is stored as \
             `r<N>`; you see its first `limit` rows."
        } else {
            "Run one read-only query against a warehouse. Give exactly one of `sql` (a single \
             SELECT or WITH statement) or `semantic` (a semantic-model query naming members as \
             `view.member`). The full result is stored as `r<N>`; you see its first `limit` \
             rows. Writes are refused."
        };
        ToolDef {
            name: "query",
            description,
            parameters: json!({"type": "object", "properties": properties}),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let prepared = self.prepare(&args)?;
        let limit = shown_limit(&args)?;
        let connector =
            self.pick_connector(&prepared, args.get("database").and_then(Value::as_str))?;
        // A temporary: `dyn Dialect` is not `Send`, so it must not live across
        // the await below.
        gate(
            &prepared.sql,
            sqlparser_dialect(connector.dialect()).as_ref(),
        )
        .map_err(|r| gate_failure(&prepared, &r))?;
        let start = Instant::now();
        // The database-level backstop: the gate above is the fast refusal
        // with a hint, this is the guarantee (a connector without one never
        // reaches here; `build_react_run` refuses it).
        let result = connector
            .execute_query_read_only(&prepared.sql, SAMPLE_LIMIT)
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(match result {
            Ok(exec) => {
                let truncated = exec.result.truncated;
                let reply = success_reply(prepared, exec, duration_ms, limit, cx);
                if let Some(id) = reply.value.get("result_id").and_then(Value::as_str) {
                    cx.results.persist(id, truncated).await;
                }
                reply
            }
            Err(e) => failed_reply(prepared, e.to_string(), duration_ms),
        })
    }
}
