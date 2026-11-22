//! Sentry classification pins for the modules decompose W4 moved down out of
//! `oxy-app` into `oxy-workspace-runtime` (2026-11-22): the project context,
//! the compile dispatcher, the preagg cycle's task executor and the recovery
//! path's context cache. Beside `sentry_filter.rs` rather than inside it: that
//! file is over its size budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// Kept (an `error` is an event, nothing is dropped) and not the data plane, at
/// the old path and the new one alike. The project context builds connectors
/// (`agentic_connector` runs the query, and is the data plane in its own
/// right); `oxy_app::agentic_wiring::*` bar `sql_exec` and `oxy_app::server`
/// never were, and the move must not change that in either direction.
#[test]
fn the_w4_modules_keep_their_sentry_lines_after_moving_down() {
    let mut pairs = vec![
        (
            "oxy_app::agentic_wiring::project_ctx".to_string(),
            "oxy_workspace_runtime::project_ctx".to_string(),
        ),
        (
            "oxy_app::agentic_wiring::compile_dispatcher".to_string(),
            "oxy_workspace_runtime::compile_dispatcher".to_string(),
        ),
        (
            "oxy_app::server::preagg_executor".to_string(),
            "oxy_workspace_runtime::preagg_executor".to_string(),
        ),
        (
            "oxy_app::server::router::workspace_cache".to_string(),
            "oxy_workspace_runtime::workspace_cache".to_string(),
        ),
    ];
    // `project_ctx`'s submodules log under their own targets.
    for sub in [
        "agent_lookup",
        "compile_cooldown",
        "context_documents",
        "context_root",
        "function_context",
        "monitor_scan",
        "preview_hold",
        "project_context",
        "react_flag",
        "revision_rebuild",
        "workspace_context",
    ] {
        pairs.push((
            format!("oxy_app::agentic_wiring::project_ctx::{sub}"),
            format!("oxy_workspace_runtime::project_ctx::{sub}"),
        ));
    }
    for (old, new) in &pairs {
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
