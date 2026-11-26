//! Sentry classification pins for the modules decompose W6a moved down out of
//! `oxy-app` into `oxy-workspace-runtime` (2026-11-22): the custom-app gate's
//! response half and project-context composition root (`custom_apps_gates`),
//! and the semantic boundary the semantic-analysis twins enter by. Beside
//! `sentry_filter.rs` rather than inside it: that file is over its size budget,
//! and these tests read only its public API.
//!
//! The move keeps both classifications exactly:
//!
//! - `custom_apps_gates` carried the `custom_apps` marker at its old path, so
//!   every line was dropped from every Sentry surface. Its new path,
//!   `oxy_workspace_runtime::custom_apps_gates`, carries the marker too.
//! - `semantic_boundary` sat under `oxy_app::server::api::projects`, which is
//!   kept (an `error` is an event) and is the data plane (its Sentry Logs are
//!   shaped). Its new path is deliberately not under a `custom_apps*` module,
//!   which would drop it, and has its own `DATA_PLANE_MODULE_PREFIXES` row,
//!   because a line can carry the semantic-model load's error text.
//!
//! The pins for the rest of the old `projects` module (the automation run's
//! row and its preview settle, from CA1 and P1) moved here from
//! `sentry_filter.rs` in the same PR, unchanged, to make room in that file's
//! size budget for the boundary's `DATA_PLANE_MODULE_PREFIXES` row.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// Dropped at every level and from Sentry Logs, and not the data plane, at the
/// old path and the new one alike.
#[test]
fn the_custom_app_gate_stays_dropped_after_moving_down() {
    for module in [
        "oxy_app::server::api::custom_apps_gates",
        "oxy_workspace_runtime::custom_apps_gates",
    ] {
        for level in [Level::ERROR, Level::WARN, Level::INFO] {
            assert_eq!(
                sentry_disposition(module, &level),
                Disposition::Drop,
                "{module} at {level}"
            );
        }
        assert!(drop_log(Some(module)), "{module}");
        assert!(!is_data_plane_module(module), "{module}");
    }
}

/// Kept (an `error` is an event, a `warn` a breadcrumb, the log not dropped)
/// and shaped as the data plane, at the old path and the new one alike.
#[test]
fn the_semantic_boundary_keeps_its_sentry_lines_after_moving_down() {
    for module in [
        "oxy_app::server::api::projects::semantic_boundary",
        "oxy_workspace_runtime::semantic_boundary",
    ] {
        assert_eq!(
            sentry_disposition(module, &Level::ERROR),
            Disposition::Event,
            "{module}"
        );
        assert_eq!(
            sentry_disposition(module, &Level::WARN),
            Disposition::Breadcrumb,
            "{module}"
        );
        assert!(!drop_log(Some(module)), "{module}");
        assert!(is_data_plane_module(module), "{module}");
    }
}

/// The new row is one module, matched a segment at a time: a sibling that
/// shares its letters, and the rest of the workspace runtime, are not the data
/// plane because of it.
#[test]
fn the_semantic_boundary_row_claims_only_its_module() {
    for module in [
        "oxy_workspace_runtime::semantic_boundary_x",
        "oxy_workspace_runtime",
        "oxy_workspace_runtime::project_ctx",
        "oxy_workspace_runtime::custom_apps_gates",
    ] {
        assert!(!is_data_plane_module(module), "{module}");
    }
}

/// The automation run's row state machine kept its classification when it
/// went down from `oxy_app::server::api::projects::automation_run` to
/// `oxy_custom_apps::automation_run` (2026-11-22): an issue at `error`, a
/// breadcrumb at `warn`, not dropped from Sentry Logs, and shaped there as
/// data plane — at the old path and the new one. The exemption is a path
/// segment: a sibling that shares the name, the surface's own
/// `automation_run` and every other module of the crate stay dropped. Its
/// preview settle kept the same class when it went down the same day to
/// `oxy_previews::queue::preview_settle`, and again when it moved beside the
/// state machine (`oxy_custom_apps::automation_run::preview_settle`,
/// 2026-11-26); its old queue siblings stay kept and unshaped.
#[test]
fn the_automation_run_row_keeps_its_sentry_class_after_the_move() {
    for module in [
        "oxy_app::server::api::projects::automation_run",
        "oxy_app::server::api::projects::automation_run::settle",
        "oxy_app::server::api::projects::automation_run::preview_settle",
        "oxy_custom_apps::automation_run",
        "oxy_custom_apps::automation_run::settle",
        // The preview settle at its path from 2026-11-22, and at its path
        // since 2026-11-26, beside the state machine.
        "oxy_previews::queue::preview_settle",
        "oxy_custom_apps::automation_run::preview_settle",
    ] {
        assert_eq!(
            sentry_disposition(module, &Level::ERROR),
            Disposition::Event,
            "{module}"
        );
        assert_eq!(
            sentry_disposition(module, &Level::WARN),
            Disposition::Breadcrumb,
            "{module}"
        );
        assert!(!drop_log(Some(module)), "{module}");
        assert!(is_data_plane_module(module), "{module}");
    }
    for module in [
        "oxy_custom_apps::automation_run_x",
        "oxy_custom_apps::storage::sweeper",
        "oxy_api_custom_apps::automation_run",
        "oxy_api_custom_apps::automation_run::executor",
    ] {
        assert_eq!(
            sentry_disposition(module, &Level::ERROR),
            Disposition::Drop,
            "{module}"
        );
        assert!(drop_log(Some(module)), "{module}");
    }
    // The preview settle's shaping is the one module, not its siblings
    // in the previews queue, which were never the data plane.
    for module in [
        "oxy_previews::queue",
        "oxy_previews::queue::sweep",
        "oxy_previews::queue::lifecycle",
        "oxy_previews::queue::preview_settle_x",
    ] {
        assert!(!drop_log(Some(module)), "{module}");
        assert!(!is_data_plane_module(module), "{module}");
    }
}
