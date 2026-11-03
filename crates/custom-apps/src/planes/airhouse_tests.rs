use chrono::{Duration, TimeZone, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a sandbox")
}

#[test]
fn a_pin_reads_within_the_window_only() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let window = Duration::hours(24);
    assert_eq!(pin_refusal(now - Duration::hours(23), now, window), None);
    assert_eq!(pin_refusal(now, now, window), None);
    let stale = pin_refusal(now - Duration::hours(25), now, window).expect("stale");
    assert!(stale.contains("older than 24 hours"), "{stale}");
    let future = pin_refusal(now + Duration::minutes(1), now, window).expect("future");
    assert!(future.contains("in the future"), "{future}");
}

#[test]
fn the_window_falls_back_to_a_day_on_a_bad_value() {
    assert_eq!(parse_window_hours(None), 24);
    assert_eq!(parse_window_hours(Some(" ")), 24);
    assert_eq!(parse_window_hours(Some("72")), 72);
    for bad in ["0", "-3", "1.5", "a day"] {
        assert_eq!(parse_window_hours(Some(bad)), 24, "{bad}");
    }
}

/// Staging is never torn down, so it may not hold copies of production:
/// both overlay modes are refused there, with the reason.
#[test]
fn staging_refuses_the_overlay_modes() {
    let now = Utc::now();
    for mode in [AirhouseMode::Branch, AirhouseMode::Snapshot] {
        let why = check(mode, &AppEnvironment::Staging, None, now).expect_err("staging");
        assert!(why.contains("sandboxes only"), "{why}");
        assert!(why.contains("30 days"), "{why}");
        assert!(why.contains("write-only"), "{why}");
        check(mode, &sandbox(), None, now).expect("a sandbox may");
    }
    check(AirhouseMode::WriteOnly, &AppEnvironment::Staging, None, now).expect("the default");
}

#[test]
fn snapshot_refuses_a_fork_point_outside_the_window() {
    let now = Utc::now();
    let old = Some(now - Duration::days(3));
    let why = check(AirhouseMode::Snapshot, &sandbox(), old, now).expect_err("stale");
    assert!(why.contains("--at"), "{why}");
    check(
        AirhouseMode::Snapshot,
        &sandbox(),
        Some(now - Duration::hours(1)),
        now,
    )
    .expect("recent");
    check(AirhouseMode::Snapshot, &sandbox(), None, now).expect("pins now");
    // Only `snapshot` reads at the fork point.
    check(AirhouseMode::Branch, &sandbox(), old, now).expect("branch copies live");
}
