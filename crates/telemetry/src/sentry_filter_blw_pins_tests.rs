//! Sentry classification pins for `oxy serve`'s boot jobs, which left
//! `oxy-app`'s `api_router` (`server::router::entry`) for `oxy-server`'s
//! `commands::serve_background` in decompose BL-W (2026-11-23). Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.
//!
//! The new path read `oxy::commands::serve_background` in the binary and reads
//! `oxy_server::commands::serve_background` since decompose LT gave the crate a
//! library (`sentry_filter_lt_pins_tests` pins the two alike). Neither may read
//! as the data plane (`oxy::connector`) or as custom-app code
//! (`oxy::app_function`).

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The old and new module path of the boot section's one log line (the
/// `--no-workers` notice). Every job it starts logs from its own module, which
/// the move does not touch.
const MOVED: &[(&str, &str)] = &[(
    "oxy_app::server::router::entry",
    "oxy_server::commands::serve_background",
)];

/// Platform code: kept (an `error` is an event, nothing is dropped) and not
/// the data plane, at the old path and the new one alike.
#[test]
fn the_serve_boot_keeps_its_sentry_lines_after_moving_up() {
    for (old, new) in MOVED {
        for module in [*old, *new] {
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
fn the_serve_boot_classifies_alike_at_every_level() {
    for (old, new) in MOVED {
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
