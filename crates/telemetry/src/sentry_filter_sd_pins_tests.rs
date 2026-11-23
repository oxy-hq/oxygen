//! Sentry classification pins for decompose SD (2026-11-24), which moved the
//! demo seed (`oxy dev seed`) up from `oxy-app`'s `cli::commands::seed*` into
//! `oxy-server`'s `dev::seed`, with the seed half of `cli::commands::dev`
//! (`handle_seed_command`). Beside `sentry_filter.rs` rather than inside it:
//! that file is over its size budget, and these tests read only its public API.
//!
//! The library logs as `oxy_server::…`, an ident no rule shares with core's
//! `oxy::…`, so nothing moved here can read as `oxy::connector` or
//! `oxy::app_function`. Every moved module is listed, old path and new, and
//! each must classify the same at both.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// `(old path in oxy-app, new path in oxy-server)` for every module the move
/// touched. The files kept their names, so each `seed_*` module sits under
/// `dev::seed`; `seed.rs` itself is `dev::seed::seed`, and the command's
/// dispatch (`handle_seed_command`, out of `cli::commands::dev`) is
/// `dev::seed`.
const PAIRS: &[(&str, &str)] = &[
    ("oxy_app::cli::commands::dev", "oxy_server::dev::seed"),
    (
        "oxy_app::cli::commands::seed",
        "oxy_server::dev::seed::seed",
    ),
    (
        "oxy_app::cli::commands::seed_airhouse",
        "oxy_server::dev::seed::seed_airhouse",
    ),
    (
        "oxy_app::cli::commands::seed_compile",
        "oxy_server::dev::seed::seed_compile",
    ),
    (
        "oxy_app::cli::commands::seed_llm_keys",
        "oxy_server::dev::seed::seed_llm_keys",
    ),
    (
        "oxy_app::cli::commands::seed_org_workspace",
        "oxy_server::dev::seed::seed_org_workspace",
    ),
    (
        "oxy_app::cli::commands::seed_partners",
        "oxy_server::dev::seed::seed_partners",
    ),
    (
        "oxy_app::cli::commands::seed_platform_grants",
        "oxy_server::dev::seed::seed_platform_grants",
    ),
    (
        "oxy_app::cli::commands::seed_threads",
        "oxy_server::dev::seed::seed_threads",
    ),
];

/// Every level classifies the same at the new path as at the old one, and the
/// data-plane and log rules agree too.
#[test]
fn the_seed_modules_classify_alike_after_the_move() {
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
/// marker, and none falls under a `DATA_PLANE_MODULE_PREFIXES` entry.
#[test]
fn the_seed_modules_keep_their_sentry_lines_after_the_move() {
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
