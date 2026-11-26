//! Sentry classification pins for decompose CLI (2026-11-25), which moved the
//! `oxy` CLI's one-shot commands up from `oxy-app`'s `cli` into `oxy-server`'s
//! `cli` (`compile`, `migrate`, `migrate-agents`, the dev `compile` and the
//! one-release stubs, with `SubCommand`, `prepare` and the panic hook), and
//! `managed_connection_resolver` beside them. Beside `sentry_filter.rs` rather
//! than inside it: that file is over its size budget, and these tests read
//! only its public API.
//!
//! The library logs as `oxy_server::…`, an ident no rule shares with core's
//! `oxy::…`, so nothing moved here can read as `oxy::connector` or
//! `oxy::app_function`. Every moved module is listed, old path and new, and
//! each must classify the same at both.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// `(old path in oxy-app, new path in oxy-server)` for every module the move
/// touched that still exists (the one-release stubs' six modules were deleted
/// 2026-11-26, so their pairs went with them). The subtree kept its shape, so each path only changes its crate.
const PAIRS: &[(&str, &str)] = &[
    ("oxy_app::cli", "oxy_server::cli"),
    ("oxy_app::cli::commands", "oxy_server::cli::commands"),
    (
        "oxy_app::cli::commands::compile",
        "oxy_server::cli::commands::compile",
    ),
    (
        "oxy_app::cli::commands::dev",
        "oxy_server::cli::commands::dev",
    ),
    (
        "oxy_app::cli::commands::migrate",
        "oxy_server::cli::commands::migrate",
    ),
    (
        "oxy_app::cli::commands::migrate_agents",
        "oxy_server::cli::commands::migrate_agents",
    ),
    (
        "oxy_app::cli::commands::migrate_agents::refs",
        "oxy_server::cli::commands::migrate_agents::refs",
    ),
    (
        "oxy_app::cli::commands::migrate_agents::report",
        "oxy_server::cli::commands::migrate_agents::report",
    ),
    (
        "oxy_app::cli::commands::migrate_agents::translate",
        "oxy_server::cli::commands::migrate_agents::translate",
    ),
    (
        "oxy_app::managed_connection_resolver",
        "oxy_server::managed_connection_resolver",
    ),
];

/// Every level classifies the same at the new path as at the old one, and the
/// data-plane and log rules agree too.
#[test]
fn the_cli_modules_classify_alike_after_the_move() {
    for (old, new) in PAIRS {
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
        assert_eq!(drop_log(Some(*old)), drop_log(Some(*new)), "{old} -> {new}");
        assert_eq!(
            is_data_plane_module(old),
            is_data_plane_module(new),
            "{old} -> {new}"
        );
    }
}

/// Platform code, at both paths: an `error` is an event, no line is dropped,
/// and none of it is the data plane. No moved path carries the `custom_apps`
/// marker, and none falls under a `DATA_PLANE_MODULE_PREFIXES` entry; the
/// resolver mints managed connections but runs no customer query, and it was
/// never the data plane at its old path either.
#[test]
fn the_cli_modules_keep_their_sentry_lines_after_the_move() {
    for (old, new) in PAIRS {
        for path in [*old, *new] {
            assert_eq!(
                sentry_disposition(path, &Level::ERROR),
                Disposition::Event,
                "{path}"
            );
            assert!(!drop_log(Some(path)), "{path}");
            assert!(!is_data_plane_module(path), "{path}");
            assert!(!path.contains("custom_apps"), "{path}");
        }
    }
}
