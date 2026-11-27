//! Sentry classification pins for the Airhouse tenant client, down from
//! `oxy_cameras::airhouse` into `airhouse::tenant` (crate-tiers stage D1).
//! Beside `sentry_filter.rs` rather than inside it: that file is over its size
//! budget, and these tests read only its public API.
//!
//! The move keeps the class exactly. At its old path the client was neither a
//! custom-app module nor the data plane: an `error` is an event, a `warn` a
//! breadcrumb, and its Sentry Logs keep their body. `airhouse::connector` *is* a
//! data-plane prefix, so the new module is deliberately a sibling of it
//! (`airhouse::tenant`), not under it: these lines are Oxy's own system writes
//! (camera ingest, the usage drain), not a tenant's warehouse queries.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

#[test]
fn the_tenant_client_keeps_its_sentry_class_after_the_move() {
    for module in [
        "oxy_cameras::airhouse",
        "oxy_cameras::airhouse::client",
        "airhouse::tenant",
        "airhouse::tenant::client",
        "airhouse::tenant::escape",
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
        assert!(!is_data_plane_module(module), "{module}");
    }
    // The connector next to it stays the data plane.
    assert!(is_data_plane_module("airhouse::connector"));
}
