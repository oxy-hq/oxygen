//! Return types for the [`Catalog`](super::Catalog) trait.

/// Brief metric description for browsing/searching.
#[derive(Debug, Clone)]
pub struct MetricSummary {
    pub name: String,
    pub description: String,
    /// Aggregation kind: `"sum"`, `"count"`, `"avg"`, etc.
    /// Empty for raw-schema metrics that have no semantic definition.
    pub metric_type: String,
    /// SQL expression or column reference (e.g. `"SUM(amount)"`, `"revenue"`).
    /// Lets the LLM see the formula without a separate tool call.
    pub expr: Option<String>,
}

/// Result of a batch catalog search across multiple queries.
///
/// Returned by [`Catalog::search_catalog`].  Contains **deduplicated**
/// metrics and dimensions matching *any* of the supplied query terms.
///
/// [`Catalog::search_catalog`]: super::Catalog::search_catalog
#[derive(Debug, Clone, Default)]
pub struct CatalogSearchResult {
    pub metrics: Vec<MetricSummary>,
    pub dimensions: Vec<DimensionSummary>,
}

/// Brief dimension description for browsing/searching.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DimensionSummary {
    pub name: String,
    pub description: String,
    /// Semantic type: `"string"`, `"number"`, `"date"`, `"boolean"`.
    pub data_type: String,
}

/// Full metric definition returned by [`Catalog::get_metric_definition`].
///
/// [`Catalog::get_metric_definition`]: super::Catalog::get_metric_definition
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricDef {
    pub name: String,
    /// SQL expression or column reference (e.g. `"orders.revenue"`, `"amount"`).
    pub expr: String,
    /// Aggregation kind: `"column"`, `"sum"`, `"count"`, `"avg"`, `"min"`,
    /// `"max"`, `"count_distinct"`.
    pub metric_type: String,
    /// Source table or view name.
    pub table: String,
    pub description: Option<String>,
    /// Logical connector name that owns this metric's source table.
    ///
    /// `None` means "use the default connector".  Set by `SchemaCatalog` from
    /// the connector tag recorded during `from_schema_info_named`, and by
    /// `SemanticCatalog` from the `data_source:` field on the view YAML.
    pub data_source: Option<String>,
}

/// Value range and sample values for a dimension column.
#[derive(Debug, Clone)]
pub struct ColumnRange {
    pub min: Option<serde_json::Value>,
    pub max: Option<serde_json::Value>,
    pub sample_values: Vec<serde_json::Value>,
    /// Semantic type: `"string"`, `"number"`, `"date"`, `"boolean"`.
    pub data_type: String,
}

/// Resolved physical target for the `sample_column` tool.
///
/// When the semantic model is active the LLM refers to views and dimensions
/// by their logical names (e.g. `orders_view.status`).  `SampleTarget` maps
/// those to the underlying database table and column expression so the tool
/// can build a valid SQL query.
#[derive(Debug, Clone)]
pub struct SampleTarget {
    /// Underlying database table (e.g. `"orders"`).
    pub table: String,
    /// SQL column expression (e.g. `"order_status"`).
    pub column_expr: String,
    /// Pre-existing sample values from the semantic definition, if any.
    /// When non-empty the tool can return these directly instead of hitting
    /// the database.
    pub static_samples: Vec<String>,
    /// Semantic data type (e.g. `"string"`, `"date"`).
    pub data_type: Option<String>,
    /// Connector/database name the view lives in (from `datasource:` in the
    /// view YAML). `None` means "use the default connector".
    pub datasource: Option<String>,
}

/// Resolved target for the `check_data_freshness` tool.
///
/// The watermark is the column expression whose `MAX(..)` answers "what date
/// does this view's data actually run through?". Views can declare it — plus
/// a human-maintained freshness contract — via free-form `meta:` keys:
///
/// ```yaml
/// meta:
///   freshness_watermark_column: ["business_date"]
///   freshness_expected_cadence: ["several times daily"]
///   freshness_complete_through: ["prior full day (America/Los_Angeles)"]
///   freshness_caveat: ["Current business day is intraday-partial."]
/// ```
///
/// Without a declaration the first date/datetime dimension is used.
#[derive(Debug, Clone)]
pub struct FreshnessTarget {
    /// Semantic view name.
    pub view: String,
    /// Underlying database table, or the view's name when it has none.
    pub table: String,
    /// The view is defined by `sql:`, not `table:`: [`Self::table`] names no
    /// relation, and the watermark is a column of the view's SQL output only.
    pub sql_defined: bool,
    /// Connector/database name from the view's `datasource:`. `None` means
    /// "use the default connector".
    pub datasource: Option<String>,
    /// SQL expression for the watermark column (a dimension's `expr`, or the
    /// raw declared column name when it matches no dimension).
    pub watermark_expr: String,
    /// Logical name of the watermark (dimension name or declared column).
    pub watermark_name: String,
    /// `true` when the view declared the watermark in `meta:`; `false` when
    /// it was inferred from the first date-typed dimension.
    pub declared: bool,
    /// Declared cadence (`meta: freshness_expected_cadence`).
    pub expected_cadence: Option<String>,
    /// Declared completeness policy (`meta: freshness_complete_through`).
    pub complete_through: Option<String>,
    /// Declared settlement caveat (`meta: freshness_caveat`).
    pub caveat: Option<String>,
    /// Recency-probe SQL from the view's `refresh_key: sql:`, if declared.
    pub refresh_key_sql: Option<String>,
}

/// A resolved join path between two tables or views.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JoinPath {
    /// Human-readable path expression (e.g. `"orders JOIN customers ON customer_id"`).
    pub path: String,
    pub join_type: String,
}

/// Rich context provided to the LLM for SQL generation when
/// [`Catalog::try_compile`] returns [`CatalogError::TooComplex`].
///
/// [`Catalog::try_compile`]: super::Catalog::try_compile
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct QueryContext {
    pub metric_definitions: Vec<MetricDef>,
    pub dimension_definitions: Vec<DimensionSummary>,
    /// `(from_entity, to_entity, path)` tuples.
    pub join_paths: Vec<(String, String, JoinPath)>,
    /// Prompt-ready schema description (tables, columns, join keys).
    pub schema_description: String,
    /// Why `try_compile` returned `TooComplex`, if applicable.
    pub compile_failure_reason: Option<String>,
}

// ── CatalogError ──────────────────────────────────────────────────────────────

/// Error variants from [`Catalog::try_compile`] and related routing decisions.
///
/// [`Catalog::try_compile`]: super::Catalog::try_compile
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogError {
    /// The query is too complex for direct compilation; the LLM must handle it.
    ///
    /// The `String` contains a human-readable reason for debugging
    /// (e.g. "filters contain SQL functions", "airlayer compile error: ...").
    TooComplex(String),
    /// The named metric is unknown to this catalog.
    UnresolvableMetric(String),
    /// The named dimension is unknown or unreachable via any join path.
    UnresolvableDimension(String),
}
