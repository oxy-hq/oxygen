//! Validation functions for the three pipeline stages.
//!
//! | Function | Stage | Checks |
//! |---|---|---|
//! | [`validate_specified`] | `specify` | metrics/joins/filters resolve to real columns |
//! | [`validate_solvable`]  | `solve`   | SQL is syntactically sound and tables exist |
//! | [`validate_solved`]    | `execute` | results non-empty, shape matches, values plausible |
//!
//! ## Declarative validation via YAML config
//!
//! Use [`Validator`] to run a user-configured subset of rules. A `validation:`
//! section replaces the default rule set rather than adjusting it, so this one
//! runs outlier detection and no other rule, in any stage:
//!
//! ```yaml
//! validation:
//!   rules:
//!     solved:
//!       - name: outlier_detection
//!         enabled: true
//!         threshold_sigma: 3.0
//!         min_rows: 6
//! ```
//!
//! ```rust,ignore
//! let validator = match &agent_config.validation {
//!     Some(cfg) => Validator::from_config(cfg)?,
//!     None => Validator::default_validator(),
//! };
//! validator.validate_solved(&result, &spec)?;
//! ```

pub mod config;
pub mod registry;
pub mod rule;
pub mod validator;

mod solvable;
mod solved;
mod specified;

pub use config::ValidationConfig;
#[allow(unused_imports)]
pub use registry::RegistryError;
#[allow(unused_imports)]
pub use solvable::validate_solvable;
#[allow(unused_imports)]
pub use solved::validate_solved;
#[allow(unused_imports)]
pub use specified::validate_specified;
pub use validator::Validator;

// Backward-compatible free functions — delegate to Validator::default().

// Shared helper: extract table.column references from a SQL expression

/// Extract all `table.column` references from a SQL expression.
///
/// Handles:
/// - Unquoted identifiers: `table.column`
/// - Backtick-quoted identifiers (DuckDB / MySQL): `` table.`column name` ``
/// - Double-quoted identifiers (PostgreSQL): `"table"."column"`
///
/// Returns an empty `Vec` when the expression contains no qualified
/// references (e.g. `COUNT(*)`).
///
/// Bug-fix #4: double-quoted identifiers (`"col"`) are now handled in
/// addition to backtick-quoted identifiers.
pub(super) fn extract_table_column_refs(expr: &str) -> Vec<(String, String)> {
    let bytes = expr.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut refs = Vec::new();

    while i < n {
        let b = bytes[i];

        // ── Double-quoted identifier start ────────────────────────────────
        // PostgreSQL: `"table"."column"` or `"schema"."table"."column"`.
        if b == b'"' {
            let (ident, next_i) = read_quoted_ident(bytes, i, b'"');
            if ident.is_empty() {
                i = next_i;
                continue;
            }
            if next_i < n && bytes[next_i] == b'.' {
                // Look for a second quoted or unquoted identifier.
                let after_dot = next_i + 1;
                if after_dot < n && bytes[after_dot] == b'"' {
                    let (col, col_end) = read_quoted_ident(bytes, after_dot, b'"');
                    if !col.is_empty() {
                        refs.push((ident.to_lowercase(), col.to_lowercase()));
                        i = col_end;
                        continue;
                    }
                    i = col_end;
                } else if after_dot < n
                    && (bytes[after_dot].is_ascii_alphabetic() || bytes[after_dot] == b'_')
                {
                    let (col, col_end) = read_unquoted_ident(bytes, after_dot);
                    if !col.is_empty() {
                        refs.push((ident.to_lowercase(), col.to_lowercase()));
                        i = col_end;
                        continue;
                    }
                    i = col_end;
                } else {
                    i = after_dot;
                }
            } else {
                i = next_i;
            }
            continue;
        }

        // ── Unquoted identifier start ──────────────────────────────────────
        if b.is_ascii_alphabetic() || b == b'_' {
            let (table, next_i) = read_unquoted_ident(bytes, i);

            if next_i < n && bytes[next_i] == b'.' {
                let after_dot = next_i + 1;

                if after_dot < n && bytes[after_dot] == b'`' {
                    // Backtick-quoted column: `column name`.
                    let (col, col_end) = read_quoted_ident(bytes, after_dot, b'`');
                    if !col.is_empty() {
                        refs.push((table.to_lowercase(), col.to_lowercase()));
                        i = col_end;
                        continue;
                    }
                    i = col_end;
                } else if after_dot < n && bytes[after_dot] == b'"' {
                    // Double-quoted column after unquoted table.
                    let (col, col_end) = read_quoted_ident(bytes, after_dot, b'"');
                    if !col.is_empty() {
                        refs.push((table.to_lowercase(), col.to_lowercase()));
                        i = col_end;
                        continue;
                    }
                    i = col_end;
                } else if after_dot < n
                    && (bytes[after_dot].is_ascii_alphabetic() || bytes[after_dot] == b'_')
                {
                    let (col, col_end) = read_unquoted_ident(bytes, after_dot);
                    if !col.is_empty() {
                        refs.push((table.to_lowercase(), col.to_lowercase()));
                        i = col_end;
                        continue;
                    }
                    i = col_end;
                } else {
                    i = after_dot;
                }
            } else {
                i = next_i;
            }
            continue;
        }

        i += 1;
    }

    refs
}

/// Read a quoted identifier (backtick or double-quote) starting at `start`
/// (which points to the opening quote character).  Returns `(ident, end_i)`
/// where `end_i` is the index after the closing quote (or `bytes.len()` if
/// unterminated).
fn read_quoted_ident(bytes: &[u8], start: usize, quote: u8) -> (String, usize) {
    let n = bytes.len();
    let mut i = start + 1; // skip opening quote
    let content_start = i;
    while i < n && bytes[i] != quote {
        i += 1;
    }
    let ident = String::from_utf8_lossy(&bytes[content_start..i]).into_owned();
    if i < n {
        i += 1; // skip closing quote
    }
    (ident, i)
}

/// Read an unquoted identifier starting at `start`.  Returns `(ident, end_i)`.
fn read_unquoted_ident(bytes: &[u8], start: usize) -> (String, usize) {
    let n = bytes.len();
    let mut i = start;
    while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    let ident = String::from_utf8_lossy(&bytes[start..i]).into_owned();
    (ident, i)
}

// Shared test fixtures

#[cfg(test)]
pub(crate) mod test_fixtures {
    use crate::semantic::SemanticCatalog;
    use crate::{
        AnalyticsIntent, AnalyticsResult, QuerySpec, QuestionType, ResultShape, SchemaCatalog,
    };
    use agentic_core::result::CellValue;
    use agentic_core::{QueryResult, QueryRow};

    #[allow(dead_code)]
    pub fn sample_schema_catalog() -> SchemaCatalog {
        SchemaCatalog::new()
            .add_table(
                "orders",
                &["order_id", "customer_id", "revenue", "date", "status"],
            )
            .add_table("customers", &["customer_id", "region", "name"])
            .add_table("products", &["product_id", "category", "price"])
            .add_join_key("orders", "customers", "customer_id")
    }

    /// Returns a [`SemanticCatalog`] with views matching the old schema fixture.
    ///
    /// Views: orders (order_id, customer_id, revenue, date, status),
    ///        customers (customer_id, region, name),
    ///        products (product_id, category, price).
    /// Join: orders ↔ customers via customer_id.
    pub fn sample_catalog() -> SemanticCatalog {
        use crate::airlayer_compat;
        let orders_yaml = r#"
name: orders
description: Order data
table: orders
entities:
  - name: order_pk
    type: primary
    key: order_id
  - name: customer_fk
    type: primary
    key: customer_id
dimensions:
  - name: order_id
    type: number
    expr: order_id
  - name: customer_id
    type: number
    expr: customer_id
  - name: date
    type: date
    expr: date
  - name: status
    type: string
    expr: status
measures:
  - name: revenue
    type: sum
    expr: revenue
"#;
        let customers_yaml = r#"
name: customers
description: Customer dimension
table: customers
entities:
  - name: customer_fk
    type: foreign
    key: customer_id
dimensions:
  - name: customer_id
    type: number
    expr: customer_id
  - name: region
    type: string
    expr: region
  - name: name
    type: string
    expr: name
"#;
        let products_yaml = r#"
name: products
description: Product catalog
table: products
dimensions:
  - name: product_id
    type: number
    expr: product_id
  - name: category
    type: string
    expr: category
  - name: price
    type: number
    expr: price
"#;
        let views = vec![
            airlayer_compat::parse_view_yaml(orders_yaml).unwrap(),
            airlayer_compat::parse_view_yaml(customers_yaml).unwrap(),
            airlayer_compat::parse_view_yaml(products_yaml).unwrap(),
        ];
        let layer = oxy_airlayer_compat::SemanticLayer::new(views, None);
        let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
            oxy_airlayer_compat::Dialect::DuckDB,
        );
        let engine =
            oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer, dialects).unwrap();
        SemanticCatalog::from_engine(engine)
    }

    pub fn make_intent() -> AnalyticsIntent {
        AnalyticsIntent {
            raw_question: "What is total revenue by region?".into(),
            summary: "Total revenue broken down by region".into(),
            question_type: QuestionType::Breakdown,
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            filters: vec![],
            history: vec![],
            spec_hint: None,
            selected_automation: None,
            semantic_query: Default::default(),
            semantic_confidence: 0.0,
        }
    }

    pub fn make_spec() -> QuerySpec {
        QuerySpec {
            intent: make_intent(),
            resolved_metrics: vec!["orders.revenue".into()],
            resolved_filters: vec![],
            resolved_tables: vec!["orders".into(), "customers".into()],
            join_path: vec![("orders".into(), "customers".into(), "customer_id".into())],
            expected_result_shape: ResultShape::Table {
                columns: vec!["region".into(), "revenue".into()],
            },
            assumptions: vec![],
            solution_source: Default::default(),
            precomputed: None,
            context: None,
            connector_name: "default".to_string(),
            query_request_item: None,
            query_request: None,
            compile_error: None,
        }
    }

    pub fn scalar_result(value: f64) -> AnalyticsResult {
        AnalyticsResult::single(
            QueryResult {
                columns: vec!["total".into()],
                rows: vec![QueryRow(vec![CellValue::Number(value)])],
                total_row_count: 1,
                truncated: false,
            },
            None,
        )
    }

    pub fn series_result(values: &[f64]) -> AnalyticsResult {
        let rows: Vec<QueryRow> = values
            .iter()
            .map(|&v| QueryRow(vec![CellValue::Number(v)]))
            .collect();
        let total_row_count = rows.len() as u64;
        AnalyticsResult::single(
            QueryResult {
                columns: vec!["value".into()],
                rows,
                total_row_count,
                truncated: false,
            },
            None,
        )
    }

    pub fn table_result(columns: Vec<String>, rows: Vec<Vec<CellValue>>) -> AnalyticsResult {
        let total_row_count = rows.len() as u64;
        AnalyticsResult::single(
            QueryResult {
                columns,
                rows: rows.into_iter().map(QueryRow).collect(),
                total_row_count,
                truncated: false,
            },
            None,
        )
    }

    pub fn timeseries_result() -> AnalyticsResult {
        AnalyticsResult::single(
            QueryResult {
                columns: vec!["date".into(), "revenue".into()],
                rows: vec![
                    QueryRow(vec![
                        CellValue::Text("2024-01".into()),
                        CellValue::Number(100.0),
                    ]),
                    QueryRow(vec![
                        CellValue::Text("2024-02".into()),
                        CellValue::Number(120.0),
                    ]),
                    QueryRow(vec![
                        CellValue::Text("2024-03".into()),
                        CellValue::Number(90.0),
                    ]),
                ],
                total_row_count: 3,
                truncated: false,
            },
            None,
        )
    }
}

// Tests for extract_table_column_refs

#[cfg(test)]
mod tests {
    use super::extract_table_column_refs;

    #[test]
    fn extracts_simple_dotted() {
        let refs = extract_table_column_refs("orders.revenue");
        assert_eq!(refs, vec![("orders".into(), "revenue".into())]);
    }

    #[test]
    fn extracts_backtick_quoted_column() {
        let refs = extract_table_column_refs("orders.`gross revenue`");
        assert_eq!(refs, vec![("orders".into(), "gross revenue".into())]);
    }

    /// Bug-fix #4: double-quoted identifiers now extracted correctly.
    #[test]
    fn extracts_double_quoted_identifiers() {
        let refs = extract_table_column_refs(r#""orders"."revenue""#);
        assert_eq!(refs, vec![("orders".into(), "revenue".into())]);
    }

    #[test]
    fn extracts_double_quoted_table_unquoted_col() {
        let refs = extract_table_column_refs(r#""orders".revenue"#);
        assert_eq!(refs, vec![("orders".into(), "revenue".into())]);
    }

    #[test]
    fn extracts_unquoted_table_double_quoted_col() {
        let refs = extract_table_column_refs(r#"orders."revenue""#);
        assert_eq!(refs, vec![("orders".into(), "revenue".into())]);
    }

    #[test]
    fn no_refs_for_count_star() {
        assert!(extract_table_column_refs("COUNT(*)").is_empty());
    }

    #[test]
    fn extracts_multiple_refs() {
        let refs = extract_table_column_refs("orders.revenue + customers.discount");
        assert!(refs.contains(&("orders".into(), "revenue".into())));
        assert!(refs.contains(&("customers".into(), "discount".into())));
    }

    #[test]
    fn lowercases_identifiers() {
        let refs = extract_table_column_refs("Orders.Revenue");
        assert_eq!(refs, vec![("orders".into(), "revenue".into())]);
    }
}
