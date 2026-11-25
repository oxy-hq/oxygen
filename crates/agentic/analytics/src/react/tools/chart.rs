//! `chart`: a chart of one stored result, drawn from its rows as the query
//! returned them.
//!
//! The spec names columns of a result `rN`; [`check`] tests it against the
//! stored result and returns the points the chart row carries. Nothing here
//! sums, groups or sorts: a spec whose x (and series) value repeats is refused,
//! so the model aggregates in SQL and the chart can never disagree with the
//! query.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_core::events::Event;
use agentic_core::react::{Artifact, Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use agentic_runtime::react::{ColumnKind, ResultStore, StoredResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AnalyticsEvent;

/// The most rows a chart draws. A row is one x value (per series), so this
/// is the number of x positions, whatever the number of y columns.
pub const MAX_CHART_ROWS: usize = 500;

const AGGREGATE_HINT: &str = "a chart draws the rows as the query returned them and sums \
    nothing: GROUP BY the x column (and the series column) in a new query, then chart that \
    result";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Line,
    Bar,
    Pie,
    Scatter,
}

impl ChartKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Bar => "bar",
            Self::Pie => "pie",
            Self::Scatter => "scatter",
        }
    }
}

/// What the model asks for: columns of `result_id`, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartSpec {
    pub result_id: String,
    pub kind: ChartKind,
    pub x: String,
    pub y: Vec<String>,
    #[serde(default)]
    pub series: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

fn failure(error: String, hint: Option<&str>) -> ToolFailure {
    ToolFailure {
        error,
        hint: hint.map(str::to_owned),
        candidates: Vec::new(),
    }
}

/// The one column called `name`: a missing name lists the columns, and a
/// name two columns share (a join) asks for an alias.
fn column(result: &StoredResult, id: &str, name: &str) -> Result<usize, ToolFailure> {
    let mut at = result
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| *c == name);
    match (at.next(), at.next()) {
        (Some((i, _)), None) => Ok(i),
        (None, _) => Err(ToolFailure {
            error: format!("{id} has no column {name}"),
            hint: None,
            candidates: result.columns.clone(),
        }),
        (Some(_), Some(_)) => Err(failure(
            format!("{id} has more than one column named {name}"),
            Some("give them distinct names with AS in a new query, then chart that result"),
        )),
    }
}

fn require_number(
    result: &StoredResult,
    id: &str,
    name: &str,
    i: usize,
    axis: &str,
) -> Result<(), ToolFailure> {
    match result.column_kinds.get(i) {
        Some(ColumnKind::Number) => Ok(()),
        _ => Err(failure(
            format!("{axis} column {name} of {id} is not numeric"),
            Some("cast it to a number in a new query, or chart a numeric column"),
        )),
    }
}

/// Checks `spec` against result `id` and returns the chart's points: one row
/// per result row, as `[x, series?, y…]`, values as stored (a null stays
/// null). Every refusal says how to fix the spec or the query.
pub fn check(spec: &ChartSpec, result: &StoredResult) -> Result<Vec<Vec<Value>>, ToolFailure> {
    let id = spec.result_id.as_str();
    let rows = drawable_rows(result, id)?;
    let cols = resolve(spec, result)?;
    if spec.kind == ChartKind::Pie {
        refuse_negative(rows, cols.ys[0], id)?;
    }
    // Only a scatter may repeat x: every other kind draws one mark per x.
    if spec.kind != ChartKind::Scatter {
        refuse_repeats(rows, cols.x, cols.series, id)?;
    }
    let cell = |row: &Vec<Value>, i: usize| row.get(i).cloned().unwrap_or(Value::Null);
    Ok(rows
        .iter()
        .map(|row| {
            std::iter::once(cols.x)
                .chain(cols.series)
                .chain(cols.ys.iter().copied())
                .map(|i| cell(row, i))
                .collect()
        })
        .collect())
}

/// The result's rows, when there are some and no more than a chart draws.
fn drawable_rows<'a>(result: &'a StoredResult, id: &str) -> Result<&'a [Vec<Value>], ToolFailure> {
    if result.row_count == 0 {
        return Err(failure(format!("{id} has no rows to chart"), None));
    }
    if result.truncated || result.row_count > MAX_CHART_ROWS {
        let n = match result.truncated {
            true => format!("more than {}", result.row_count),
            false => result.row_count.to_string(),
        };
        return Err(failure(
            format!("{id} has {n} rows; a chart draws at most {MAX_CHART_ROWS}"),
            Some("aggregate or filter in a new query (GROUP BY, WHERE, LIMIT), then chart that"),
        ));
    }
    let rows = result.rows.as_slice();
    if rows.len() < result.row_count {
        return Err(failure(format!("the rows of {id} are not available"), None));
    }
    Ok(rows)
}

/// The spec's columns, by place.
struct Columns {
    x: usize,
    series: Option<usize>,
    ys: Vec<usize>,
}

/// Resolves the spec's names and applies each kind's shape: numeric y, a
/// numeric x for a scatter, one y with a series, one y and no series on a pie.
fn resolve(spec: &ChartSpec, result: &StoredResult) -> Result<Columns, ToolFailure> {
    let id = spec.result_id.as_str();
    if spec.y.is_empty() {
        return Err(failure(
            "y names no column".into(),
            Some("pass y: [<numeric column>]"),
        ));
    }
    let x = column(result, id, &spec.x)?;
    if spec.kind == ChartKind::Scatter {
        require_number(result, id, &spec.x, x, "x")?;
    }
    let ys = spec
        .y
        .iter()
        .map(|name| {
            let i = column(result, id, name)?;
            require_number(result, id, name, i, "y").map(|()| i)
        })
        .collect::<Result<Vec<usize>, _>>()?;
    let pie = spec.kind == ChartKind::Pie;
    let refusal = match (spec.series.is_some(), ys.len() > 1) {
        (true, _) if pie => Some(("a pie chart takes no series", Some("use bar"))),
        (true, true) => Some((
            "with a series, y names exactly one column",
            Some("chart one measure split by the series, or drop the series"),
        )),
        (false, true) if pie => Some(("a pie chart takes one y column", None)),
        _ => None,
    };
    if let Some((error, hint)) = refusal {
        return Err(failure(error.into(), hint));
    }
    let series = match spec.series.as_deref() {
        Some(name) => Some(column(result, id, name)?),
        None => None,
    };
    Ok(Columns { x, series, ys })
}

/// A pie slice cannot be negative: drawing one would hide the sign.
fn refuse_negative(rows: &[Vec<Value>], y: usize, id: &str) -> Result<(), ToolFailure> {
    match rows
        .iter()
        .find_map(|r| r.get(y)?.as_f64().filter(|v| *v < 0.0))
    {
        Some(v) => Err(failure(
            format!("{id} has a negative value ({v}); a pie chart cannot show it"),
            Some("use a bar chart"),
        )),
        None => Ok(()),
    }
}

/// Each (x, series) pair must name one row: a repeat would have to be summed
/// or dropped to draw, and the chart does neither.
fn refuse_repeats(
    rows: &[Vec<Value>],
    x: usize,
    series: Option<usize>,
    id: &str,
) -> Result<(), ToolFailure> {
    let mut seen: HashMap<String, usize> = HashMap::with_capacity(rows.len());
    for (n, row) in rows.iter().enumerate() {
        let at = |i: usize| row.get(i).cloned().unwrap_or(Value::Null);
        let key = json!([at(x), series.map(at)]).to_string();
        if let Some(first) = seen.insert(key, n) {
            let label = match series {
                Some(s) => format!("x {} with series {}", at(x), at(s)),
                None => format!("x {}", at(x)),
            };
            return Err(failure(
                format!(
                    "{label} appears in more than one row of {id} (rows {} and {})",
                    first + 1,
                    n + 1
                ),
                Some(AGGREGATE_HINT),
            ));
        }
    }
    Ok(())
}

/// The `result_chart` row for a checked spec.
fn result_chart(spec: ChartSpec, rows: Vec<Vec<Value>>) -> Artifact {
    let event = AnalyticsEvent::ResultChart {
        result_id: spec.result_id,
        kind: spec.kind,
        title: spec.title,
        x: spec.x,
        series: spec.series,
        y: spec.y,
        rows,
    };
    let (event_type, payload) = Event::Domain(event).serialize();
    Artifact {
        event_type,
        payload,
    }
}

/// Reads results from the run's [`ResultStore`], the one the loop writes, so
/// a result made before a park is read back from storage after the resume.
pub struct ChartTool {
    results: Arc<ResultStore>,
}

impl ChartTool {
    pub fn new(results: Arc<ResultStore>) -> Self {
        Self { results }
    }
}

#[async_trait]
impl Tool for ChartTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "chart",
            description: "Draw a chart of a query result; it shows under your answer. The \
                chart plots the result's rows as returned and never sums or groups them, so \
                the query must return one row per x value (per series value). At most 500 \
                rows. y columns must be numeric.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "result_id": {"type": "string", "description": "A result a query returned, e.g. r2"},
                    "kind": {"type": "string", "enum": ["line", "bar", "pie", "scatter"]},
                    "x": {"type": "string", "description": "Column for the x axis (pie: the slice labels)"},
                    "y": {"type": "array", "items": {"type": "string"}, "description": "Numeric columns to plot"},
                    "series": {"type": "string", "description": "Column that splits one y into lines or bars"},
                    "title": {"type": "string"}
                },
                "required": ["result_id", "kind", "x", "y"],
                "additionalProperties": false
            }),
            strict: true,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, _cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let spec: ChartSpec = serde_json::from_value(args).map_err(|e| {
            failure(
                format!("bad chart spec: {e}"),
                Some("pass result_id, kind (line|bar|pie|scatter), x and y; series and title are optional"),
            )
        })?;
        let result = self.results.load(&spec.result_id).await.ok_or_else(|| {
            failure(
                format!("no result {}", spec.result_id),
                Some("chart a result id a query returned in this run"),
            )
        })?;
        let rows = check(&spec, &result)?;
        let text = format!(
            "Chart drawn: {} of {} by {} from {}, {} rows. It shows under your answer.",
            spec.kind.as_str(),
            spec.y.join(", "),
            spec.x,
            spec.result_id,
            rows.len()
        );
        let value = json!({"result_id": spec.result_id});
        let mut reply = ToolReply::new(text, value);
        reply.artifacts.push(result_chart(spec, rows));
        Ok(reply)
    }
}
