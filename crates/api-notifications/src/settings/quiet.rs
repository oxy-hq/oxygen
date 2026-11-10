//! Quiet hours: a daily window, in the person's timezone, during which push
//! and Slack hold back what is below the level it lets through.
//!
//! `days` are the days a window *starts* on. A window whose end is before its
//! start wraps past midnight and runs into the next morning, so
//! `22:00–07:00` on Friday covers Friday 22:00 to Saturday 07:00, and
//! Saturday 06:00 is quiet even when Saturday is not one of the days. The end
//! is exclusive: at 07:00 the window is over.
//!
//! Pure: everything takes `now` and a timezone.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::catalog::model::Level;

/// Minutes in a day: a time of day is `0..MINUTES_PER_DAY`.
pub const MINUTES_PER_DAY: u16 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    /// Minutes after local midnight.
    pub start: u16,
    pub end: u16,
    /// Bit `d` set: a window starts on day `d` (0 = Sunday).
    pub days: u8,
    /// The lowest level still let through: `TimeSensitive` or `Critical`.
    pub allow: Level,
}

/// `QuietHours` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuietHoursView {
    pub start: String,
    pub end: String,
    pub days: Vec<u8>,
    pub allow: Level,
}

/// `HH:MM`, 24-hour, two digits each. `None` otherwise.
pub fn parse_time(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !(h.bytes().chain(m.bytes())).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_time(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// An IANA timezone name, as `chrono-tz` knows it.
pub fn parse_tz(name: &str) -> Option<Tz> {
    name.parse().ok()
}

/// The days as bits; `None` for a day outside `0..=6`.
pub fn days_mask(days: &[u8]) -> Option<u8> {
    days.iter()
        .try_fold(0u8, |mask, d| (*d < 7).then(|| mask | (1 << d)))
}

impl QuietHours {
    pub fn view(&self) -> QuietHoursView {
        QuietHoursView {
            start: format_time(self.start),
            end: format_time(self.end),
            days: (0..7).filter(|d| self.starts_on(*d)).collect(),
            allow: self.allow,
        }
    }

    fn starts_on(&self, day: u8) -> bool {
        self.days & (1 << day) != 0
    }

    fn wraps(&self) -> bool {
        self.end < self.start
    }

    /// Whether a notice of `level` comes through the window.
    pub fn lets_through(&self, level: Level) -> bool {
        level >= self.allow
    }

    /// Whether `now` is inside a window, read in `tz`.
    pub fn active_at(&self, now: DateTime<Utc>, tz: Tz) -> bool {
        let local = now.with_timezone(&tz);
        let t = (local.hour() * 60 + local.minute()) as u16;
        let today = local.weekday().num_days_from_sunday() as u8;
        let yesterday = (today + 6) % 7;
        if self.wraps() {
            (self.starts_on(today) && t >= self.start)
                || (self.starts_on(yesterday) && t < self.end)
        } else {
            self.starts_on(today) && self.start <= t && t < self.end
        }
    }

    /// When the window `now` is in ends, as an instant; `None` outside one.
    pub fn ends_after(&self, now: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
        if !self.active_at(now, tz) {
            return None;
        }
        let local = now.with_timezone(&tz);
        let t = (local.hour() * 60 + local.minute()) as u16;
        // The evening half of a wrapping window ends tomorrow morning.
        let date = match self.wraps() && t >= self.start {
            true => local.date_naive() + Duration::days(1),
            false => local.date_naive(),
        };
        let end = date.and_hms_opt(u32::from(self.end / 60), u32::from(self.end % 60), 0)?;
        Some(resolve(tz, end, now))
    }
}

/// A local wall-clock time as the first instant after `now` that has it. In
/// a repeated hour that is the earlier of the two, unless it has already
/// passed — inside the second pass through the hour, the end is the later
/// one. In a skipped hour, the same wall time an hour later.
fn resolve(tz: Tz, local: NaiveDateTime, now: DateTime<Utc>) -> DateTime<Utc> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => t.with_timezone(&Utc),
        LocalResult::Ambiguous(first, second) => {
            let first = first.with_timezone(&Utc);
            match first > now {
                true => first,
                false => second.with_timezone(&Utc),
            }
        }
        LocalResult::None => tz
            .from_local_datetime(&(local + Duration::hours(1)))
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&local)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// 22:00–07:00, windows starting Friday only.
    fn friday_nights(allow: Level) -> QuietHours {
        QuietHours {
            start: parse_time("22:00").unwrap(),
            end: parse_time("07:00").unwrap(),
            days: days_mask(&[5]).unwrap(),
            allow,
        }
    }

    #[test]
    fn times_and_days_parse_strictly() {
        assert_eq!(parse_time("00:00"), Some(0));
        assert_eq!(parse_time("23:59"), Some(1439));
        for bad in [
            "24:00", "7:00", "07:60", "0700", "07:0", "ab:cd", "07:00:00", "",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
        assert_eq!(format_time(7 * 60 + 5), "07:05");
        assert_eq!(days_mask(&[0, 6]), Some(0b100_0001));
        assert_eq!(days_mask(&[7]), None);
        assert!(parse_tz("America/Los_Angeles").is_some());
        assert!(parse_tz("Mars/Olympus").is_none());
    }

    #[test]
    fn a_window_wraps_past_midnight_into_the_next_day() {
        let q = friday_nights(Level::Critical);
        let la: Tz = "America/Los_Angeles".parse().unwrap();
        // 2026-10-16 is a Friday. LA is UTC-7 in October.
        assert!(!q.active_at(at("2026-10-17T04:59:00Z"), la), "Fri 21:59");
        assert!(q.active_at(at("2026-10-17T05:00:00Z"), la), "Fri 22:00");
        assert!(q.active_at(at("2026-10-17T13:59:00Z"), la), "Sat 06:59");
        assert!(!q.active_at(at("2026-10-17T14:00:00Z"), la), "Sat 07:00");
        assert!(
            !q.active_at(at("2026-10-18T05:30:00Z"), la),
            "Sat 22:30: Saturday starts no window"
        );
        assert!(
            !q.active_at(at("2026-10-16T13:00:00Z"), la),
            "Fri 06:00: Thursday started none"
        );
        assert_eq!(
            q.ends_after(at("2026-10-17T05:00:00Z"), la),
            Some(at("2026-10-17T14:00:00Z")),
            "the evening half ends tomorrow at 07:00 local"
        );
        assert_eq!(
            q.ends_after(at("2026-10-17T13:00:00Z"), la),
            Some(at("2026-10-17T14:00:00Z")),
            "the morning half ends today"
        );
        assert_eq!(q.ends_after(at("2026-10-17T15:00:00Z"), la), None);
    }

    #[test]
    fn the_same_instant_is_quiet_in_one_timezone_and_not_another() {
        let q = friday_nights(Level::Critical);
        let instant = at("2026-10-17T05:30:00Z");
        let la: Tz = "America/Los_Angeles".parse().unwrap();
        let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
        assert!(q.active_at(instant, la), "Fri 22:30 in LA");
        assert!(!q.active_at(instant, tokyo), "Sat 14:30 in Tokyo");
        // Tokyo's own Friday night is a different instant.
        assert!(
            q.active_at(at("2026-10-16T13:30:00Z"), tokyo),
            "Fri 22:30 in Tokyo"
        );
    }

    #[test]
    fn a_daytime_window_is_inside_one_day_and_lets_through_from_allow() {
        let q = QuietHours {
            start: parse_time("09:00").unwrap(),
            end: parse_time("17:00").unwrap(),
            days: days_mask(&[1, 2, 3, 4, 5]).unwrap(),
            allow: Level::TimeSensitive,
        };
        let utc: Tz = "UTC".parse().unwrap();
        assert!(q.active_at(at("2026-10-19T09:00:00Z"), utc), "Mon 09:00");
        assert!(!q.active_at(at("2026-10-19T17:00:00Z"), utc), "Mon 17:00");
        assert!(!q.active_at(at("2026-10-18T12:00:00Z"), utc), "Sun noon");
        assert!(q.lets_through(Level::TimeSensitive) && q.lets_through(Level::Critical));
        assert!(!q.lets_through(Level::Active));
        assert_eq!(
            serde_json::to_value(q.view()).unwrap(),
            serde_json::json!({ "start": "09:00", "end": "17:00", "days": [1, 2, 3, 4, 5],
                                "allow": "time_sensitive" })
        );
    }

    #[test]
    fn an_end_in_a_repeated_hour_is_the_next_one_not_one_already_past() {
        // Europe/London repeats 01:00–02:00 on 2026-10-25: 01:30 is 00:30Z
        // (BST) and again 01:30Z (GMT). A window from Saturday 22:00 to 01:30.
        let q = QuietHours {
            start: parse_time("22:00").unwrap(),
            end: parse_time("01:30").unwrap(),
            days: days_mask(&[6]).unwrap(),
            allow: Level::Critical,
        };
        let london: Tz = "Europe/London".parse().unwrap();
        assert_eq!(
            q.ends_after(at("2026-10-25T00:15:00Z"), london),
            Some(at("2026-10-25T00:30:00Z")),
            "the first pass through 01:15 ends at the first 01:30"
        );
        let second_pass = at("2026-10-25T01:15:00Z");
        assert!(q.active_at(second_pass, london), "01:15 again, GMT");
        let end = q.ends_after(second_pass, london).unwrap();
        assert_eq!(
            end,
            at("2026-10-25T01:30:00Z"),
            "the second 01:30, not the past one"
        );
        assert!(end > second_pass);
        // America/New_York repeats 01:00–02:00 on 2026-11-01.
        let new_york: Tz = "America/New_York".parse().unwrap();
        assert_eq!(
            q.ends_after(at("2026-11-01T06:15:00Z"), new_york),
            Some(at("2026-11-01T06:30:00Z")),
            "01:15 EST ends at 01:30 EST"
        );
    }

    #[test]
    fn an_end_in_a_skipped_hour_lands_an_hour_later() {
        // Europe/Stockholm skips 02:00–03:00 on 2027-03-28.
        let q = QuietHours {
            start: parse_time("22:00").unwrap(),
            end: parse_time("02:30").unwrap(),
            days: days_mask(&[6]).unwrap(),
            allow: Level::Critical,
        };
        let sthlm: Tz = "Europe/Stockholm".parse().unwrap();
        assert_eq!(
            q.ends_after(at("2027-03-27T22:00:00Z"), sthlm),
            Some(at("2027-03-28T01:30:00Z")),
            "03:30 CEST"
        );
    }
}
