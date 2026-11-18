use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

use super::dead::signature_parts;
use super::queue::{PER_SECOND, Spread};
use super::replay::floor;
use super::reply::{first_line, listener};

#[test]
fn tasks_are_spread_at_ten_a_second_per_listener() {
    let start = Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap();
    let mut spread = Spread::new(start);
    let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let times: Vec<_> = (0..30).map(|_| spread.next(a, "on-event")).collect();
    assert_eq!(times[0], start);
    assert_eq!(
        times[1] - times[0],
        Duration::milliseconds(1_000 / PER_SECOND)
    );
    assert_eq!(times[29] - times[0], Duration::milliseconds(2_900));
    // Each listener has its own pace.
    assert_eq!(spread.next(b, "on-event"), start);
    assert_eq!(spread.next(a, "other"), start);
}

#[test]
fn a_replay_window_starts_no_earlier_than_retention() {
    let now = Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap();
    assert_eq!(
        floor(now - Duration::days(90), now),
        now - Duration::days(30)
    );
    assert_eq!(floor(now - Duration::days(2), now), now - Duration::days(2));
}

#[test]
fn a_listener_is_named_app_slash_function() {
    assert_eq!(
        listener("shop/on-order").ok(),
        Some(("shop".into(), "on-order".into()))
    );
    for bad in ["shop", "/fn", "shop/", ""] {
        assert!(listener(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn an_error_signature_is_the_outcome_and_the_first_line() {
    assert_eq!(first_line("boom\n  at handler (fn.js:3)"), "boom");
    assert_eq!(first_line("  spaced  "), "spaced");
    // No attempt: the delivery's own error, as `dead`.
    assert_eq!(
        signature_parts(&[], Some("all 5 attempts are spent\nmore")),
        ("dead".to_string(), "all 5 attempts are spent".to_string())
    );
    assert_eq!(
        signature_parts(&[], None),
        ("dead".to_string(), "(no error)".to_string())
    );
}
