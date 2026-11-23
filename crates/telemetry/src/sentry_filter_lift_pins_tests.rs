//! Sentry classification pins for decompose LIFT (2026-11-23), which moved the
//! router assembly up from `oxy-app` into `oxy-server`'s library: the router
//! and its middlewares, the admin console shell, the route-role installer and
//! its tests' files, the local `/setup` handlers and the boot's auth-provider
//! check. Beside `sentry_filter.rs` rather than inside it: that file is over
//! its size budget, and these tests read only its public API.
//!
//! Since decompose LT the library logs as `oxy_server::…`, an ident no rule
//! shares with core's `oxy::…`, so nothing moved here can read as
//! `oxy::connector` or `oxy::app_function`. Every moved module is listed, old
//! path and new, and each must classify the same at both.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// `(old path in oxy-app, new path in oxy-server)` for every non-test module
/// the lift moved. `server::router::workspace_cache`, a re-export with no code
/// and so no log line, was deleted rather than moved.
const PAIRS: &[(&str, &str)] = &[
    ("oxy_app::server::router", "oxy_server::router"),
    (
        "oxy_app::server::router::entry",
        "oxy_server::router::entry",
    ),
    (
        "oxy_app::server::router::global",
        "oxy_server::router::global",
    ),
    (
        "oxy_app::server::router::openapi",
        "oxy_server::router::openapi",
    ),
    (
        "oxy_app::server::router::protected",
        "oxy_server::router::protected",
    ),
    (
        "oxy_app::server::router::public",
        "oxy_server::router::public",
    ),
    (
        "oxy_app::server::router::workspace",
        "oxy_server::router::workspace",
    ),
    (
        "oxy_app::server::api::local_setup",
        "oxy_server::router::local_setup",
    ),
    (
        "oxy_app::server::api::middlewares",
        "oxy_server::router::middlewares",
    ),
    (
        "oxy_app::server::api::middlewares::api_key_query",
        "oxy_server::router::middlewares::api_key_query",
    ),
    (
        "oxy_app::server::api::middlewares::app_grant_scope",
        "oxy_server::router::middlewares::app_grant_scope",
    ),
    (
        "oxy_app::server::api::middlewares::app_publish_token_scope",
        "oxy_server::router::middlewares::app_publish_token_scope",
    ),
    (
        "oxy_app::server::api::middlewares::local_context",
        "oxy_server::router::middlewares::local_context",
    ),
    (
        "oxy_app::server::api::middlewares::request_id",
        "oxy_server::router::middlewares::request_id",
    ),
    (
        "oxy_app::server::api::middlewares::timeout",
        "oxy_server::router::middlewares::timeout",
    ),
    (
        "oxy_app::server::api::middlewares::token_grant_scope",
        "oxy_server::router::middlewares::token_grant_scope",
    ),
    (
        "oxy_app::server::api::middlewares::token_usage",
        "oxy_server::router::middlewares::token_usage",
    ),
    ("oxy_app::server::api::admin", "oxy_server::admin"),
    (
        "oxy_app::server::role_manifest",
        "oxy_server::role_manifest",
    ),
    (
        "oxy_app::server::role_middleware",
        "oxy_server::role_middleware",
    ),
    (
        "oxy_app::server::auth_provider_check",
        "oxy_server::auth_provider_check",
    ),
];

/// Every level classifies the same at the new path as at the old one, and the
/// data-plane and log rules agree too.
#[test]
fn the_router_modules_classify_alike_after_the_lift() {
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
/// and none of it is the data plane. In particular no moved path carries the
/// `custom_apps` marker (`app_publish_token_scope` and `app_grant_scope` do
/// not), and none falls under a `DATA_PLANE_MODULE_PREFIXES` entry.
#[test]
fn the_router_modules_keep_their_sentry_lines_after_the_lift() {
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
