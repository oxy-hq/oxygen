//! `AppResponse::staging_url` / `staging_url_for` — see `dto.rs`.
//!
//! `OXY_API_URL` is process-global, so every test here serializes on
//! `env_lock()` — a mutex local to this module (the var is also mutated by
//! `custom_apps_host_dispatch`'s tests, but those live in the separate
//! `oxy-app-core` crate and can't share a lock with this one). What actually
//! keeps the two from racing is `cargo nextest`'s one-process-per-test
//! isolation; `env_lock()` is defense in depth against another `oxy-app` lib
//! test doing the same thing in the same process (e.g. under plain `cargo
//! test`, which does not isolate like that).

use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use super::staging_url_for;
use crate::server::api::custom_apps_env_resolve::EnvironmentBuilds;

const ORG: &str = "acme";
const APP: &str = "warehouse";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Runs `body` with `OXY_API_URL` set to `api_url` (or unset, for `None`),
/// restoring it to unset afterward. Holds `env_lock()` for the duration.
fn with_api_url<T>(api_url: Option<&str>, body: impl FnOnce() -> T) -> T {
    let _guard = env_lock().lock().unwrap();
    match api_url {
        Some(url) => unsafe { std::env::set_var("OXY_API_URL", url) },
        None => unsafe { std::env::remove_var("OXY_API_URL") },
    }
    let result = body();
    unsafe { std::env::remove_var("OXY_API_URL") };
    result
}

#[test]
fn none_when_the_app_has_no_staging_build() {
    with_api_url(Some("https://app.oxygen-hq.com/api"), || {
        let builds = EnvironmentBuilds {
            production: None,
            staging: None,
        };
        assert_eq!(staging_url_for(ORG, APP, &builds), None);
    });
}

#[test]
fn none_when_staging_is_the_same_build_already_live() {
    with_api_url(Some("https://app.oxygen-hq.com/api"), || {
        let build = Uuid::new_v4();
        let builds = EnvironmentBuilds {
            production: Some(build),
            staging: Some(build),
        };
        assert_eq!(staging_url_for(ORG, APP, &builds), None);
    });
}

#[test]
fn some_when_staging_has_a_distinct_build_and_the_zone_derives() {
    with_api_url(Some("https://app.oxygen-hq.com/api"), || {
        let builds = EnvironmentBuilds {
            production: Some(Uuid::new_v4()),
            staging: Some(Uuid::new_v4()),
        };
        assert_eq!(
            staging_url_for(ORG, APP, &builds).as_deref(),
            Some("https://staging--acme--warehouse.customer-apps.oxygen-hq.com/")
        );
    });
}

#[test]
fn some_when_the_app_has_never_been_promoted_and_staging_alone_has_a_build() {
    with_api_url(Some("https://app-dev.oxygen-hq.com/api"), || {
        let builds = EnvironmentBuilds {
            production: None,
            staging: Some(Uuid::new_v4()),
        };
        assert_eq!(
            staging_url_for(ORG, APP, &builds).as_deref(),
            Some("https://staging--acme--warehouse.customer-apps-dev.oxygen-hq.com/")
        );
    });
}

#[test]
fn none_when_the_zone_cannot_be_derived_even_with_a_distinct_staging_build() {
    // Local dev (`http://localhost:3000`) is the documented case where
    // `custom_apps_zone()` returns `None` — no `.` in the host at all.
    with_api_url(Some("http://localhost:3000/api"), || {
        let builds = EnvironmentBuilds {
            production: Some(Uuid::new_v4()),
            staging: Some(Uuid::new_v4()),
        };
        assert_eq!(staging_url_for(ORG, APP, &builds), None);
    });
}

#[test]
fn none_when_oxy_api_url_is_unset() {
    with_api_url(None, || {
        let builds = EnvironmentBuilds {
            production: Some(Uuid::new_v4()),
            staging: Some(Uuid::new_v4()),
        };
        assert_eq!(staging_url_for(ORG, APP, &builds), None);
    });
}
