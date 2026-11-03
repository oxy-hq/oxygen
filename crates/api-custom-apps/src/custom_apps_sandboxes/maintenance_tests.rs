//! The maintenance loop's pure rules: its interval, which planes expire
//! from the fork, and when a stuck teardown is retried.

use super::*;

/// Fifteen minutes by default; the override is whole seconds, never under
/// a minute. nextest runs each test in its own process, so the env writes
/// are its own.
#[test]
fn the_interval_is_fifteen_minutes_unless_overridden_and_never_under_one() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var(INTERVAL_ENV) };
    assert_eq!(
        MaintenanceConfig::from_env().interval,
        Duration::from_secs(900)
    );
    for (raw, secs) in [
        ("120", 120),
        (" 3600 ", 3600),
        ("5", 60),
        ("0", 60),
        ("soon", 900),
    ] {
        unsafe { std::env::set_var(INTERVAL_ENV, raw) };
        assert_eq!(
            MaintenanceConfig::from_env().interval,
            Duration::from_secs(secs),
            "{raw:?}"
        );
    }
}

/// Only a plane that copies production rows counts. OLTP `empty` copies
/// none; `schema` is a schema of the shared staging branch; storage is
/// never a reason.
#[test]
fn only_a_plane_that_copies_production_rows_expires_from_the_fork() {
    use oxy_custom_apps::planes::{PlaneSpec, StorageMode};
    let planes = |oltp: Option<OltpMode>| {
        let spec = PlaneSpec {
            oltp,
            storage: Some(StorageMode::Branch),
            ..PlaneSpec::default()
        };
        let sandbox = AppEnvironment::parse("dev-a1").expect("a sandbox");
        spec.resolve(&sandbox).expect("a sandbox has planes")
    };
    assert!(expires_from_fork(&planes(Some(OltpMode::Branch))));
    assert!(!expires_from_fork(&planes(Some(OltpMode::Empty))));
    assert!(!expires_from_fork(&planes(Some(OltpMode::Schema))));
    assert!(!expires_from_fork(&planes(None)), "the default is a schema");
    let sandbox = AppEnvironment::parse("dev-a1").expect("a sandbox");
    for (mode, expires) in [
        (AirhouseMode::Branch, true),
        (AirhouseMode::Snapshot, true),
        (AirhouseMode::WriteOnly, false),
    ] {
        let spec = PlaneSpec {
            airhouse: Some(mode),
            ..PlaneSpec::default()
        };
        let resolved = spec.resolve(&sandbox).expect("a sandbox has planes");
        assert_eq!(expires_from_fork(&resolved), expires, "airhouse {mode:?}");
    }
}

#[test]
fn a_teardown_is_retried_after_six_hours() {
    assert_eq!(stale_teardown(), chrono::Duration::hours(6));
}
