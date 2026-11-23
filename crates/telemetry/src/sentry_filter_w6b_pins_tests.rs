//! Sentry classification pins for the queued analytics run and the driver's
//! context builders, which went down from `oxy-app` into
//! `oxy-workspace-runtime` in decompose W6b (2026-11-23). Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The old and new module path of every module W6b moved.
fn moved_pairs() -> Vec<(String, String)> {
    let mut pairs = vec![
        (
            "oxy_app::server::queued_analytics".to_string(),
            "oxy_workspace_runtime::queued_analytics".to_string(),
        ),
        // The builders were part of `recovery.rs` itself; the rest of that
        // module stays in `oxy-app` under the old path.
        (
            "oxy_app::server::router::recovery".to_string(),
            "oxy_workspace_runtime::driver_ctx".to_string(),
        ),
        (
            "oxy_app::server::router::recovery::worker_ctx".to_string(),
            "oxy_workspace_runtime::driver_ctx::worker_ctx".to_string(),
        ),
    ];
    for sub in ["admission", "caller", "chat", "heartbeat", "requester"] {
        pairs.push((
            format!("oxy_app::server::queued_analytics::{sub}"),
            format!("oxy_workspace_runtime::queued_analytics::{sub}"),
        ));
    }
    pairs
}

/// Platform code in `oxy-app`: kept (an `error` is an event, nothing is
/// dropped) and not the data plane — neither path was ever a
/// `DATA_PLANE_MODULE_PREFIXES` entry, and neither carries the `custom_apps`
/// marker, so the move changes nothing in either direction. Most of the
/// builders' lines name `target: "recovery"` or `"health_eval"`, which the
/// move does not touch; the rest log under the module path pinned here.
#[test]
fn the_w6b_modules_keep_their_sentry_lines_after_moving_down() {
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
fn the_w6b_modules_classify_alike_at_every_level() {
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
