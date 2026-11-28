//! [`ToolDef`] factories — one set per FSM state.

use agentic_core::tools::ToolDef;
use serde_json::json;

use crate::types::{ChartConfig, QuestionType};

use super::{
    CHECK_DATA_FRESHNESS_DESC, SAMPLE_COLUMNS_DESC, SEARCH_AUTOMATIONS_DESC, SEARCH_CATALOG_DESC,
};

// ── Tool definitions per state ────────────────────────────────────────────────

/// Tools available during the **triage** sub-phase of Clarify.
///
/// Triage only *classifies* the question and discovers schema. Metric-tree
/// tools belong to the dedicated `root_cause` handler that runs when
/// triage classifies as `QuestionType::RootCause`. Earlier we tried
/// putting them here directly; the result was that triage produced a
/// great draft answer but the FSM's `general_inquiry_impl` then issued a
/// fresh LLM call with no awareness of the tool result, producing a
/// "I can only report what the data shows" non-answer.
///
/// `has_metric_tree` is kept on the signature for API stability — the
/// caller doesn't need to thread two different function signatures
/// based on workspace config — but it currently has no effect on the
/// returned tool list.
pub fn triage_tools(_has_metric_tree: bool) -> Vec<ToolDef> {
    let tools = vec![
        ToolDef {
            name: "search_automations",
            description: SEARCH_AUTOMATIONS_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search term matched against automation names and descriptions"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        ToolDef {
            name: "search_catalog",
            description: SEARCH_CATALOG_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Search terms matched against measure/dimension names and descriptions."
                    }
                },
                "required": ["queries"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        propose_semantic_query_tool(),
    ];
    tools
}

const PROPOSE_SEMANTIC_QUERY_DESC: &str = "Call this tool AFTER search_catalog confirms that ALL needed measures \
     and dimensions exist. Submits a structured semantic query for fast \
     compilation, skipping SQL generation. Only call when you are certain \
     about the view.member paths — do not guess.";

/// Tool definition for `propose_semantic_query`.
///
/// Extracted from the former `semantic_query` field of the triage response
/// schema so that the response schema stays small enough for strict-mode
/// grammar compilation.
pub fn propose_semantic_query_tool() -> ToolDef {
    ToolDef {
        name: "propose_semantic_query",
        description: PROPOSE_SEMANTIC_QUERY_DESC,
        parameters: json!({
            "type": "object",
            "properties": {
                "measures": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exact measure member paths in view.member format (e.g. 'orders.revenue'). Must match names from search_catalog results exactly."
                },
                "dimensions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exact dimension member paths in view.member format. Must match names from search_catalog results exactly."
                },
                "filters": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "member": { "type": "string" },
                            "operator": { "type": "string" },
                            "values": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["member", "operator", "values"],
                        "additionalProperties": false
                    },
                    "description": "Structured filter conditions using exact member paths."
                },
                "time_dimensions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "dimension": { "type": "string" },
                            "granularity": { "type": ["string", "null"] },
                            "date_range": { "type": ["array", "null"], "items": { "type": "string" } }
                        },
                        "required": ["dimension", "granularity", "date_range"],
                        "additionalProperties": false
                    },
                    "description": "Time dimension entries with granularity and optional date range."
                },
                "order": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "desc": { "type": "boolean" }
                        },
                        "required": ["id", "desc"],
                        "additionalProperties": false
                    },
                    "description": "Sort order entries."
                },
                "limit": {
                    "type": ["integer", "null"],
                    "description": "Row limit, or null for no limit."
                },
                "confidence": {
                    "type": "number",
                    "description": "How confident you are that these members are correct (0.0–1.0). Only set >= 0.85 when ALL measures and dimensions were confirmed by search_catalog."
                }
            },
            "required": ["measures", "dimensions", "filters", "time_dimensions", "order", "limit", "confidence"],
            "additionalProperties": false
        }),
        strict: false,
    }
}

/// Tools available during the **clarifying** state.
///
/// When `has_semantic` is `true` the semantic model covers the data model and
/// raw database introspection tools (`list_tables`, `describe_table`) are
/// excluded to avoid confusing the LLM with two competing schema views.
pub fn clarifying_tools(has_semantic: bool, has_metric_tree: bool) -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef {
            name: "search_catalog",
            description: SEARCH_CATALOG_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One or more search terms. Each term is matched against metric names and descriptions. Use [\"\"] to list everything."
                    }
                },
                "required": ["queries"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        ToolDef {
            name: "search_automations",
            description: SEARCH_AUTOMATIONS_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search term matched against automation names and descriptions"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
    ];
    if !has_semantic {
        tools.push(list_tables_tool_def());
        tools.push(describe_table_tool_def());
    }
    // Metric-tree tools live in the dedicated `root_cause` handler, not
    // here. See `triage_tools` comment for the architectural rationale.
    let _ = has_metric_tree;
    tools
}

/// Tools available during the **specifying** state.
///
/// Includes `search_catalog` so Specifying can discover metrics/dimensions
/// directly from the raw question without a prior Ground phase.
///
/// When `has_semantic` is `true`, raw database tools (`list_tables`,
/// `describe_table`) are excluded — same rationale as [`clarifying_tools`].
pub fn specifying_tools(has_semantic: bool, has_metric_tree: bool) -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef {
            name: "search_catalog",
            description: SEARCH_CATALOG_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One or more search terms matched against metric names and descriptions. Use [\"\"] to list everything."
                    }
                },
                "required": ["queries"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        ToolDef {
            name: "sample_columns",
            description: SAMPLE_COLUMNS_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "columns": {
                        "type": "array",
                        "description": "One or more columns to sample.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "table": {
                                    "type": "string",
                                    "description": "Semantic view name or database table name"
                                },
                                "column": {
                                    "type": "string",
                                    "description": "Dimension/measure name or database column name"
                                },
                                "search_term": {
                                    "type": ["string", "null"],
                                    "description": "Optional substring filter (LIKE '%term%'). Pass null when not searching."
                                }
                            },
                            "required": ["table", "column", "search_term"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["columns"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
    ];
    if has_semantic {
        // Freshness targets resolve through semantic views (watermark
        // dimension + meta contract), so the tool is only offered when a
        // semantic model is present.
        tools.push(check_data_freshness_tool_def());
    }
    if !has_semantic {
        // Without a semantic model, the LLM needs manual join discovery and
        // raw schema introspection tools.
        tools.push(ToolDef {
            name: "get_join_path",
            description:
                "Return the join path between two entities: path expression and join type.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "from_entity": {
                        "type": "string",
                        "description": "Source table or entity name"
                    },
                    "to_entity": {
                        "type": "string",
                        "description": "Target table or entity name"
                    }
                },
                "required": ["from_entity", "to_entity"],
                "additionalProperties": false
            }),
            ..Default::default()
        });
        tools.push(list_tables_tool_def());
        tools.push(describe_table_tool_def());
    }
    // Metric-tree tools live in the dedicated `root_cause` handler.
    let _ = has_metric_tree;
    tools
}

/// Tool definition for `check_data_freshness`.
///
/// Offered in the **specifying** state when a semantic model is present.
/// The executor lives in [`super::specifying`]; targets resolve via
/// [`crate::catalog::Catalog::resolve_freshness_target`].
pub fn check_data_freshness_tool_def() -> ToolDef {
    ToolDef {
        name: "check_data_freshness",
        description: CHECK_DATA_FRESHNESS_DESC,
        parameters: json!({
            "type": "object",
            "properties": {
                "views": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Semantic view names (or their underlying table names) to check."
                }
            },
            "required": ["views"],
            "additionalProperties": false
        }),
        ..Default::default()
    }
}

/// Tools available during the **solving** state.
pub fn solving_tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "execute_preview",
        description: "Run a SQL query with a hard LIMIT 5 and return real columns and rows. \
                      Use this to verify joins and filters produce actual results before \
                      finalizing the SQL. Returns {ok, columns, rows, row_count} on success \
                      or {ok: false, error} on failure.",
        parameters: json!({
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "The SQL query to preview"
                }
            },
            "required": ["sql"],
            "additionalProperties": false
        }),
        ..Default::default()
    }]
}

/// Tools available during the **interpreting** state.
pub fn interpreting_tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "render_chart",
        description: "Render a chart or table from the query result. \
                      The data is already available from the executed query — \
                      only specify the chart type and which columns to use. \
                      Column names must exactly match the columns in the result set. \
                      Returns {ok: true} on success or {ok: false, errors: [...]} when a \
                      column name is wrong — fix and retry immediately. \
                      The chart is streamed to the client immediately when this tool is called. \
                      You may call it multiple times to produce multiple charts. \
                      When multiple result sets are available, use `result_index` to select which \
                      one to visualise (0-based, default 0).",
        parameters: json!({
            "type": "object",
            "properties": {
                "chart_type": {
                    "type": "string",
                    "enum": ["line_chart", "bar_chart", "pie_chart", "table"],
                    "description": "Chart variant to render"
                },
                "x": {
                    "type": ["string", "null"],
                    "description": "Column name for the x-axis. Required for line_chart and bar_chart. Use null for pie_chart and table."
                },
                "y": {
                    "type": ["string", "null"],
                    "description": "Column name for the y-axis / metric. Required for line_chart and bar_chart. Use null for pie_chart and table."
                },
                "series": {
                    "type": ["string", "null"],
                    "description": "Optional grouping column name to split data into multiple series \
        (line_chart / bar_chart only). When set, the data is grouped by this column's \
        distinct values and each group becomes a separate line or bar series in the chart. \
        For example, if x='month', y='revenue', series='region', the chart renders one \
        line/bar per region. Use null when there is no grouping column or for pie_chart/table."
                },
                "name": {
                    "type": ["string", "null"],
                    "description": "Category column name. Required for pie_chart. Use null for other chart types."
                },
                "value": {
                    "type": ["string", "null"],
                    "description": "Value column name. Required for pie_chart. Use null for other chart types."
                },
                "x_axis_label": {
                    "type": ["string", "null"],
                    "description": "Human-readable x-axis label (include units, e.g. 'Date', 'Revenue (USD)'). Use null to omit."
                },
                "y_axis_label": {
                    "type": ["string", "null"],
                    "description": "Human-readable y-axis label (include units, e.g. 'Sales ($)', 'Count'). Use null to omit."
                },
                "result_index": {
                    "type": ["integer", "null"],
                    "description": "Which result set to visualise (0-based). Use null to default to the first result set."
                },
                "title": {
                    "type": ["string", "null"],
                    "description": "Optional chart title. Use null to omit."
                }
            },
            "required": ["chart_type", "x", "y", "series", "name", "value", "x_axis_label", "y_axis_label", "result_index", "title"],
            "additionalProperties": false
        }),
        ..Default::default()
    }]
}

/// Derive a deterministic [`ChartConfig`] suggestion from the question type and
/// result columns.
///
/// Returns `None` for question types that do not benefit from a chart (e.g.
/// `SingleValue`, `GeneralInquiry`) or when there are fewer than two columns.
pub fn suggest_chart_config(
    question_type: &QuestionType,
    columns: &[String],
) -> Option<ChartConfig> {
    if columns.len() < 2 {
        return None;
    }
    match question_type {
        QuestionType::Trend => Some(ChartConfig {
            chart_type: "line_chart".to_string(),
            x: Some(columns[0].clone()),
            y: Some(columns[1].clone()),
            series: columns.get(2).cloned(),
            name: None,
            value: None,
            title: None,
            x_axis_label: None,
            y_axis_label: None,
        }),
        QuestionType::Comparison | QuestionType::Breakdown => Some(ChartConfig {
            chart_type: "bar_chart".to_string(),
            x: Some(columns[0].clone()),
            y: Some(columns[1].clone()),
            series: columns.get(2).cloned(),
            name: None,
            value: None,
            title: None,
            x_axis_label: None,
            y_axis_label: None,
        }),
        QuestionType::Distribution => Some(ChartConfig {
            chart_type: "bar_chart".to_string(),
            x: Some(columns[0].clone()),
            y: Some(columns[1].clone()),
            series: None,
            name: None,
            value: None,
            title: None,
            x_axis_label: None,
            y_axis_label: None,
        }),
        QuestionType::SingleValue
        | QuestionType::GeneralInquiry
        | QuestionType::RootCause
        | QuestionType::Opportunity => None,
    }
}

pub(super) fn list_tables_tool_def() -> ToolDef {
    ToolDef {
        name: "list_tables",
        description: "List all tables available in the connected database(s). \
                      Use this when the semantic model doesn't cover the data \
                      the user is asking about. Returns {tables: [{name, database}]}.",
        parameters: json!({
            "type": "object",
            "properties": {
                "database": {
                    "type": ["string", "null"],
                    "description": "Specific database/connector name. Use null to list from all databases."
                }
            },
            "required": ["database"],
            "additionalProperties": false
        }),
        ..Default::default()
    }
}

// ── Metric-tree tools ────────────────────────────────────────────────────────
//
// Surfaced when the workspace has a semantic model AND a
// `MetricTreeRunner` is wired in. The four tools cover the airlayer
// metric-tree op surface:
//
// - `explain_metric`     — period-over-period root cause analysis
// - `find_opportunities` — segment opportunity sizing
// - `metric_sensitivity` — rank declared drivers of a measure
// - `predict_impact`     — propagate hypothetical deltas through the tree
//
// Tool descriptions point the LLM at the right tool for the question
// type: "why did X drop / change / spike" → explain_metric; "where can we
// grow X / which segments underperform" → find_opportunities;
// "what drives X" → metric_sensitivity; "if Y went up by 10% what
// happens to Z" → predict_impact.

const EXPLAIN_METRIC_DESC: &str = "Explain WHY a metric changed between two time periods. Use this for \
     questions like 'why did revenue drop in Q1?', 'what caused the spike \
     in churn last week?', 'what's behind the slowdown in signups?'. \
     Walks the metric tree to find the smallest (component, dimension-segment) \
     combinations that account for the change. Returns a ranked tree of \
     splits with per-node concentration, plus driver attribution and warnings \
     about Simpson's paradox / opposing offsets / non-additive measures.";

const OPPORTUNITY_DESC: &str = "Size the upside opportunity for a metric. For each viable \
     dimension of the target's view, picks a benchmark (best-performing peer \
     for small dimensions, P75 once there are >=8 segments), measures every \
     segment's gap to that benchmark, and ranks dimensions by total match-the-best \
     upside. Use for 'how do we make more money?', 'where can we grow X?', \
     'which segments underperform?'. Returns the top-K dimensions x top-K \
     segments (with the long tail trimmed), each segment's gap and weighted \
     upside, plus downstream effects propagated via the metric tree. \
     High-cardinality dimensions (customer_id, order_id, etc.) and flat \
     distributions are excluded automatically and reported in skipped_dimensions \
     so you can describe what was and wasn't analysed.";

const SENSITIVITY_DESC: &str = "List the declared drivers of a metric, ranked by influence. Use for \
     questions like 'what drives revenue?', 'what affects churn?', 'what \
     levers move X?'. Uses the metric tree's component + driver edges \
     (with declared coefficients and functional forms) — does NOT fit \
     coefficients from data. Returns each driver's path, effective \
     coefficient (chain rule along the path), direction, strength, \
     and lag.";

const PREDICT_IMPACT_DESC: &str = "Propagate hypothetical changes through the metric tree. Use for \
     'if we improve X by 10%, what happens to Y?' / 'what's the impact \
     of cutting fuel cost by 5% on profit?'. Takes one or more (measure, \
     delta) pairs and returns predicted impacts on every downstream \
     measure, marked as 'exact' (component edges) or 'estimated' (driver \
     edges with declared coefficients).";

/// `ToolDef`s for the four metric-tree analysis tools.
///
/// Returned only when both a semantic model and a `MetricTreeRunner`
/// are present. The caller (`specifying_tools` / `clarifying_tools`)
/// concatenates these into the per-state tool list.
pub fn metric_tree_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "explain_metric",
            description: EXPLAIN_METRIC_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Fully qualified measure id, e.g. 'orders.revenue'. Must come from search_catalog results."
                    },
                    "time_dimension": {
                        "type": "string",
                        "description": "Fully qualified time-dimension id used to partition the periods, e.g. 'orders.order_date'."
                    },
                    "current_period_start": {
                        "type": "string",
                        "description": "Inclusive start date of the period being explained (YYYY-MM-DD)."
                    },
                    "current_period_end": {
                        "type": "string",
                        "description": "Inclusive end date of the period being explained (YYYY-MM-DD)."
                    },
                    "previous_period_start": {
                        "type": "string",
                        "description": "Inclusive start date of the comparison/baseline period (YYYY-MM-DD)."
                    },
                    "previous_period_end": {
                        "type": "string",
                        "description": "Inclusive end date of the comparison/baseline period (YYYY-MM-DD)."
                    },
                    "deep": {
                        "type": ["boolean", "null"],
                        "description": "When true, run beam-search + statistical significance for higher-quality alternatives at higher query cost. Default false."
                    }
                },
                "required": [
                    "target", "time_dimension",
                    "current_period_start", "current_period_end",
                    "previous_period_start", "previous_period_end",
                    "deep"
                ],
                "additionalProperties": false
            }),
            // strict=false: the four metric-tree tool schemas combined
            // push the strict-mode compiled grammar past OpenAI's size
            // cap on the specifying state's tool list. Descriptions still
            // drive selection; runtime ToolError::BadParams catches any
            // shape drift at dispatch.
            strict: false,
            ..Default::default()
        },
        ToolDef {
            name: "find_opportunities",
            description: OPPORTUNITY_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Fully qualified measure id to optimize."
                    },
                    "time_dimension": {
                        "type": "string",
                        "description": "Fully qualified time-dimension id used to bound the analysis window."
                    },
                    "period_start": {
                        "type": "string",
                        "description": "Inclusive start date of the analysis window (YYYY-MM-DD)."
                    },
                    "period_end": {
                        "type": "string",
                        "description": "Inclusive end date of the analysis window (YYYY-MM-DD)."
                    }
                },
                "required": ["target", "time_dimension", "period_start", "period_end"],
                "additionalProperties": false
            }),
            strict: false,
            ..Default::default()
        },
        ToolDef {
            name: "metric_sensitivity",
            description: SENSITIVITY_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Fully qualified measure id whose drivers should be ranked."
                    }
                },
                "required": ["target"],
                "additionalProperties": false
            }),
            strict: false,
            ..Default::default()
        },
        ToolDef {
            name: "predict_impact",
            description: PREDICT_IMPACT_DESC,
            parameters: json!({
                "type": "object",
                "properties": {
                    "changes": {
                        "type": "array",
                        "description": "List of hypothetical changes to propagate. Each entry is a (measure, delta) pair.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "measure": { "type": "string" },
                                "delta": { "type": "number" }
                            },
                            "required": ["measure", "delta"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["changes"],
                "additionalProperties": false
            }),
            strict: false,
            ..Default::default()
        },
    ]
}

/// Tool names exposed by [`metric_tree_tools`]. Routing in the solver
/// uses this to detect a metric-tree dispatch without string-matching
/// every call site.
pub const METRIC_TREE_TOOL_NAMES: &[&str] = &[
    "explain_metric",
    "find_opportunities",
    "metric_sensitivity",
    "predict_impact",
];

/// True when `name` is one of the four metric-tree tool names.
pub fn is_metric_tree_tool(name: &str) -> bool {
    METRIC_TREE_TOOL_NAMES.contains(&name)
}

pub(super) fn describe_table_tool_def() -> ToolDef {
    ToolDef {
        name: "describe_table",
        description: "Get column names, data types, and sample values for a database table. \
                      Use this to understand table structure when the semantic model doesn't \
                      have the information needed. \
                      Returns {table, columns: [{name, data_type, sample_values}]}.",
        parameters: json!({
            "type": "object",
            "properties": {
                "table": {
                    "type": "string",
                    "description": "Table name to describe"
                },
                "database": {
                    "type": ["string", "null"],
                    "description": "Connector name if multiple databases are configured. Use null for the default database."
                }
            },
            "required": ["table", "database"],
            "additionalProperties": false
        }),
        ..Default::default()
    }
}

/// Tools for anomaly discovery and explanation, used inside the `RootCause` inquiry loop.
pub fn anomaly_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_anomalies",
            description: "Check the anomaly inbox for previously detected anomalies matching \
                          the given metric and time range. Call this first before running \
                          on-the-fly detection.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "measure": { "type": "string", "description": "Measure name from the semantic model" },
                    "time_dimension": { "type": "string", "description": "Time dimension field" },
                    "granularity": {
                        "type": "string",
                        "enum": ["day", "week", "month", "quarter"],
                        "description": "Time granularity"
                    },
                    "period_start": { "type": "string", "description": "First day of the period, YYYY-MM-DD (UTC), e.g. 2024-01-01" },
                    "period_end": { "type": "string", "description": "Last day of the period, included, YYYY-MM-DD (UTC), e.g. 2024-01-31" }
                },
                "required": ["measure", "time_dimension", "granularity", "period_start", "period_end"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        ToolDef {
            name: "detect_anomalies",
            description: "Run anomaly detection on every bucket from period_start to period_end; the tool \
                          fetches the history before it. Use when the inbox is empty or the period is not yet \
                          scanned. Results go to the inbox. Report each bucket listed in missing_periods.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "measure": { "type": "string", "description": "Measure name from the semantic model" },
                    "time_dimension": { "type": "string", "description": "Time dimension field" },
                    "granularity": {
                        "type": "string",
                        "enum": ["day", "week", "month", "quarter"],
                        "description": "Time granularity"
                    },
                    "period_start": { "type": "string", "description": "First bucket to score, YYYY-MM-DD" },
                    "period_end": { "type": "string", "description": "Last bucket to score, YYYY-MM-DD" }
                },
                "required": ["measure", "time_dimension", "granularity", "period_start", "period_end"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
        ToolDef {
            name: "explain_anomaly",
            description: "Run root-cause analysis on a specific anomaly using the metric tree. \
                          Returns contributing dimensions and their impact magnitude. \
                          Uses a cached result when available.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "anomaly_id": {
                        "type": "string",
                        "description": "UUID of the anomaly from list_anomalies or detect_anomalies"
                    }
                },
                "required": ["anomaly_id"],
                "additionalProperties": false
            }),
            ..Default::default()
        },
    ]
}
