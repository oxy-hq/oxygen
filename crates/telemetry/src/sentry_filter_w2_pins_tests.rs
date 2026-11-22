//! Sentry classification pins for the modules decompose W2 moved down out of
//! `oxy-app`'s `agentic_wiring` into `oxy-workspace-runtime` (2026-11-22).
//! Beside `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// The metric-tree runner, its executor-facing core and the semantic scan
/// resolution: kept (an `error` is an event, nothing is dropped) and not the
/// data plane, at the old path and the new one alike. The runner runs warehouse
/// SQL, but through `agentic_connector`, which is the data plane in its own
/// right; `oxy_app::agentic_wiring::*` bar `sql_exec` never was, and the move
/// must not change that in either direction.
#[test]
fn the_w2_modules_keep_their_sentry_lines_after_moving_down() {
    for (old, new) in [
        (
            "oxy_app::agentic_wiring::metric_tree_runner",
            "oxy_workspace_runtime::metric_tree_runner",
        ),
        (
            "oxy_app::agentic_wiring::metric_tree_exec",
            "oxy_workspace_runtime::metric_tree_exec",
        ),
        (
            "oxy_app::agentic_wiring::scan_source",
            "oxy_workspace_runtime::scan_source",
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
