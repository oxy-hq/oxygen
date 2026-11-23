//! Sentry classification pins for the router seams, which went down from
//! `oxy-app` into `oxy-app-core` in decompose R0 (2026-11-23). Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The old and new module path of the one module R0 moved that logs under
/// its module path: `install_global_ticks`'s `warn!` on a second, different
/// tick list. (The tick runner's `info!` names `target: "global_tick"`, which
/// the move does not touch; `bare_app_state` and the driver's interval const
/// log nothing.)
const MOVED: (&str, &str) = ("oxy_app::server::router::seams", "oxy_app_core::seams");

/// Platform code: kept (an `error` is an event, nothing is dropped) and not
/// the data plane. Neither path was ever a `DATA_PLANE_MODULE_PREFIXES` entry
/// and neither carries the `custom_apps` marker, so the move changes nothing
/// in either direction.
#[test]
fn the_r0_seams_keep_their_sentry_lines_after_moving_down() {
    let (old, new) = MOVED;
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

/// Every level classifies the same at the new path as at the old one.
#[test]
fn the_r0_seams_classify_alike_at_every_level() {
    let (old, new) = MOVED;
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
