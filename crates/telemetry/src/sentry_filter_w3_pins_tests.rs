//! Sentry classification pins for the preview connector and its DuckDB double,
//! which went down from `oxy-app` into `oxy-workspace-runtime` in decompose W3
//! (2026-11-22). Beside `sentry_filter.rs` rather than inside it: that file is
//! over its size budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// `agentic_wiring::preview_airhouse` (and its `backend`, `batch` and `ports`)
/// and `server::previews::airhouse_duckdb` were platform code in `oxy-app`:
/// kept, and not the data plane (neither path was ever a
/// `DATA_PLANE_MODULE_PREFIXES` entry). They stay so at their new paths, and
/// the old ones, still re-exported, are pinned beside them.
#[test]
fn the_preview_connector_keeps_its_sentry_lines_after_moving_down() {
    for module in [
        "oxy_app::agentic_wiring::preview_airhouse",
        "oxy_workspace_runtime::preview_airhouse",
        "oxy_workspace_runtime::preview_airhouse::backend",
        "oxy_workspace_runtime::preview_airhouse::batch",
        "oxy_workspace_runtime::preview_airhouse::ports",
        "oxy_app::server::previews::airhouse_duckdb",
        "oxy_workspace_runtime::airhouse_duckdb",
    ] {
        assert_eq!(
            sentry_disposition(module, &Level::ERROR),
            Disposition::Event,
            "{module}"
        );
        assert!(!drop_log(Some(module)), "{module}");
        assert!(!is_data_plane_module(module), "{module}");
    }
}
