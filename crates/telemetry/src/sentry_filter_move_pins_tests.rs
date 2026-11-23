//! Sentry classification pins for modules that moved up out of `oxy-app` in
//! decompose UP2 (2026-11-22). Beside `sentry_filter.rs` rather than inside
//! it: that file is over its size budget, and these tests read only its
//! public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The standalone worker's runtime, health listener and metrics, and the
/// boot-time shared-state check, went up from `oxy-app` into the `oxy` binary
/// (`oxy-server`). Platform code there, kept and not the data plane, and
/// pinned so here, under `oxy_server::` since decompose LT gave the crate a
/// library (its `oxy::` paths until then: `sentry_filter_lt_pins_tests`). None
/// may read as `oxy::connector` (data plane) or `oxy::app_function`. `open_master_key`
/// went down into `oxy_platform::secrets::master_key` the same day.
#[test]
fn the_worker_modules_keep_their_sentry_lines_after_moving_up() {
    for (old, new) in [
        (
            "oxy_app::server::worker_runtime",
            "oxy_server::worker_runtime",
        ),
        (
            "oxy_app::server::worker_health",
            "oxy_server::worker_health",
        ),
        (
            "oxy_app::server::worker_metrics",
            "oxy_server::worker_metrics",
        ),
        (
            "oxy_app::server::shared_state_check",
            "oxy_server::shared_state_check",
        ),
        (
            "oxy_app::server::shared_state_check",
            "oxy_platform::secrets::master_key::entry",
        ),
    ] {
        for module in [old, new] {
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
