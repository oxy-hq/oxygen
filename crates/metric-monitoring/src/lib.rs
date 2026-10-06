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
//! - [`notify`] — the `notify:` block: which events a scan should announce,
//!   the once-per-event claim on them, and the Slack message. Posting it is
//!   the host's.
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
pub mod notify;
pub mod persist;
pub mod service;
pub mod store;
pub mod tick;

pub use config::{
    Direction, Granularity, LoadError as ConfigLoadError, MonitorConfig, MonitorEntry,
    MonitorScheduleConfig, Sensitivity, WeekStart, default_config_path, load_from_file,
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
