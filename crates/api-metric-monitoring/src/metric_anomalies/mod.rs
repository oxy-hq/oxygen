//! Anomaly Inbox API — list / scan / acknowledge / dismiss.
//!
//! The detection algorithm + `.monitor.yml` parsing live in
//! `oxy-metric-monitoring`. This module is the HTTP surface plus the
//! `MonitorOutcome` → `metric_anomalies` row upsert. Scans run inline on
//! the request thread for now; the eventual scheduler tick will reuse the
//! same `run_scan` helper from a background task.
//!
//! Split by what each half answers, since the reading order differs: `list` is
//! the paged read (event ranking, counts), `cap` the per-event bucket trim,
//! `status` the acknowledge/dismiss writes, `scan` the detector trigger,
//! `preview` its dry run of one monitor, and `explain` the cached
//! decomposition. `error` is shared by all of them, and
//! every handler is re-exported here so the router's paths are unchanged.
//!
//! (Named in prose rather than linked: the modules are private, and an
//! intra-doc link to a private item is a rustdoc warning.)

mod cap;
mod error;
mod explain;
mod list;
mod preview;
mod scan;
mod status;

pub use error::AnomalyError;
pub use explain::explain_anomaly;
pub use list::{list_anomalies, list_monitors};
pub use preview::preview_monitor;
pub use scan::run_scan;
pub use status::{apply_status_bulk, apply_status_bulk_capped, update_status, update_status_bulk};
