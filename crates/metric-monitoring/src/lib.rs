//! Time-series anomaly detection over the semantic-model metric tree.
//!
//! Three pieces:
//! - [`config`] — `.monitor.yml` parser; per-workspace list of monitored
//!   (measure, time-dim, granularity) tuples.
//! - [`detect`] — MSTL + AutoETS detector that flags anomalies in a tail
//!   window of a single time-series.
//! - [`forecast`] — the same model pointed forward: project a series past its
//!   last bucket, for the metric-tree scenario canvas. Shares [`gates`] with
//!   the detector so a projected expectation and a flagged one cannot
//!   disagree about what normal was.
//! - [`gates`] — statistical trust checks around the detector: how much
//!   history a series needs before it is scored at all, and which flags to
//!   suppress because the fit that produced them is not credible.
//! - [`metric_tree_baseline`], [`metric_tree_groups`], [`metric_tree_inputs`]
//!   — the scenario canvas's baseline read over the semantic-model metric
//!   tree: the shared query budget and the split-retry executor, per-view
//!   grouped values and the cross-view grouped fit, and the inputs a reached
//!   composite reads. Pure over airlayer; the HTTP handlers that call them
//!   (and build the executor from Oxy's connector pool) live in `oxy-app`,
//!   which re-exports these modules at their old `server::api::` paths.
//! - [`metric_tree_scenario`], [`metric_tree_projection`], [`metric_tree_series`]
//!   — the rest of the canvas's domain half: the routes' request and response
//!   shapes, the baseline reads and their notes, the unvalued diff; the
//!   projection's validation, seasonality resolution, per-view read and fit;
//!   and the bucketed time-series request with its timezone pad/trim. The
//!   handlers that call them stay in `oxy-app`, which re-exports the items at
//!   `server::api::metric_tree{,_projection}`.
//! - [`notify`] — the `notify:` block: which events a scan should announce,
//!   the once-per-event claim on them, and the Slack message. Posting it is
//!   the host's.
//! - [`preview`] — a dry run of one monitor: what the next scan would flag,
//!   with nothing written.
//! - [`service`] — orchestrator that loads the config, fetches each
//!   series via a [`MetricTreeRunner`], runs the detector, and returns
//!   a flat list of [`detect::DetectedAnomaly`] paired with the monitor
//!   that produced them. Persistence lives in the host (`oxy-app`), so the
//!   service doesn't take a database handle.
//!
//! [`MetricTreeRunner`]: agentic_analytics::MetricTreeRunner

pub mod config;
pub mod detect;
pub mod forecast;
pub mod gates;
pub mod metric_tree_baseline;
pub mod metric_tree_groups;
pub mod metric_tree_inputs;
pub mod metric_tree_projection;
pub mod metric_tree_scenario;
pub mod metric_tree_series;
pub mod notify;
pub mod persist;
pub mod preview;
pub mod service;
pub mod store;
pub mod tick;

pub use config::{
    Direction, Granularity, LoadError as ConfigLoadError, MonitorConfig, MonitorEntry,
    MonitorScheduleConfig, Sensitivity, WeekStart, default_config_path, from_definition,
    load_from_file,
};
pub use detect::{
    Continuation, DetectError, DetectInputs, DetectedAnomaly, Observation, Severity, detect,
};
pub use forecast::{DEFAULT_INTERVAL_LEVEL, ProjectError, ProjectInputs, ProjectedBucket, project};
pub use gates::min_history_buckets;
pub use notify::NotifyConfig;
pub use persist::{load_open_events, persist_scan, upsert_anomalies, upsert_coverage};
pub use service::{
    Coverage, MonitorOutcome, OpenEvents, ScanError, ScanResult, SegmentKey, SegmentScan,
    scan_workspace,
};
pub use tick::{LastScanRegistry, TickError, TickOutcome, global_registry, tick_workspace};
