use super::*;
use crate::config::{Direction, Sensitivity};
use agentic_analytics::MetricTreeRunnerError;
use chrono::{NaiveDate, TimeZone};
use std::sync::atomic::{AtomicUsize, Ordering};

type Rows = Vec<(String, f64)>;
type SeriesFor = Box<dyn Fn(Option<&str>) -> Result<Rows, String> + Send + Sync>;

/// A warehouse that answers the two questions a preview asks: which values a
/// dimension has, and one segment's series. Everything else errors, so a new
/// call site in the preview is loud.
struct Warehouse {
    values: Result<Vec<String>, String>,
    series: SeriesFor,
    series_calls: AtomicUsize,
}

impl Warehouse {
    fn with(series: impl Fn(Option<&str>) -> Result<Rows, String> + Send + Sync + 'static) -> Self {
        Warehouse {
            values: Ok(vec![]),
            series: Box::new(series),
            series_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl MetricTreeRunner for Warehouse {
    async fn load_layer(
        &self,
    ) -> Result<oxy_airlayer_compat::SemanticLayer, MetricTreeRunnerError> {
        Err(MetricTreeRunnerError::LayerLoad(
            "unused by a preview".into(),
        ))
    }
    async fn list_databases(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn run_explain(
        &self,
        _: String,
        _: String,
        _: (String, String),
        _: (String, String),
        _: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
        _: oxy_airlayer_compat::engine::metric_tree_ops::ExplainConfig,
    ) -> Result<oxy_airlayer_compat::engine::metric_tree_ops::ExplainResult, MetricTreeRunnerError>
    {
        Err(MetricTreeRunnerError::Op("unused by a preview".into()))
    }
    async fn run_opportunity(
        &self,
        _: String,
        _: String,
        _: (String, String),
        _: oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic,
    ) -> Result<
        oxy_airlayer_compat::engine::metric_tree_ops::OpportunityResult,
        MetricTreeRunnerError,
    > {
        Err(MetricTreeRunnerError::Op("unused by a preview".into()))
    }
    async fn get_dimension_values(
        &self,
        _: String,
        _: String,
        _: u32,
    ) -> Result<Vec<String>, MetricTreeRunnerError> {
        self.values.clone().map_err(MetricTreeRunnerError::Op)
    }
    async fn run_time_series(
        &self,
        _measure: String,
        _time_dimension: String,
        _granularity: String,
        _period: (String, String),
        filters: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
        _timezone: Option<String>,
    ) -> Result<Rows, MetricTreeRunnerError> {
        self.series_calls.fetch_add(1, Ordering::Relaxed);
        let segment = filters
            .last()
            .and_then(|f| f.values.first())
            .map(String::as_str);
        (self.series)(segment).map_err(MetricTreeRunnerError::Op)
    }
}

/// Noon UTC on a Monday; the newest complete daily bucket is the day before.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap()
}

const YESTERDAY: &str = "2026-07-26";

/// `days` of a steady weekly rhythm ending yesterday, with `bump` added to
/// yesterday's value.
fn daily(days: i64, bump: f64) -> Rows {
    let last = NaiveDate::from_ymd_opt(2026, 7, 26).unwrap();
    (0..days)
        .map(|i| {
            let date = last - chrono::Duration::days(days - 1 - i);
            let phase = (i % 7) as f64 / 7.0;
            let wiggle = ((i * 1103515245 + 12345) % 1000) as f64 / 10_000.0;
            let value = 100.0 + 5.0 * (2.0 * std::f64::consts::PI * phase).sin() + wiggle;
            (date.format("%Y-%m-%d").to_string(), value)
        })
        .map(|(date, value)| {
            let value = if date == YESTERDAY {
                value + bump
            } else {
                value
            };
            (date, value)
        })
        .collect()
}

fn entry() -> MonitorEntry {
    MonitorEntry {
        measure: "sales.net".into(),
        time_dimension: "sales.day".into(),
        granularity: Granularity::Day,
        lookback_days: 90,
        seasonality: None,
        sensitivity: Sensitivity::Medium,
        label: None,
        filters: vec![],
        group_by: None,
        direction: Direction::Both,
        timezone: None,
        freshness: None,
        week_start: None,
    }
}

async fn preview(warehouse: Warehouse, entry: &MonitorEntry) -> Result<MonitorPreview, ScanError> {
    preview_monitor(Arc::new(warehouse), entry, now(), &OpenEvents::new()).await
}

#[tokio::test]
async fn a_spike_yesterday_is_what_the_next_scan_would_flag() {
    let out = preview(Warehouse::with(|_| Ok(daily(90, 400.0))), &entry())
        .await
        .unwrap();

    assert_eq!(out.window_buckets, 7, "a daily scan scores the last week");
    assert_eq!(out.segments_total, 1);
    assert_eq!(out.segments.len(), 1);
    assert_eq!(out.segments[0].dimension_key, "");
    let SegmentOutcome::Scored {
        flagged,
        measured_buckets,
        required_buckets,
    } = &out.segments[0].outcome
    else {
        panic!("a full history is scored: {:?}", out.segments[0].outcome);
    };
    assert!(measured_buckets >= required_buckets);
    let days: Vec<String> = flagged
        .iter()
        .map(|a| a.timestamp.format("%Y-%m-%d").to_string())
        .collect();
    assert_eq!(days, [YESTERDAY], "only the bucket that moved is flagged");
    assert!(flagged[0].observed > flagged[0].upper);
}

/// The control for the test above: the same series without the bump. If this
/// flagged too, "a spike is flagged" would be saying nothing.
#[tokio::test]
async fn a_quiet_series_is_scored_and_flags_nothing() {
    let out = preview(Warehouse::with(|_| Ok(daily(90, 0.0))), &entry())
        .await
        .unwrap();

    assert!(
        matches!(&out.segments[0].outcome, SegmentOutcome::Scored { flagged, .. } if flagged.is_empty()),
        "{:?}",
        out.segments[0].outcome
    );
}

/// Too little history is its own answer. Reported as "scored, nothing
/// flagged" it would tell an author their new monitor is healthy when it is
/// not being scored at all.
#[tokio::test]
async fn too_little_history_is_warming_up_not_nothing_found() {
    let out = preview(Warehouse::with(|_| Ok(daily(20, 400.0))), &entry())
        .await
        .unwrap();

    let SegmentOutcome::WarmingUp {
        measured_buckets,
        required_buckets,
    } = out.segments[0].outcome
    else {
        panic!(
            "twenty days cannot be scored: {:?}",
            out.segments[0].outcome
        );
    };
    assert_eq!(measured_buckets, 20);
    assert!(required_buckets > 20);
}

#[tokio::test]
async fn a_segment_that_errors_is_reported_as_that_segment() {
    let out = preview(
        Warehouse::with(|_| Err("relation \"sales_daily\" does not exist".into())),
        &entry(),
    )
    .await
    .expect("one failing segment is an outcome, not a failed preview");

    let SegmentOutcome::Failed { error } = &out.segments[0].outcome else {
        panic!("{:?}", out.segments[0].outcome);
    };
    assert!(error.contains("sales_daily"), "{error}");
}

/// A `group_by` monitor is previewed on its first dozen segments, in the order
/// the dimension listed them, each under the key a scan would file it under —
/// and the response still says how many there are.
#[tokio::test]
async fn a_fanned_out_monitor_previews_its_first_segments_and_counts_them_all() {
    let stores: Vec<String> = (1..=15).map(|n| format!("loc-{n:02}")).collect();
    let warehouse = Warehouse {
        values: Ok(stores.clone()),
        series: Box::new(|segment| match segment {
            Some("loc-02") => Err("warehouse timed out".into()),
            Some("loc-03") => Ok(daily(90, 400.0)),
            _ => Ok(daily(90, 0.0)),
        }),
        series_calls: AtomicUsize::new(0),
    };
    let warehouse = Arc::new(warehouse);
    let fanned = MonitorEntry {
        filters: vec![MonitorFilter {
            member: "sales.region".into(),
            values: vec!["US".into()],
        }],
        group_by: Some("sales.store".into()),
        ..entry()
    };

    let out = preview_monitor(warehouse.clone(), &fanned, now(), &OpenEvents::new())
        .await
        .unwrap();

    assert_eq!(out.segments_total, 15);
    assert_eq!(out.segments.len(), MAX_PREVIEW_SEGMENTS);
    assert_eq!(
        warehouse.series_calls.load(Ordering::Relaxed),
        MAX_PREVIEW_SEGMENTS
    );
    let keys: Vec<&str> = out
        .segments
        .iter()
        .map(|s| s.dimension_key.as_str())
        .collect();
    let expected: Vec<String> = stores[..MAX_PREVIEW_SEGMENTS]
        .iter()
        .map(|s| format!("sales.region=US;sales.store={s}"))
        .collect();
    assert_eq!(keys, expected);
    assert!(
        matches!(&out.segments[1].outcome, SegmentOutcome::Failed { error } if error.contains("timed out"))
    );
    assert!(
        matches!(&out.segments[2].outcome, SegmentOutcome::Scored { flagged, .. } if flagged.len() == 1)
    );
    assert!(
        matches!(&out.segments[0].outcome, SegmentOutcome::Scored { flagged, .. } if flagged.is_empty())
    );
}

/// With no values there are no segments to report on, and "zero segments, all
/// fine" would be a lie about a dimension that could not be read.
#[tokio::test]
async fn a_dimension_that_cannot_be_listed_fails_the_preview() {
    let warehouse = Warehouse {
        values: Err("unknown dimension sales.stor".into()),
        ..Warehouse::with(|_| Ok(daily(90, 0.0)))
    };
    let fanned = MonitorEntry {
        group_by: Some("sales.stor".into()),
        ..entry()
    };

    let err = preview(warehouse, &fanned)
        .await
        .expect_err("no segments to preview");
    assert!(err.to_string().contains("sales.stor"), "{err}");
}

#[test]
fn a_selector_tells_apart_entries_that_share_a_measure() {
    let us = MonitorEntry {
        filters: vec![MonitorFilter {
            member: "sales.region".into(),
            values: vec!["US".into()],
        }],
        ..entry()
    };
    let weekly = MonitorEntry {
        granularity: Granularity::Week,
        ..entry()
    };
    // The common pair: a total, and the same measure split per store. They
    // share the triple and have no filters; only `group_by` differs.
    let per_store = MonitorEntry {
        group_by: Some("sales.store".into()),
        ..entry()
    };
    let config = MonitorConfig {
        monitors: vec![entry(), us, weekly, per_store],
        ..Default::default()
    };
    let select = |granularity, dimension_key: &str, group_by: Option<&str>| MonitorSelector {
        measure: "sales.net".into(),
        time_dimension: "sales.day".into(),
        granularity,
        dimension_key: dimension_key.into(),
        group_by: group_by.map(String::from),
    };
    let found = |s: MonitorSelector| {
        s.find(&config)
            .map(|m| (m.granularity, m.filters.len(), m.group_by.clone()))
    };

    assert_eq!(
        found(select(Granularity::Day, "", None)),
        Some((Granularity::Day, 0, None))
    );
    assert_eq!(
        found(select(Granularity::Day, "sales.region=US", None)),
        Some((Granularity::Day, 1, None))
    );
    assert_eq!(
        found(select(Granularity::Week, "", None)),
        Some((Granularity::Week, 0, None))
    );
    assert_eq!(
        found(select(Granularity::Day, "", Some("sales.store"))),
        Some((Granularity::Day, 0, Some("sales.store".to_string()))),
        "the split is not the total"
    );
    assert_eq!(found(select(Granularity::Month, "", None)), None);
    assert_eq!(
        found(select(Granularity::Day, "sales.region=EU", None)),
        None
    );
    assert_eq!(
        found(select(Granularity::Day, "", Some("sales.region"))),
        None
    );
}

/// A selector sent by a client from before `group_by` was part of it still
/// names an entry that has none.
#[test]
fn a_selector_without_group_by_names_the_ungrouped_entry() {
    let selector: MonitorSelector = serde_json::from_value(serde_json::json!({
        "measure": "sales.net", "time_dimension": "sales.day", "granularity": "day",
    }))
    .unwrap();
    assert_eq!(selector.group_by, None);
    assert_eq!(selector.dimension_key, "");
}

/// The wire shape the Monitors tab reads: one flat object per segment, the
/// state as a word.
#[test]
fn a_segment_serialises_flat_with_its_state_as_a_word() {
    let warming = SegmentPreview {
        dimension_key: "sales.store=loc-01".into(),
        outcome: SegmentOutcome::WarmingUp {
            measured_buckets: 9,
            required_buckets: 26,
        },
    };
    assert_eq!(
        serde_json::to_value(&warming).unwrap(),
        serde_json::json!({
            "dimension_key": "sales.store=loc-01",
            "state": "warming_up",
            "measured_buckets": 9,
            "required_buckets": 26,
        })
    );
    let failed = SegmentPreview {
        dimension_key: String::new(),
        outcome: SegmentOutcome::Failed {
            error: "boom".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&failed).unwrap(),
        serde_json::json!({ "dimension_key": "", "state": "failed", "error": "boom" })
    );
}
