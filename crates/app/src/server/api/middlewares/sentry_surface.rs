//! Moved to [`oxy_telemetry::sentry_surface`] (2026-11-21): the custom-app
//! Sentry surface tag, barrier 2. Re-exported here so `oxy_app::host` and
//! `oxy_app::surface` keep their paths. Its tests, the hub-carry source walks
//! included, are `tests/routing/sentry_hub_scan.rs`.

pub use oxy_telemetry::sentry_surface::{
    HubBoundStream, bind_hub_stream, custom_app_hub, mark_custom_app_surface,
    tag_custom_app_surface,
};
