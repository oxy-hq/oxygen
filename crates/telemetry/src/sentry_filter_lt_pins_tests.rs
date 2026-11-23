//! Sentry classification pins for decompose LT (2026-11-23), which gave
//! `oxy-server` a library target behind a thin `main.rs`. Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.
//!
//! The binary's crate ident is `oxy`, the same as the core library's, so its
//! modules logged as `oxy::…`. In the library they log as `oxy_server::…`, which
//! ends that collision. The lines `main.rs` keeps (the role hint, the telemetry
//! bring-up, "Application error") still log as `oxy`. Every module path the
//! crate had at the cut is listed here, old and new, and each must classify
//! the same at both.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// Every non-test module of `crates/server/src` at the cut, relative to the
/// crate root.
const MODULES: &[&str] = &[
    "admission",
    "app_platform",
    "commands",
    "commands::args",
    "commands::serve",
    "commands::serve_background",
    "commands::start",
    "commands::worker",
    "core_routes",
    "core_routes::catalog",
    "core_routes::healthcheck",
    "custom_apps_seams",
    "dev",
    "drivers",
    "drivers::audit_anchor",
    "drivers::billing_reconciler",
    "drivers::drive_policy",
    "drivers::fleet_jobs",
    "drivers::periodic",
    "drivers::periodic::claim",
    "drivers::periodic::metrics",
    "drivers::periodic::monitors",
    "drivers::periodic::reconcile",
    "drivers::recovery",
    "drivers::run_retention",
    "drivers::token_usage_flush",
    "logging",
    "metrics_server",
    "observability_boot",
    "observability_boot::retry",
    "shared_state_check",
    "static_assets",
    "static_assets::precompressed",
    "worker_health",
    "worker_metrics",
    "worker_runtime",
];

/// The one module whose path carries the `custom_apps` marker. Its lines were
/// refused every Sentry surface under `oxy::` and still are under
/// `oxy_server::`; the rename must not change that either way.
const CUSTOM_APPS_MODULE: &str = "custom_apps_seams";

fn pairs() -> Vec<(String, String)> {
    MODULES
        .iter()
        .map(|module| (format!("oxy::{module}"), format!("oxy_server::{module}")))
        .collect()
}

/// Every level classifies the same at the new path as at the old one, and the
/// data-plane and log rules agree too.
#[test]
fn the_server_modules_classify_alike_after_the_lib_split() {
    for (old, new) in &pairs() {
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
        assert_eq!(drop_log(Some(old)), drop_log(Some(new)), "{old} -> {new}");
        assert_eq!(
            is_data_plane_module(old),
            is_data_plane_module(new),
            "{old} -> {new}"
        );
    }
}

/// Platform code: kept (an `error` is an event, nothing is dropped) and not
/// the data plane, at the old path and the new one alike. The new ident cannot
/// read as core's `oxy::connector` or `oxy::app_function` at all.
#[test]
fn the_server_modules_keep_their_sentry_lines_after_the_lib_split() {
    for module in MODULES.iter().filter(|m| **m != CUSTOM_APPS_MODULE) {
        for path in [format!("oxy::{module}"), format!("oxy_server::{module}")] {
            assert_eq!(
                sentry_disposition(&path, &Level::ERROR),
                Disposition::Event,
                "{path}"
            );
            assert!(!drop_log(Some(&path)), "{path}");
            assert!(!is_data_plane_module(&path), "{path}");
        }
    }
    assert!(!is_data_plane_module("oxy_server"));
    assert!(!drop_log(Some("oxy_server")));
}

/// The composition root's custom-app seams: refused before, refused after.
#[test]
fn the_custom_app_seams_stay_refused_after_the_lib_split() {
    for path in [
        format!("oxy::{CUSTOM_APPS_MODULE}"),
        format!("oxy_server::{CUSTOM_APPS_MODULE}"),
    ] {
        assert_eq!(
            sentry_disposition(&path, &Level::ERROR),
            Disposition::Drop,
            "{path}"
        );
        assert!(drop_log(Some(&path)), "{path}");
    }
}
