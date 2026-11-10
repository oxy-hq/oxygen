//! Pure helpers for reconcile operands: build the semantic `QueryRequest`
//! (window injected), render SQL templates, coerce a SQL scalar cell to
//! `f64`, and shape a grouped SQL result into `(key, value)` rows. No I/O —
//! all unit-testable.

use agentic_core::result::{CellValue, QueryRow};
use oxy_airlayer_compat::engine::query::{QueryRequest, TimeDimensionQuery};

use super::ResolvedWindow;

/// Clone the user's semantic query and OWN the time window: overwrite
/// `time_dimensions` with the reconcile window bound to `time_dimension`, and
/// carry the window's timezone onto the request. Any user-supplied
/// `time_dimensions` is intentionally replaced — reconcile always controls the
/// comparison period so both sides align.
pub fn semantic_request(
    query: &QueryRequest,
    time_dimension: &str,
    window: &ResolvedWindow,
) -> QueryRequest {
    let mut req = query.clone();
    req.time_dimensions = vec![TimeDimensionQuery {
        dimension: time_dimension.to_string(),
        granularity: None,
        date_range: Some(window.dates.to_vec()),
    }];
    // Leave UTC unset. It is equivalent to Some("UTC") in the SQL generator,
    // but `None` is what every pre-timezone config produced and it also keeps
    // pre-aggregation rollup matching on its existing path.
    if window.timezone != "UTC" {
        req.timezone = Some(window.timezone.clone());
    }
    req
}

/// The request a grouped semantic operand runs, and the result members its
/// group key is read from (in `members` order). A member that is the operand's
/// own `time_dimension` is not sent as a plain dimension: it buckets the
/// window's time dimension by day instead, so a datetime column yields one
/// local date per group (in the window's timezone, which `semantic_request`
/// carries) rather than a raw timestamp that joins nothing.
pub fn grouped_semantic_request(
    query: &QueryRequest,
    time_dimension: &str,
    window: &ResolvedWindow,
    members: &[String],
) -> (QueryRequest, Vec<String>) {
    let mut req = semantic_request(query, time_dimension, window);
    req.dimensions = members
        .iter()
        .filter(|m| m.as_str() != time_dimension)
        .cloned()
        .collect();
    let keys = members
        .iter()
        .map(|m| {
            if m == time_dimension {
                req.time_dimensions[0].granularity = Some("day".to_string());
                format!("{m}.day")
            } else {
                m.clone()
            }
        })
        .collect();
    (req, keys)
}

/// Render a reconcile SQL template, binding `start_date` / `end_date` from the
/// resolved window plus the `timezone` they were resolved in, so an author can
/// convert their column to the same calendar. Values are rendered BARE — the
/// SQL author quotes them.
pub fn render_sql(sql: &str, window: &ResolvedWindow) -> Result<String, String> {
    minijinja::Environment::new()
        .render_str(
            sql,
            minijinja::context! {
                start_date => window.dates[0],
                end_date => window.dates[1],
                timezone => window.timezone,
            },
        )
        .map_err(|e| format!("reconcile sql template render failed: {e}"))
}

/// Coerce the first cell of a SQL scalar result to `f64`.
pub fn cell_to_f64(cell: &CellValue) -> Result<f64, String> {
    match cell {
        CellValue::Number(n) => Ok(*n),
        CellValue::Text(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("reconcile sql returned non-numeric value '{s}'")),
        CellValue::Null => Err("reconcile sql returned NULL".to_string()),
    }
}

/// Shape a grouped `sql` operand's result into `(key, value)` rows. Each key
/// is read from the column named like it (case-insensitive), in `keys` order;
/// the value is the one remaining column, or the column named `value` when
/// more than one remains.
pub fn grouped_rows(
    columns: &[String],
    rows: &[QueryRow],
    keys: &[String],
) -> Result<Vec<(Vec<String>, f64)>, String> {
    let find = |name: &str| columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let key_idx = keys
        .iter()
        .map(|k| {
            find(k).ok_or_else(|| {
                format!(
                    "reconcile sql: group_by key '{k}' is not a column of the result \
                     (columns: {})",
                    columns.join(", ")
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rest: Vec<usize> = (0..columns.len())
        .filter(|i| !key_idx.contains(i))
        .collect();
    let value_idx = match rest.as_slice() {
        [only] => *only,
        _ => find("value").filter(|i| rest.contains(i)).ok_or_else(|| {
            format!(
                "reconcile sql: a grouped result needs one value column besides the \
                 group_by keys, or one named 'value' (columns: {})",
                columns.join(", ")
            )
        })?,
    };
    rows.iter()
        .map(|row| {
            let cell = |i: usize| row.0.get(i).unwrap_or(&CellValue::Null);
            let key: Vec<String> = key_idx.iter().map(|&i| key_text(cell(i))).collect();
            let value = cell_to_f64(cell(value_idx))
                .map_err(|e| format!("{e} for group ({})", key.join(", ")))?;
            Ok((key, value))
        })
        .collect()
}

/// A key cell as text: whole numbers without a fraction, NULL as `null`.
fn key_text(cell: &CellValue) -> String {
    match cell {
        CellValue::Text(s) => s.clone(),
        CellValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{n:.0}"),
        CellValue::Number(n) => n.to_string(),
        CellValue::Null => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn text(s: &str) -> CellValue {
        CellValue::Text(s.to_string())
    }

    #[test]
    fn a_time_dimension_key_buckets_by_day_instead_of_grouping_raw_values() {
        let mut base = QueryRequest::new();
        base.measures = vec!["sales.net".to_string()];
        let members = cols(&["sales.restaurant_id", "sales.business_date"]);
        let (req, keys) = grouped_semantic_request(
            &base,
            "sales.business_date",
            &period_tz("America/Los_Angeles"),
            &members,
        );
        assert_eq!(req.dimensions, cols(&["sales.restaurant_id"]));
        assert_eq!(req.time_dimensions.len(), 1);
        assert_eq!(req.time_dimensions[0].dimension, "sales.business_date");
        assert_eq!(req.time_dimensions[0].granularity.as_deref(), Some("day"));
        assert_eq!(
            req.time_dimensions[0].date_range,
            Some(cols(&["2026-07-01", "2026-07-01"]))
        );
        // Bucketed in the window's calendar, not UTC.
        assert_eq!(req.timezone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(
            keys,
            cols(&["sales.restaurant_id", "sales.business_date.day"])
        );
    }

    #[test]
    fn grouped_request_without_a_time_key_leaves_the_window_unbucketed() {
        let mut base = QueryRequest::new();
        base.measures = vec!["sales.net".to_string()];
        let members = cols(&["sales.restaurant_id"]);
        let (req, keys) =
            grouped_semantic_request(&base, "sales.business_date", &period(), &members);
        assert_eq!(req.dimensions, members);
        assert_eq!(req.time_dimensions[0].granularity, None);
        assert_eq!(keys, members);
    }

    #[test]
    fn grouped_rows_reads_keys_by_name_in_group_by_order() {
        let rows = vec![
            QueryRow(vec![text("2026-05"), text("olo"), CellValue::Number(12.5)]),
            QueryRow(vec![
                text("2026-06"),
                CellValue::Null,
                CellValue::Number(3.0),
            ]),
        ];
        let out = grouped_rows(
            &cols(&["MONTH", "vendor", "unexplained"]),
            &rows,
            &cols(&["vendor", "month"]),
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                (cols(&["olo", "2026-05"]), 12.5),
                (cols(&["null", "2026-06"]), 3.0),
            ]
        );
    }

    #[test]
    fn grouped_rows_picks_the_value_column_by_name_when_several_remain() {
        let rows = vec![QueryRow(vec![
            CellValue::Number(7.0),
            text("note"),
            CellValue::Number(1.5),
        ])];
        let out = grouped_rows(
            &cols(&["store", "je_ids", "value"]),
            &rows,
            &cols(&["store"]),
        )
        .unwrap();
        assert_eq!(out, vec![(cols(&["7"]), 1.5)]);
    }

    #[test]
    fn grouped_rows_explains_a_bad_shape() {
        let rows: Vec<QueryRow> = vec![];
        let err = grouped_rows(&cols(&["a", "v"]), &rows, &cols(&["store"])).unwrap_err();
        assert!(err.contains("'store' is not a column"), "{err}");
        let err = grouped_rows(&cols(&["store", "x", "y"]), &rows, &cols(&["store"])).unwrap_err();
        assert!(err.contains("one value column"), "{err}");
        let null_value = vec![QueryRow(vec![text("a"), CellValue::Null])];
        let err = grouped_rows(&cols(&["store", "v"]), &null_value, &cols(&["store"])).unwrap_err();
        assert!(err.contains("NULL for group (a)"), "{err}");
    }

    fn period() -> ResolvedWindow {
        ResolvedWindow {
            dates: ["2026-07-01".to_string(), "2026-07-01".to_string()],
            timezone: "UTC".to_string(),
        }
    }

    fn period_tz(tz: &str) -> ResolvedWindow {
        ResolvedWindow {
            dates: ["2026-07-01".to_string(), "2026-07-01".to_string()],
            timezone: tz.to_string(),
        }
    }

    #[test]
    fn semantic_request_sets_a_non_utc_timezone() {
        let mut base = QueryRequest::new();
        base.measures = vec!["sales.net".to_string()];
        let req = semantic_request(
            &base,
            "sales.business_date",
            &period_tz("America/Los_Angeles"),
        );
        assert_eq!(req.timezone.as_deref(), Some("America/Los_Angeles"));
    }

    #[test]
    fn semantic_request_leaves_utc_unset() {
        // Some("UTC") and None are equivalent in the SQL generator, but None is
        // what every existing config produces — keep it byte-identical.
        let mut base = QueryRequest::new();
        base.measures = vec!["sales.net".to_string()];
        let req = semantic_request(&base, "sales.business_date", &period());
        assert_eq!(req.timezone, None);
    }

    #[test]
    fn render_sql_binds_the_timezone() {
        let out = render_sql(
            "select v where d at time zone '{{ timezone }}' between '{{ start_date }}' and '{{ end_date }}'",
            &period_tz("America/Los_Angeles"),
        )
        .unwrap();
        assert_eq!(
            out,
            "select v where d at time zone 'America/Los_Angeles' between '2026-07-01' and '2026-07-01'"
        );
    }

    #[test]
    fn semantic_request_overwrites_time_dimension_window() {
        let mut base = QueryRequest::new();
        base.measures = vec!["sales.net".to_string()];
        base.segments = vec!["sales.dine_in".to_string()];
        let req = semantic_request(&base, "sales.business_date", &period());
        // User params preserved.
        assert_eq!(req.measures, vec!["sales.net".to_string()]);
        assert_eq!(req.segments, vec!["sales.dine_in".to_string()]);
        // Window injected on the named dimension.
        assert_eq!(req.time_dimensions.len(), 1);
        assert_eq!(req.time_dimensions[0].dimension, "sales.business_date");
        assert_eq!(req.time_dimensions[0].granularity, None);
        assert_eq!(
            req.time_dimensions[0].date_range,
            Some(vec!["2026-07-01".to_string(), "2026-07-01".to_string()])
        );
    }

    #[test]
    fn render_sql_binds_bare_dates() {
        let out = render_sql(
            "select v where d between '{{ start_date }}' and '{{ end_date }}'",
            &period(),
        )
        .unwrap();
        assert_eq!(
            out,
            "select v where d between '2026-07-01' and '2026-07-01'"
        );
    }

    #[test]
    fn render_sql_reports_bad_template() {
        assert!(render_sql("{{ unclosed", &period()).is_err());
    }

    #[test]
    fn cell_to_f64_handles_number_text_null() {
        assert_eq!(cell_to_f64(&CellValue::Number(12.5)).unwrap(), 12.5);
        assert_eq!(
            cell_to_f64(&CellValue::Text(" 7 ".to_string())).unwrap(),
            7.0
        );
        assert!(cell_to_f64(&CellValue::Text("abc".to_string())).is_err());
        assert!(cell_to_f64(&CellValue::Null).is_err());
    }
}
