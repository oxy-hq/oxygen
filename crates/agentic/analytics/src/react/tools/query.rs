//! `query`: the agent's only path to a customer warehouse.
//!
//! Runs one read statement (raw `sql`, or a `semantic` query compiled by the
//! semantic model) behind the read-only [`gate`], stores the rows as `r<N>`, and
//! leaves the same `query_executed` row the FSM writes for each execution.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentic_connector::{DatabaseConnector, ExecutionResult};
use agentic_core::events::Event;
use agentic_core::react::{Artifact, Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::result::CellValue;
use agentic_core::tools::ToolDef;
use agentic_runtime::react::DEFAULT_PREVIEW_ROWS;
use agentic_semantic::config::SemanticQueryConfig;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::semantic::compile_semantic;
use crate::react::read_only::{gate, sqlparser_dialect};
use crate::react::render::render_table;
use crate::{AnalyticsEvent, QuerySource, SemanticCatalog};

/// Rows fetched per query, as the FSM's executing stage fetches.
const SAMPLE_LIMIT: u64 = 1_000;
const DEFAULT_SHOWN: usize = 20;
const MAX_SHOWN: usize = 100;
const MAX_CANDIDATES: usize = 5;
/// Below this Jaro-Winkler score a name is not offered as a candidate.
const MIN_SIMILARITY: f64 = 0.7;

pub struct QueryDeps {
    pub connectors: HashMap<String, Arc<dyn DatabaseConnector>>,
    pub catalog: Option<Arc<SemanticCatalog>>,
}

pub struct QueryTool {
    deps: QueryDeps,
}

/// The statement to run and the provenance its `query_executed` row records.
struct Prepared {
    sql: String,
    source: QuerySource,
}

impl Prepared {
    /// Compiled from a semantic query, so the model has not seen this SQL.
    fn compiled(&self) -> bool {
        self.source == QuerySource::Semantic
    }
}

impl QueryTool {
    pub fn new(deps: QueryDeps) -> Self {
        Self { deps }
    }

    /// The named connector, else the only one.
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
                _ => Err(ToolFailure {
                    error: "database is required".into(),
                    hint: Some("pass `database`, one of the candidates".into()),
                    candidates: names.into_iter().map(str::to_owned).collect(),
                }),
            },
        }
    }

    /// Exactly one of `sql` and `semantic`; a semantic query compiles here.
    fn prepare(&self, args: &Value) -> Result<Prepared, ToolFailure> {
        let sql = args.get("sql").and_then(Value::as_str);
        let semantic = args.get("semantic").filter(|v| !v.is_null());
        match (sql, semantic) {
            (Some(sql), None) => Ok(Prepared {
                sql: sql.to_owned(),
                source: QuerySource::Llm,
            }),
            (None, Some(v)) => {
                let catalog = self
                    .deps
                    .catalog
                    .as_deref()
                    .ok_or_else(|| failure("this agent has no semantic model; use `sql`"))?;
                let config: SemanticQueryConfig = serde_json::from_value(v.clone())
                    .map_err(|e| failure(&format!("invalid semantic query: {e}")))?;
                let sql = compile_semantic(catalog, &config)?;
                Ok(Prepared {
                    sql,
                    source: QuerySource::Semantic,
                })
            }
            _ => Err(failure("give exactly one of `sql` and `semantic`")),
        }
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
    let preview: Vec<Vec<Value>> = rows.iter().take(DEFAULT_PREVIEW_ROWS).cloned().collect();
    let artifact = query_executed(&p, duration_ms, Ok((columns.clone(), preview, row_count)));
    let id = cx.results.put(p.sql.clone(), columns, rows);
    let mut text = format!("result {id}: {}", row_count_text(&exec, row_count));
    if p.compiled() {
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
    if p.compiled() {
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
        ToolDef {
            name: "query",
            description: "Run one read-only query against a warehouse. Give exactly one of \
                `sql` (a single SELECT or WITH statement) or `semantic` (a semantic-model \
                query naming members as `view.member`). The full result is stored as `r<N>`; \
                you see its first `limit` rows. Writes are refused.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": {"type": "string"},
                    "semantic": {
                        "type": "object",
                        "description": "{topic?, measures, dimensions, time_dimensions: \
                            [{dimension, granularity?}], filters: [{field, op, value}], \
                            orders: [{field, direction}], limit?}",
                    },
                    "database": {"type": "string", "description": "Required when there are several"},
                    "limit": {"type": "integer", "description": "Rows shown (default 20, max 100)"}
                }
            }),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let prepared = self.prepare(&args)?;
        let limit = shown_limit(&args)?;
        let connector = self.connector(args.get("database").and_then(Value::as_str))?;
        // A temporary: `dyn Dialect` is not `Send`, so it must not live across
        // the await below.
        gate(
            &prepared.sql,
            sqlparser_dialect(connector.dialect()).as_ref(),
        )
        .map_err(|r| ToolFailure {
            error: r.message().to_owned(),
            hint: Some(r.hint().into()),
            candidates: Vec::new(),
        })?;
        let start = Instant::now();
        let result = connector.execute_query(&prepared.sql, SAMPLE_LIMIT).await;
        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(match result {
            Ok(exec) => success_reply(prepared, exec, duration_ms, limit, cx),
            Err(e) => failed_reply(prepared, e.to_string(), duration_ms),
        })
    }
}
