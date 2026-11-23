//! Sentry classification pins for the background drivers, which went up from
//! `oxy-app` (`server::router::{recovery, periodic, drive_policy,
//! run_retention, fleet_jobs}` and `server::{audit_anchor, token_usage_flush}`)
//! into `oxy-server`'s `drivers` in decompose BL1 (2026-11-23), with the dead
//! billing reconciler out of `server::router::entry`. Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size budget,
//! and these tests read only its public API.
//!
//! The new paths read `oxy::drivers::…` in the binary and `oxy_server::drivers::…`
//! since decompose LT gave the crate a library (`sentry_filter_lt_pins_tests`
//! pins the two alike). None may read as the data plane (`oxy::connector`) or
//! as custom-app code (`oxy::app_function`); the match is segment-aware, and
//! `drivers` is neither.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The old and new module path of every module BL1 moved. Most of the
/// drivers' lines name `target: "recovery"` or `"background"`, which the move
/// does not touch; the rest log under the module path pinned here.
fn moved_pairs() -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for module in [
        "recovery",
        "periodic",
        "periodic::claim",
        "periodic::metrics",
        "periodic::monitors",
        "periodic::reconcile",
        "drive_policy",
        "run_retention",
        "fleet_jobs",
    ] {
        pairs.push((
            format!("oxy_app::server::router::{module}"),
            format!("oxy_server::drivers::{module}"),
        ));
    }
    for module in ["audit_anchor", "token_usage_flush"] {
        pairs.push((
            format!("oxy_app::server::{module}"),
            format!("oxy_server::drivers::{module}"),
        ));
    }
    pairs.push((
        "oxy_app::server::router::entry".to_string(),
        "oxy_server::drivers::billing_reconciler".to_string(),
    ));
    pairs
}

/// Platform code in `oxy-app`: kept (an `error` is an event, nothing is
/// dropped) and not the data plane, at the old path and the new one alike.
#[test]
fn the_bl1_drivers_keep_their_sentry_lines_after_moving_up() {
    for (old, new) in &moved_pairs() {
        for module in [old.as_str(), new.as_str()] {
            assert_eq!(
                sentry_disposition(module, &Level::ERROR),
                Disposition::Event,
                "{module}"
            );
            assert!(!drop_log(Some(module)), "{module}");
            assert!(!is_data_plane_module(module), "{module}");
        }
    }
}

/// Every level classifies the same at the new path as at the old one.
#[test]
fn the_bl1_drivers_classify_alike_at_every_level() {
    for (old, new) in &moved_pairs() {
        for level in [
            Level::ERROR,
            Level::WARN,
            Level::INFO,
            Level::DEBUG,
            Level::TRACE,
        ] {
            assert_eq!(
                sentry_disposition(old, &level),
                sentry_disposition(new, &level),
                "{old} -> {new} at {level}"
            );
        }
    }
}
