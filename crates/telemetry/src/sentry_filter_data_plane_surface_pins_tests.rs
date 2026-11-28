//! Sentry classification pins for crate-depth B2's moves into
//! `oxy-api-data-plane`: the `/oltp/me/*` routes (`oxy_oltp::api`), and the
//! staff console's OLTP and Airhouse sections with the handlers they fence
//! (`oxy_api_tenancy::admin::{oltp, airhouse}`). Beside `sentry_filter.rs`
//! rather than inside it: that file is over its size budget, and these tests
//! read only its public API.
//!
//! The move keeps the class exactly. At their old paths none of these was a
//! custom-app module or the data plane (`oxy_oltp::api` was never a listed
//! prefix): their `error!` lines name a connect or lookup failure over Oxy's
//! own Postgres, never a tenant's rows. The crate's name carries `data_plane`,
//! which is why this is pinned: no rule may start reading it as a marker.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

#[test]
fn the_data_plane_surface_keeps_its_sentry_class_after_the_move() {
    for module in [
        "oxy_api_tenancy::admin::oltp",
        "oxy_api_tenancy::admin::airhouse",
        "oxy_api_data_plane::admin::oltp",
        "oxy_api_data_plane::admin::airhouse",
        "oxy_api_data_plane::admin::airhouse::inner",
        "oxy_api_data_plane::admin::airhouse::inner::rotate_sa",
        "oxy_api_data_plane::oltp",
        "oxy_api_data_plane::oltp::admin",
        "oxy_api_data_plane::oltp::erd",
        "oxy_api_data_plane::oltp::probe",
        "oxy_api_data_plane::airhouse::handlers",
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
