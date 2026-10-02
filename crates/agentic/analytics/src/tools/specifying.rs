//! Executor for **specifying**-state tools (`get_join_path`, `sample_columns`).

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::{DatabaseConnector, StringLiteral};
use agentic_core::result::CellValue;
use agentic_core::tools::ToolError;
use serde_json::{Value, json};

use crate::catalog::Catalog;

use super::{cell_to_json, emit_tool_error, emit_tool_input, emit_tool_output};

/// Execute a **specifying** tool.
///
/// `catalog` is used for `get_join_path` lookups.
/// `connectors` + `default_connector` back `sample_columns`: each sampled
/// column routes to the connector named by its semantic-view `datasource:`,
/// falling back to `default_connector` when the view has none or the named
/// connector isn't registered.
#[tracing::instrument(
    skip(catalog, connectors),
    fields(oxy.name = "analytics.tool", oxy.span_type = "analytics", tool = %name)
)]
pub async fn execute_specifying_tool(
    name: &str,
    params: Value,
    catalog: &dyn Catalog,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default_connector: &str,
) -> Result<Value, ToolError> {
    emit_tool_input(name, &params);
    let result =
        execute_specifying_tool_inner(name, params, catalog, connectors, default_connector).await;
    match &result {
        Ok(v) => emit_tool_output(v),
        Err(e) => emit_tool_error(e),
    }
    result
}

async fn execute_specifying_tool_inner(
    name: &str,
    params: Value,
    catalog: &dyn Catalog,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default_connector: &str,
) -> Result<Value, ToolError> {
    match name {
        "get_join_path" => {
            let from = params["from_entity"]
                .as_str()
                .ok_or_else(|| ToolError::BadParams("missing 'from_entity'".into()))?;
            let to = params["to_entity"]
                .as_str()
                .ok_or_else(|| ToolError::BadParams("missing 'to_entity'".into()))?;
            match catalog.get_join_path(from, to) {
                Some(jp) => Ok(json!({ "path": jp.path, "join_type": jp.join_type })),
                None => {
                    let available = catalog.table_names().join(", ");
                    Err(ToolError::Execution(format!(
                        "no registered join path between '{from}' and '{to}'. \
                         You can still join these tables: use a bare column name if both tables \
                         share the same name (e.g. `Date`), or a \
                         `left_table.left_col = right_table.right_col` expression when the \
                         column names differ (e.g. `macro.Date = strength.workout_date`). \
                         Inspect both tables' columns to find the right key. \
                         Available tables: [{available}]."
                    )))
                }
            }
        }

        "check_data_freshness" => {
            let views = params["views"]
                .as_array()
                .ok_or_else(|| ToolError::BadParams("missing 'views' array".into()))?;
            if views.is_empty() {
                return Err(ToolError::BadParams(
                    "'views' array must not be empty".into(),
                ));
            }
            let names: Vec<&str> = views
                .iter()
                .map(|v| {
                    v.as_str().ok_or_else(|| {
                        ToolError::BadParams("each view entry must be a string".into())
                    })
                })
                .collect::<Result<Vec<_>, ToolError>>()?;

            let futures: Vec<_> = names
                .iter()
                .map(|view| {
                    check_single_view_freshness(view, catalog, connectors, default_connector)
                })
                .collect();
            let results = futures::future::join_all(futures).await;

            // Individual view errors become inline error objects rather than
            // failing the whole batch — same convention as sample_columns.
            let results_json: Vec<Value> = results
                .into_iter()
                .enumerate()
                .map(|(i, r)| match r {
                    Ok(v) => v,
                    Err(e) => json!({ "view": names[i], "error": e.to_string() }),
                })
                .collect();

            Ok(json!({ "results": results_json }))
        }

        "sample_columns" => {
            let columns = params["columns"]
                .as_array()
                .ok_or_else(|| ToolError::BadParams("missing 'columns' array".into()))?;
            if columns.is_empty() {
                return Err(ToolError::BadParams(
                    "'columns' array must not be empty".into(),
                ));
            }

            // Collect validated column specs before spawning futures.
            let specs: Vec<(&str, &str, Option<&str>)> = columns
                .iter()
                .map(|entry| {
                    let table = entry["table"].as_str().ok_or_else(|| {
                        ToolError::BadParams("each column entry requires 'table'".into())
                    })?;
                    let column = entry["column"].as_str().ok_or_else(|| {
                        ToolError::BadParams("each column entry requires 'column'".into())
                    })?;
                    let search_term = entry
                        .get("search_term")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    Ok((table, column, search_term))
                })
                .collect::<Result<Vec<_>, ToolError>>()?;

            // Run all column samples concurrently.
            let futures: Vec<_> = specs
                .iter()
                .map(|(table, column, search_term)| {
                    sample_single_column(
                        table,
                        column,
                        *search_term,
                        catalog,
                        connectors,
                        default_connector,
                    )
                })
                .collect();
            let results = futures::future::join_all(futures).await;

            // Collect results — individual column errors become inline error objects
            // rather than failing the whole batch.
            let results_json: Vec<Value> = results
                .into_iter()
                .enumerate()
                .map(|(i, r)| match r {
                    Ok(v) => v,
                    Err(e) => json!({
                        "table": specs[i].0,
                        "column": specs[i].1,
                        "error": e.to_string()
                    }),
                })
                .collect();

            Ok(json!({ "results": results_json }))
        }

        _ => Err(ToolError::UnknownTool(name.into())),
    }
}

/// Probe a single view's data freshness: resolve its watermark via the
/// catalog, run `MAX(<watermark>)` against the view's connector, and compute
/// staleness relative to today (UTC). Used by `check_data_freshness` (batch)
/// above.
///
/// The declared freshness contract (`expected_cadence`, `complete_through`,
/// `caveat` from the view's `meta:`) is passed through verbatim so the LLM
/// can judge whether the observed staleness is normal for the source (a
/// monthly-loaded source that is 20 days behind is healthy; an hourly one
/// is not).
async fn check_single_view_freshness(
    view_param: &str,
    catalog: &dyn Catalog,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default_connector: &str,
) -> Result<Value, ToolError> {
    let target = catalog
        .resolve_freshness_target(view_param)
        .ok_or_else(|| {
            ToolError::Execution(format!(
                "no freshness target for '{view_param}': not a known semantic view, or it has \
             no date/datetime dimension and no meta.freshness_watermark_column"
            ))
        })?;

    // Connector routing: same rule as sample_single_column, and the same rule
    // `solver::freshness_hint` applies to the ambient version of this probe.
    // A view's `datasource:` is honoured or the call fails -- it is never
    // swapped for the default, because MAX(watermark) measured on a different
    // engine is a confident wrong date the model then states as fact.
    let connector: &dyn DatabaseConnector = {
        let name = target.datasource.as_deref().unwrap_or(default_connector);
        crate::solver::lookup_connector(connectors, name)
            .map(|(_, c)| c)
            .ok_or_else(|| {
                ToolError::Execution(format!(
                    "no connector registered for '{name}' (requested for view '{view_param}')"
                ))
            })?
            .as_ref()
    };

    let table_sql = format!("\"{}\"", target.table.replace('"', "\"\""));
    let max_sql = format!(
        "SELECT MAX({expr}) FROM {table_sql}",
        expr = target.watermark_expr
    );
    let res = connector
        .execute_query(&max_sql, 1)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    let data_through = res
        .result
        .rows
        .first()
        .and_then(|row| row.0.first())
        .map(cell_to_json)
        .unwrap_or(Value::Null);

    let mut out = json!({
        "view": target.view,
        "table": target.table,
        "watermark": target.watermark_name,
        "watermark_declared": target.declared,
        "data_through": data_through,
    });
    if let Some(days) = staleness_days(&out["data_through"]) {
        out["staleness_days"] = json!(days);
    }
    if let Some(c) = target.expected_cadence {
        out["expected_cadence"] = json!(c);
    }
    if let Some(c) = target.complete_through {
        out["complete_through"] = json!(c);
    }
    if let Some(c) = target.caveat {
        out["caveat"] = json!(c);
    }
    if let Some(sql) = target.refresh_key_sql
        && let Ok(rk) = connector.execute_query(&sql, 1).await
        && let Some(cell) = rk.result.rows.first().and_then(|row| row.0.first())
    {
        out["refresh_key_value"] = cell_to_json(cell);
    }
    Ok(out)
}

/// Days between a watermark value and today (UTC). Handles ISO strings
/// (`2026-07-14`, with or without a time suffix) and numeric `YYYYMMDD`
/// values (e.g. ClickHouse `Int64` business dates). Returns `None` when the
/// value has no recognizable date, in which case the raw `data_through` is
/// still reported without a staleness figure.
fn staleness_days(value: &Value) -> Option<i64> {
    let date = if let Some(s) = value.as_str() {
        chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?
    } else {
        let n = value.as_f64()?;
        let n = n as i64;
        if !(19000101..=29991231).contains(&n) {
            return None;
        }
        chrono::NaiveDate::from_ymd_opt(
            (n / 10000) as i32,
            ((n / 100) % 100) as u32,
            (n % 100) as u32,
        )?
    };
    Some((chrono::Utc::now().date_naive() - date).num_days())
}

/// Sample a single column: resolve via catalog, query the database for distinct
/// values and statistics.  Used by `sample_columns` (batch) above.
///
/// Picks the connector named by the view's `datasource:` (via `SampleTarget`),
/// falling back to `default_connector` only when the view names none. A
/// named-but-unregistered datasource is an error, not a reroute.
async fn sample_single_column(
    table_param: &str,
    column_param: &str,
    search_term: Option<&str>,
    catalog: &dyn Catalog,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default_connector: &str,
) -> Result<Value, ToolError> {
    // Resolve semantic view/dimension names to physical table/column.
    let resolved = catalog.resolve_sample_target(table_param, column_param);

    // Select the connector matching the view's datasource.  Without this
    // routing, a view declaring `datasource: local` would still be queried
    // against whichever connector happens to be the default — which is how
    // `oxymart.csv` lookups used to fail with "no such file" on the `training`
    // connector.
    //
    // Named-but-unregistered falls through to the error below rather than the
    // default: these samples become the literals the model writes into
    // `WHERE ... IN (...)`, so values read off the wrong engine are worse than
    // no values at all.
    let connector: &dyn DatabaseConnector = {
        let name = resolved
            .as_ref()
            .and_then(|t| t.datasource.as_deref())
            .unwrap_or(default_connector);
        crate::solver::lookup_connector(connectors, name)
            .map(|(_, c)| c)
            .ok_or_else(|| {
                ToolError::Execution(format!(
                    "no connector registered for '{name}' (requested for table '{table_param}')"
                ))
            })?
            .as_ref()
    };

    // If the semantic model has static samples, return them directly.
    // When a search_term is provided, filter the static samples.
    if let Some(ref target) = resolved
        && !target.static_samples.is_empty()
    {
        let values: Vec<Value> = target
            .static_samples
            .iter()
            .filter(|s| {
                search_term
                    .map(|term| s.to_lowercase().contains(&term.to_lowercase()))
                    .unwrap_or(true)
            })
            .map(|s| Value::String(s.clone()))
            .collect();
        return Ok(json!({
            "column": column_param,
            "table": table_param,
            "data_type": target.data_type.as_deref().unwrap_or("string"),
            "sample_values": values,
            "source": "semantic_layer",
            "hint": "These are pre-defined sample values from the semantic model definition."
        }));
    }

    // When the semantic model resolves the target, `column_expr` is
    // already a SQL expression (e.g. `"Datetime (Local)"` with its own
    // quoting).  Use it verbatim instead of wrapping in extra quotes.
    let (table_sql, col_sql) = match &resolved {
        Some(target) => {
            let t = format!("\"{}\"", target.table.replace('"', "\"\""));
            // column_expr from the semantic model is a raw SQL
            // expression — use it as-is.
            (t, target.column_expr.clone())
        }
        None => (
            format!("\"{}\"", table_param.replace('"', "\"\"")),
            format!("\"{}\"", column_param.replace('"', "\"\"")),
        ),
    };

    let sample_sql = sample_values_sql(
        &col_sql,
        &table_sql,
        search_term,
        connector.string_literal(),
    );
    let count_sql = format!("SELECT COUNT(*) FROM {table_sql}");

    let sample_res = connector
        .execute_query(&sample_sql, 20)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let values: Vec<Value> = sample_res
        .result
        .rows
        .iter()
        .filter_map(|row| row.0.first())
        .map(cell_to_json)
        .collect();

    let data_type = sample_res
        .summary
        .columns
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();

    // Detect column kind from sample values.
    // A value looks like a date if it starts with four digits followed by '-'.
    let is_date_col = values.iter().any(|v| {
        v.as_str()
            .map(|s| {
                s.len() >= 5
                    && s[..4].chars().all(|c| c.is_ascii_digit())
                    && s.as_bytes()[4] == b'-'
            })
            .unwrap_or(false)
    });
    // Numeric: all sampled values are JSON numbers (not dates).
    let is_numeric_col = !is_date_col && !values.is_empty() && values.iter().all(|v| v.is_number());

    // Helper to extract a u64 from a cell.
    let cell_u64 = |cell: &CellValue| -> Option<u64> {
        match cell {
            CellValue::Number(n) => Some(*n as u64),
            CellValue::Text(s) => s.parse().ok(),
            CellValue::Null => None,
        }
    };
    // Helper to extract a f64 from a cell.
    let cell_f64 = |cell: &CellValue| -> Option<f64> {
        match cell {
            CellValue::Number(n) => Some(*n),
            CellValue::Text(s) => s.parse().ok(),
            CellValue::Null => None,
        }
    };
    // Helper to extract a string from a cell.
    let cell_str = |cell: &CellValue| -> Option<String> {
        match cell {
            CellValue::Text(s) => Some(s.clone()),
            CellValue::Number(n) => Some(n.to_string()),
            CellValue::Null => None,
        }
    };

    let base = json!({ "column": column_param, "table": table_param });

    if is_numeric_col {
        // Numeric columns: fetch count, distinct count, min, max, avg, stdev in one query.
        let stats_sql = format!(
            "SELECT COUNT(*), COUNT(DISTINCT {col_sql}), MIN({col_sql}), MAX({col_sql}), \
             AVG({col_sql}), STDDEV_POP({col_sql}) FROM {table_sql}"
        );
        if let Ok(stats_res) = connector.execute_query(&stats_sql, 1).await
            && let Some(row) = stats_res.result.rows.first()
        {
            let row_count = row.0.first().and_then(&cell_u64).unwrap_or(0);
            let distinct_count = row.0.get(1).and_then(&cell_u64);
            let min_val = row.0.get(2).and_then(&cell_f64);
            let max_val = row.0.get(3).and_then(&cell_f64);
            let avg_val = row.0.get(4).and_then(&cell_f64);
            let stdev_val = row.0.get(5).and_then(cell_f64);
            let mut r = base;
            r["data_type"] = json!(data_type);
            r["sample_values"] = json!(values);
            r["row_count"] = json!(row_count);
            r["distinct_count"] = json!(distinct_count);
            r["min"] = json!(min_val);
            r["max"] = json!(max_val);
            r["avg"] = json!(avg_val);
            r["stdev"] = json!(stdev_val);
            return Ok(r);
        }
    } else {
        // Date and text columns: fetch count, distinct count, min, max.
        let stats_sql = format!(
            "SELECT COUNT(*), COUNT(DISTINCT {col_sql}), MIN({col_sql}), MAX({col_sql}) \
             FROM {table_sql}"
        );
        if let Ok(stats_res) = connector.execute_query(&stats_sql, 1).await
            && let Some(row) = stats_res.result.rows.first()
        {
            let row_count = row.0.first().and_then(&cell_u64).unwrap_or(0);
            let distinct_count = row.0.get(1).and_then(cell_u64);
            let min_val = row.0.get(2).and_then(&cell_str);
            let max_val = row.0.get(3).and_then(cell_str);
            let mut r = base;
            r["data_type"] = json!(data_type);
            r["sample_values"] = json!(values);
            r["row_count"] = json!(row_count);
            r["distinct_count"] = json!(distinct_count);
            r["min"] = json!(min_val);
            r["max"] = json!(max_val);
            return Ok(r);
        }
    }

    // Fallback: just return sample values with basic count.
    let count_res = connector
        .execute_query(&count_sql, 1)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    let row_count: u64 = count_res
        .result
        .rows
        .first()
        .and_then(|row| row.0.first())
        .and_then(|cell| match cell {
            CellValue::Number(n) => Some(*n as u64),
            CellValue::Text(s) => s.parse().ok(),
            CellValue::Null => None,
        })
        .unwrap_or(0);
    let mut r = base;
    r["data_type"] = json!(data_type);
    r["sample_values"] = json!(values);
    r["row_count"] = json!(row_count);
    Ok(r)
}

/// The `sample_columns` read: up to 20 distinct values of `col_sql`, narrowed
/// to those containing `search_term` when one is given.
///
/// The term is the model's, written from what the user asked, so it is quoted
/// as the connector's engine reads a literal — a backslash in it must not end
/// the pattern early on an engine that reads one.
fn sample_values_sql(
    col_sql: &str,
    table_sql: &str,
    search_term: Option<&str>,
    strings: StringLiteral,
) -> String {
    match search_term {
        Some(term) => {
            let pattern = strings.quote(&format!("%{term}%"));
            format!(
                "SELECT DISTINCT {col_sql} FROM {table_sql} WHERE {col_sql} IS NOT NULL AND CAST({col_sql} AS TEXT) LIKE {pattern} LIMIT 20"
            )
        }
        None => {
            format!(
                "SELECT DISTINCT {col_sql} FROM {table_sql} WHERE {col_sql} IS NOT NULL LIMIT 20"
            )
        }
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::staleness_days;
    use serde_json::json;

    #[test]
    fn staleness_from_iso_string() {
        let today = chrono::Utc::now().date_naive();
        assert_eq!(staleness_days(&json!(today.to_string())), Some(0));
        let week_ago = today - chrono::Duration::days(7);
        assert_eq!(staleness_days(&json!(week_ago.to_string())), Some(7));
    }

    #[test]
    fn staleness_from_iso_datetime_string() {
        let today = chrono::Utc::now().date_naive();
        let v = json!(format!("{today} 22:19:16"));
        assert_eq!(staleness_days(&v), Some(0));
    }

    #[test]
    fn staleness_from_yyyymmdd_number() {
        let today = chrono::Utc::now().date_naive();
        let n: i64 = format!("{}", today.format("%Y%m%d")).parse().unwrap();
        assert_eq!(staleness_days(&json!(n)), Some(0));
    }

    #[test]
    fn staleness_none_for_unrecognizable() {
        assert_eq!(staleness_days(&json!("not a date")), None);
        assert_eq!(staleness_days(&json!(42)), None);
        assert_eq!(staleness_days(&serde_json::Value::Null), None);
    }
}

#[cfg(test)]
mod sample_sql_tests {
    use super::sample_values_sql;
    use agentic_connector::StringLiteral;

    fn pattern(term: &str, rule: StringLiteral) -> String {
        let sql = sample_values_sql("\"c\"", "\"t\"", Some(term), rule);
        let (_, after) = sql.split_once(" LIKE ").expect("a LIKE clause");
        after
            .strip_suffix(" LIMIT 20")
            .expect("the pattern is the last thing before LIMIT")
            .to_string()
    }

    /// A term with no quote and no backslash is spelled as it always was.
    #[test]
    fn a_plain_term_is_unchanged_on_every_engine() {
        for rule in [
            StringLiteral::Standard,
            StringLiteral::Backslash,
            StringLiteral::BackslashOnly,
        ] {
            assert_eq!(
                sample_values_sql("\"c\"", "\"t\"", Some("east"), rule),
                "SELECT DISTINCT \"c\" FROM \"t\" WHERE \"c\" IS NOT NULL AND CAST(\"c\" AS TEXT) LIKE '%east%' LIMIT 20",
                "{rule:?}"
            );
        }
        assert_eq!(
            sample_values_sql("\"c\"", "\"t\"", None, StringLiteral::Backslash),
            "SELECT DISTINCT \"c\" FROM \"t\" WHERE \"c\" IS NOT NULL LIMIT 20"
        );
    }

    /// Quote-only escaping left the third term outside its literal on every
    /// engine that reads a backslash, and let the fourth swallow the quote.
    #[test]
    fn a_term_is_quoted_as_its_engine_reads_a_literal() {
        let terms = ["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"];
        let on = |rule| terms.map(|t| pattern(t, rule));
        assert_eq!(
            on(StringLiteral::Standard),
            ["'%a\\b%'", "'%it''s%'", "'%x\\'' OR 1=1 -- %'", "'%C:\\%'"]
        );
        assert_eq!(
            on(StringLiteral::Backslash),
            [
                "'%a\\\\b%'",
                "'%it''s%'",
                "'%x\\\\'' OR 1=1 -- %'",
                "'%C:\\\\%'"
            ]
        );
        assert_eq!(
            on(StringLiteral::BackslashOnly),
            [
                "'%a\\\\b%'",
                "'%it\\'s%'",
                "'%x\\\\\\' OR 1=1 -- %'",
                "'%C:\\\\%'"
            ]
        );
    }
}
