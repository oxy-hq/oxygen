//! The bucketed time-series request a metric-tree read sends airlayer, and the
//! timezone pad/trim around it: one decision shared by the anomaly scan's
//! `run_time_series` and the scenario projection's batched query, so the two
//! cannot drift apart on the workaround. Lowered from `oxy-app`'s
//! `agentic_wiring::metric_tree_runner`, which runs the request it builds.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime};
use oxy_airlayer_compat::engine::query::QueryRequest;

/// Parse a `run_time_series` bucket label or period boundary into a calendar
/// date. Labels come back as `YYYY-MM-DD` (daily granularity) or
/// `YYYY-MM-DD HH:MM:SS` (sub-daily); period boundaries supplied by callers
/// are usually plain dates but may be full ISO-8601/RFC3339 timestamps (the
/// `detect_anomalies` tool passes through whatever the LLM sent). Try each
/// representation in turn; `None` means the string matched none of them.
pub fn parse_flexible_date(s: &str) -> Option<NaiveDate> {
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.date());
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.date_naive())
}

/// Whether `run_time_series` needs to pad-and-trim for this request, and if
/// so, the caller's original window (parsed) to trim back down to.
///
/// Returns `None` for a UTC-equivalent timezone (`None` or exactly `"UTC"`) —
/// no conversion happens, so no padding or trimming is needed and the
/// pre-existing UTC path stays byte-for-byte identical. Also returns `None`
/// when either boundary of `period` fails to parse as a date: in that case
/// widening would produce a garbled request, so the request is sent as-is and
/// nothing is trimmed rather than risk corrupting or wrongly filtering it.
fn non_utc_trim_window(
    timezone: Option<&str>,
    period: &(String, String),
) -> Option<(NaiveDate, NaiveDate)> {
    // Exact-match "UTC", mirroring `preagg_for`'s convention — timezone names
    // come from `.monitor.yml`, validated at load time as parseable
    // `chrono_tz::Tz` names, so this never sees case variants in practice.
    match timezone {
        None => return None,
        Some("UTC") => return None,
        Some(_) => {}
    }
    let (Some(start), Some(end)) = (
        parse_flexible_date(&period.0),
        parse_flexible_date(&period.1),
    ) else {
        // Widening would produce a garbled request, so the request is sent
        // as-is and nothing is trimmed rather than risk corrupting or wrongly
        // filtering it. Logged because this silently reinstates the original
        // clipping bug for this one call — worth knowing about if anomaly
        // detection on a non-UTC workspace starts looking off again.
        tracing::debug!(
            target: "metric_monitoring",
            period = ?period,
            "unparseable period boundary; skipping the timezone pad/trim"
        );
        return None;
    };
    Some((start, end))
}

/// Widen a parsed `[start, end]` window for the actual airlayer request, as
/// plain `YYYY-MM-DD` strings. The pad is **asymmetric**.
///
/// As of the airlayer pin bumped in this change, `date_range` converts the
/// column to `request.timezone` before comparing, so the filter and the bucket
/// labels finally agree and this pad is a harmless **over-fetch** that
/// [`trim_to_window`] discards. It is kept rather than deleted because it is
/// the one thing standing between a future airlayer regression on that seam
/// and silently clipped edge buckets; the sizing below is the reasoning for
/// why one leading and two trailing days is the right amount of insurance.
///
/// Before that fix, `date_range` compared the RAW (unconverted) column
/// against a plain date string, which the SQL engine reads as that date's
/// midnight UTC — no time-of-day, no timezone shift. But the bucket labelled
/// `end` covers *local* wall-clock time `[end 00:00, end+1 00:00)`, and
/// converted to UTC that interval is offset by the zone's UTC delta `O`
/// (`local = UTC + O`): it spans UTC instants `[end 00:00 − O, end+1 00:00 − O)`.
///
/// For a **west-of-UTC** zone (`O` negative — every Americas zone), that
/// shifts the interval *later* in UTC, so its upper end can land after
/// `end+1 00:00 UTC`: at the `UTC-12` extreme, up to `end+1 12:00 UTC`. A
/// single trailing day of pad (`end+1`) is short by up to 12 hours — the
/// review finding that set this sizing (an `America/Los_Angeles` request was
/// missing the last 7 hours of its final bucket, the only bucket
/// `detect_and_upsert`'s `test_window: 1` evaluates). Two trailing days
/// covers every real IANA offset down to `UTC-12`.
///
/// The leading side stays a single day: the worst case there is the opposite
/// extreme, `UTC+14`, whose earliest instant (`start 00:00 − 14h`) is only 14
/// hours before `start 00:00 UTC` — well inside one day of pad. (In general,
/// one leading day covers any `O <= 24`, which every real zone satisfies.)
fn widened_date_range(start: NaiveDate, end: NaiveDate) -> (String, String) {
    (
        (start - Duration::days(1)).format("%Y-%m-%d").to_string(),
        (end + Duration::days(2)).format("%Y-%m-%d").to_string(),
    )
}

/// Trim `rows` back down to the caller's original `[start, end]` window,
/// inclusive on both ends. A row whose label fails to parse is kept rather
/// than dropped — a parse failure is a new, unrelated failure mode and must
/// not silently look like "trimmed as padding."
pub fn trim_to_window(
    rows: Vec<(String, f64)>,
    start: NaiveDate,
    end: NaiveDate,
) -> Vec<(String, f64)> {
    rows.into_iter()
        .filter(|(ts, _)| match parse_flexible_date(ts) {
            Some(d) => d >= start && d <= end,
            None => true,
        })
        .collect()
}

/// Resolve what `run_time_series` should send airlayer as its `date_range`,
/// and the window (if any) to trim the response rows back down to
/// afterward. Composes [`non_utc_trim_window`] and [`widened_date_range`]
/// into the one decision [`build_time_series_query_request`] needs.
fn time_series_request(
    timezone: Option<&str>,
    period: &(String, String),
) -> ((String, String), Option<(NaiveDate, NaiveDate)>) {
    let trim_window = non_utc_trim_window(timezone, period);
    let request_period = match &trim_window {
        Some((start, end)) => widened_date_range(*start, *end),
        None => period.clone(),
    };
    (request_period, trim_window)
}

/// Build the `QueryRequest` `run_time_series` sends to airlayer, and the
/// window (if any) to trim the response rows back down to afterward.
///
/// This is the **entire** seam between the timezone pad/trim decision and
/// the request `run_time_series` actually issues: the `date_range` embedded
/// here, via [`time_series_request`], is the only place that value is
/// computed. A regression that reverts to sending `period` unwidened (or
/// drops the trim window) has to change this function — and does change what
/// [`build_time_series_query_request_widens_the_date_range_for_non_utc`] and
/// its UTC/parse-failure counterparts below assert.
///
/// Takes a **slice** of measures rather than one: the scenario projection
/// batches every forward-reachable node into a single bucketed query, and it
/// needs this same pad/trim decision. One measure is that slice with one
/// element, so the two callers cannot drift apart on the workaround.
pub fn build_time_series_query_request(
    measures: &[String],
    time_dimension: &str,
    granularity: &str,
    filters: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
    period: &(String, String),
    timezone: Option<String>,
) -> (QueryRequest, Option<(NaiveDate, NaiveDate)>) {
    use oxy_airlayer_compat::engine::query::{OrderBy, TimeDimensionQuery};
    let (request_period, trim_window) = time_series_request(timezone.as_deref(), period);
    let dim_alias = format!("{time_dimension}.{granularity}");
    let request = QueryRequest {
        measures: measures.to_vec(),
        filters,
        time_dimensions: vec![TimeDimensionQuery {
            dimension: time_dimension.to_string(),
            granularity: Some(granularity.to_string()),
            date_range: Some(vec![request_period.0, request_period.1]),
        }],
        order: vec![OrderBy {
            id: dim_alias,
            desc: false,
        }],
        timezone,
        ..QueryRequest::new()
    };
    (request, trim_window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_airlayer_compat::SemanticLayer;

    #[test]
    fn non_utc_trim_window_is_none_for_utc_equivalent_timezones() {
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());
        assert_eq!(
            non_utc_trim_window(None, &period),
            None,
            "None means UTC default"
        );
        assert_eq!(
            non_utc_trim_window(Some("UTC"), &period),
            None,
            "explicit UTC must not pad/trim"
        );
    }

    #[test]
    fn non_utc_trim_window_is_some_for_a_real_timezone() {
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());
        assert_eq!(
            non_utc_trim_window(Some("America/Los_Angeles"), &period),
            Some((
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
            ))
        );
    }

    #[test]
    fn non_utc_trim_window_is_none_when_a_boundary_fails_to_parse() {
        // An unparseable boundary must not be silently widened/trimmed —
        // fall back to sending the request as-is.
        let period = ("not a date".to_string(), "2026-07-24".to_string());
        assert_eq!(
            non_utc_trim_window(Some("America/Los_Angeles"), &period),
            None
        );
    }

    #[test]
    fn widened_date_range_pads_one_leading_day_and_two_trailing_days() {
        // The trailing pad must be 2 days, not 1: a west-of-UTC zone's `end`
        // bucket can run up to 12h past `end+1 00:00 UTC` (UTC-12 extreme),
        // so a single trailing day is short. This is the exact shape of the
        // review finding: an America/Los_Angeles request's last bucket was
        // clipped because the old pad only reached `end+1`.
        let (start, end) = widened_date_range(
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
        );
        assert_eq!(start, "2026-07-19", "leading pad stays one day");
        assert_eq!(end, "2026-07-26", "trailing pad must be two days");
    }

    #[test]
    fn time_series_request_widens_for_non_utc_and_passes_through_for_utc() {
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());

        let (request_period, trim_window) =
            time_series_request(Some("America/Los_Angeles"), &period);
        assert_eq!(
            request_period,
            ("2026-07-19".to_string(), "2026-07-26".to_string()),
            "non-UTC must widen -1 day / +2 days"
        );
        assert_eq!(
            trim_window,
            Some((
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
            ))
        );

        for tz in [None, Some("UTC")] {
            let (request_period, trim_window) = time_series_request(tz, &period);
            assert_eq!(
                request_period, period,
                "UTC-equivalent ({tz:?}) must not widen the range"
            );
            assert_eq!(
                trim_window, None,
                "UTC-equivalent ({tz:?}) must not trim anything"
            );
        }
    }

    /// One ClickHouse view with both flavours of time dimension: an instant
    /// (`created_at`) and a business date (`business_date`), the latter shaped
    /// like the workspace whose projection panel this fix came from — a column
    /// normalised to a DATE by the dimension's own expression.
    fn ts_layer() -> SemanticLayer {
        let orders = r#"
name: orders
table: analytics.orders
dialect: clickhouse
dimensions:
  - name: created_at
    type: datetime
    expr: created_at
  - name: business_date
    type: date
    expr: "toDate(parseDateTimeBestEffort(toString(business_date)))"
measures:
  - name: revenue
    type: sum
    expr: revenue
"#;
        SemanticLayer::new(
            vec![oxy_airlayer_compat::parse_view_yaml(orders).unwrap()],
            None,
        )
    }

    #[test]
    fn build_time_series_query_request_widens_the_date_range_for_non_utc() {
        // The seam a wiring regression would have to break: the `date_range`
        // actually embedded in the `QueryRequest` `run_time_series` sends to
        // airlayer. Reverting to `Some(vec![period.0, period.1])` here — the
        // exact regression the review finding warned about — fails this
        // assertion.
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());
        let (request, trim_window) = build_time_series_query_request(
            &["orders.revenue".to_string()],
            "orders.created_at",
            "day",
            vec![],
            &period,
            Some("America/Los_Angeles".to_string()),
        );
        assert_eq!(
            request.time_dimensions[0].date_range,
            Some(vec!["2026-07-19".to_string(), "2026-07-26".to_string()]),
            "the request's date_range must be the widened -1/+2 day range"
        );
        assert_eq!(request.timezone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(
            trim_window,
            Some((
                NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
            ))
        );
    }

    #[test]
    fn build_time_series_query_request_is_unwidened_for_utc() {
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());
        for tz in [None, Some("UTC".to_string())] {
            let (request, trim_window) = build_time_series_query_request(
                &["orders.revenue".to_string()],
                "orders.created_at",
                "day",
                vec![],
                &period,
                tz.clone(),
            );
            assert_eq!(
                request.time_dimensions[0].date_range,
                Some(vec![period.0.clone(), period.1.clone()]),
                "UTC-equivalent ({tz:?}) must send the caller's range unwidened"
            );
            assert_eq!(trim_window, None, "UTC-equivalent ({tz:?}) must not trim");
        }
    }

    /// The reported failure, pinned from the side that reported it: airlayer
    /// compiled
    /// `toTimeZone(toDate(parseDateTimeBestEffort(…)), 'America/Los_Angeles')`
    /// and ClickHouse answered `Illegal type Date of argument of function
    /// toTimezone`, taking down every curve in a projection panel.
    ///
    /// The gate that prevents it is airlayer's (`time_col_expr` converts only
    /// a `DimensionType::Datetime`), and airlayer tests it — so this is not
    /// that test again. It is a **pin canary**: the gate was lost once
    /// already, not by anyone editing it but by a pin bump onto a branch cut
    /// before it merged, and a subtraction is what a dependency's own test
    /// suite cannot report to us. Asserted on compiled SQL for the same
    /// reason: it is the only artefact that shows what the warehouse will
    /// actually be asked.
    #[test]
    fn a_date_typed_dimension_compiles_to_clickhouse_without_a_timezone_conversion() {
        let layer = ts_layer();
        let period = ("2026-07-20".to_string(), "2026-07-24".to_string());
        let dialects = oxy_airlayer_compat::DatasourceDialectMap::with_default(
            oxy_airlayer_compat::Dialect::ClickHouse,
        );
        let engine =
            oxy_airlayer_compat::SemanticEngine::from_semantic_layer(layer.clone(), dialects)
                .expect("engine");

        let compile = |dim: &str| {
            let (request, _) = build_time_series_query_request(
                &["orders.revenue".to_string()],
                dim,
                "day",
                vec![],
                &period,
                Some("America/Los_Angeles".to_string()),
            );
            engine.compile_query(&request).expect("compile").sql
        };

        let date_sql = compile("orders.business_date");
        assert!(
            !date_sql.contains("toTimeZone"),
            "a DATE argument to toTimeZone is a ClickHouse type error: {date_sql}"
        );
        // The other half of the claim: this is a narrowing, not a removal. A
        // datetime dimension is an instant and still converts — a "fix" that
        // dropped every conversion would pass the assertion above.
        assert!(
            compile("orders.created_at").contains("toTimeZone"),
            "a type: datetime dimension must still be converted"
        );
    }

    #[test]
    fn trim_to_window_drops_padded_edges_and_keeps_in_window_rows() {
        let rows = vec![
            ("2026-07-19".to_string(), 1.0), // padded lead
            ("2026-07-20".to_string(), 2.0), // window start
            ("2026-07-22".to_string(), 3.0), // interior
            ("2026-07-24".to_string(), 4.0), // window end (inclusive)
            ("2026-07-25".to_string(), 5.0), // padded trail
        ];
        let kept = trim_to_window(
            rows,
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
        );
        assert_eq!(
            kept,
            vec![
                ("2026-07-20".to_string(), 2.0),
                ("2026-07-22".to_string(), 3.0),
                ("2026-07-24".to_string(), 4.0),
            ]
        );
    }

    #[test]
    fn trim_to_window_keeps_sub_daily_labels_at_the_inclusive_boundary() {
        // A naive lexicographic `<=` against "2026-07-24" would wrongly drop
        // "2026-07-24 00:00:00" (sub-daily granularity label); the parsed-date
        // comparison must not.
        let rows = vec![
            ("2026-07-24 00:00:00".to_string(), 1.0),
            ("2026-07-25 00:00:00".to_string(), 2.0), // padded trail, out of window
        ];
        let kept = trim_to_window(
            rows,
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
        );
        assert_eq!(kept, vec![("2026-07-24 00:00:00".to_string(), 1.0)]);
    }

    #[test]
    fn trim_to_window_keeps_unparseable_labels_rather_than_dropping() {
        let rows = vec![("garbage".to_string(), 42.0)];
        let kept = trim_to_window(
            rows.clone(),
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
        );
        assert_eq!(kept, rows, "an unparseable label must be kept, not dropped");
    }
}
