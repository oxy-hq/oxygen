//! Sentry classification pins for the modules decompose W7 moved down out of
//! `oxy-app` into `oxy-workspace-runtime::request` (2026-11-22): the workspace
//! middleware, the extractor that reads its row, the Factory-access gate and
//! its route lists, and the preview read-only guard's axum adapter. Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size budget,
//! and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// Kept (an `error` is an event, nothing is dropped) and not the data plane, at
/// the old path and the new one alike. The middleware authorizes, pins a
/// revision and builds a project context; it runs no warehouse query, and
/// `oxy_app::server` never was the data plane. The move must not change that
/// in either direction, and `request` must not read as a `custom_apps` path.
#[test]
fn the_w7_modules_keep_their_sentry_lines_after_moving_down() {
    for (old, new) in [
        (
            "oxy_app::server::api::middlewares::workspace_context",
            "oxy_workspace_runtime::request::workspace_context",
        ),
        (
            "oxy_app::server::api::middlewares::workspace_extractor",
            "oxy_workspace_runtime::request::workspace_extractor",
        ),
        (
            "oxy_app::server::factory_access",
            "oxy_workspace_runtime::request::factory_access",
        ),
        (
            "oxy_app::server::factory_access::routes",
            "oxy_workspace_runtime::request::factory_access::routes",
        ),
        (
            "oxy_app::server::previews::read_only",
            "oxy_workspace_runtime::request::preview_read_only",
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
