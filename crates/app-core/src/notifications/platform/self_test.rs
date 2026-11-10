//! The test notice's cooldown: one test per person every [`COOLDOWN`]. The
//! notice itself is the notifications surface's (`platform::self_test` in
//! `oxy-api-notifications`); the staff console's app test, in
//! `oxy-api-custom-apps`, counts its own window by the same rule.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// How long a person waits between two test notices.
pub const COOLDOWN: Duration = Duration::from_secs(10);

/// Whole seconds left of the cooldown after a test sent at `last`, or `None`
/// once it has passed. At least 1 (a `Retry-After: 0` invites a retry storm)
/// and at most the cooldown, even when another replica's clock runs ahead.
pub fn cooldown_left(last: DateTime<Utc>, now: DateTime<Utc>) -> Option<u64> {
    let window = chrono::Duration::from_std(COOLDOWN).ok()?;
    let elapsed = now - last;
    if elapsed >= window {
        return None;
    }
    let left_ms = u64::try_from((window - elapsed).num_milliseconds()).unwrap_or(0);
    Some(left_ms.div_ceil(1000).clamp(1, COOLDOWN.as_secs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32, s: u32, ms: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 16, h, m, s).unwrap()
            + chrono::Duration::milliseconds(ms.into())
    }

    #[test]
    fn test_notice_cooldown_rounds_up_and_ends_at_ten_seconds() {
        let last = at(14, 0, 0, 0);
        assert_eq!(cooldown_left(last, last), Some(10));
        assert_eq!(cooldown_left(last, at(14, 0, 0, 1)), Some(10));
        assert_eq!(cooldown_left(last, at(14, 0, 9, 0)), Some(1));
        assert_eq!(cooldown_left(last, at(14, 0, 9, 999)), Some(1));
        assert_eq!(cooldown_left(last, at(14, 0, 10, 0)), None);
        assert_eq!(cooldown_left(last, at(14, 5, 0, 0)), None);
        assert_eq!(
            cooldown_left(at(14, 0, 3, 0), last),
            Some(10),
            "a replica whose clock runs ahead never asks for more than the cooldown"
        );
    }
}
