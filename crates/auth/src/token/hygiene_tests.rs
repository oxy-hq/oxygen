use super::*;

fn day(n: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap() + Duration::days(n)
}

#[test]
fn the_notice_is_due_inside_the_last_seven_days_only() {
    let created = day(-100);
    let expires = Some(day(0));
    assert!(!notice_due(day(-8), created, expires, false), "too early");
    assert!(
        notice_due(day(-7), created, expires, false),
        "the window opens"
    );
    assert!(notice_due(day(-1), created, expires, false));
    assert!(
        !notice_due(day(0), created, expires, false),
        "already expired"
    );
    assert!(!notice_due(day(1), created, expires, false));
}

#[test]
fn the_notice_goes_out_once_per_expiry() {
    let created = day(-100);
    assert!(
        !notice_due(day(-3), created, Some(day(0)), true),
        "already sent"
    );
    // Extend clears the stamp; the new expiry's own window decides again.
    assert!(!notice_due(day(-3), created, Some(day(30)), false));
    assert!(notice_due(day(24), created, Some(day(30)), false));
}

#[test]
fn a_token_without_expiry_or_born_inside_its_window_is_not_mailed() {
    assert!(!notice_due(day(0), day(-100), None, false), "never expires");
    // Minted with a five-day life: its owner chose it, nothing to warn of.
    assert!(!notice_due(day(-2), day(-5), Some(day(0)), false));
    // Minted exactly as the window opens: it existed, so it is mailed.
    assert!(notice_due(day(-2), day(-7), Some(day(0)), false));
}

#[test]
fn unused_means_nothing_happened_for_a_year() {
    let now = day(0);
    let over = day(-UNUSED_DAYS - 1);
    let under = day(-UNUSED_DAYS + 1);
    assert!(
        unused(now, over, None, None),
        "never used, created over a year ago"
    );
    assert!(!unused(now, under, None, None), "never used, but young");
    assert!(
        unused(now, day(-900), Some(over), None),
        "last used over a year ago"
    );
    assert!(
        !unused(now, day(-900), Some(under), None),
        "used within the year"
    );
}

#[test]
fn a_renewal_counts_as_activity() {
    let now = day(0);
    // Unused for years, then extended (or regenerated) by its owner: the
    // sweep must not expire it again on the next pass.
    assert!(!unused(now, day(-900), Some(day(-800)), Some(day(-1))));
    assert!(unused(now, day(-900), None, Some(day(-UNUSED_DAYS - 10))));
}
