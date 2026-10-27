//! A switch moment: when a lever flips, in `design.timezone`. A day, week or
//! month design switches at a period's first midnight, so its moments are
//! dates; an hour-grain design may switch at any whole hour (`2026-10-26T14:00`).
//!
//! It is written as a bare `YYYY-MM-DD` when it falls on midnight, so every
//! design that names only dates serializes, and hashes, as it always has.

use std::fmt;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Moment(pub NaiveDateTime);

impl Moment {
    pub fn midnight(d: NaiveDate) -> Self {
        Moment(d.and_time(NaiveTime::MIN))
    }

    pub fn at(self) -> NaiveDateTime {
        self.0
    }

    pub fn date(self) -> NaiveDate {
        self.0.date()
    }

    pub fn is_midnight(self) -> bool {
        self.0.time() == NaiveTime::MIN
    }

    /// Whether it is a whole hour (no minutes or seconds).
    pub fn is_whole_hour(self) -> bool {
        self.0.minute() == 0 && self.0.second() == 0 && self.0.nanosecond() == 0
    }

    /// The first midnight at or after it: the date a day-counting read starts on.
    pub fn ceil_date(self) -> NaiveDate {
        if self.is_midnight() {
            self.date()
        } else {
            self.date().succ_opt().unwrap_or(self.date())
        }
    }

    /// `n` hours later, `None` past chrono's calendar.
    pub fn plus_hours(self, n: i64) -> Option<Self> {
        self.0
            .checked_add_signed(chrono::Duration::hours(n))
            .map(Moment)
    }

    pub fn parse(s: &str) -> Option<Self> {
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Some(Moment::midnight(d));
        }
        [
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%d %H:%M:%S",
        ]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
        .map(Moment)
    }
}

impl std::str::FromStr for Moment {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Moment::parse(s).ok_or_else(|| format!("`{s}` is not a date or a local date and time"))
    }
}

/// Whole days later, at the same time of day.
impl std::ops::Add<chrono::Days> for Moment {
    type Output = Moment;
    fn add(self, d: chrono::Days) -> Moment {
        Moment(self.0 + d)
    }
}

impl std::ops::Sub<chrono::Days> for Moment {
    type Output = Moment;
    fn sub(self, d: chrono::Days) -> Moment {
        Moment(self.0 - d)
    }
}

impl From<NaiveDate> for Moment {
    fn from(d: NaiveDate) -> Self {
        Moment::midnight(d)
    }
}

/// A date stands for its midnight, so a midnight moment equals its date.
impl PartialEq<NaiveDate> for Moment {
    fn eq(&self, other: &NaiveDate) -> bool {
        *self == Moment::midnight(*other)
    }
}

impl fmt::Display for Moment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_midnight() {
            write!(f, "{}", self.0.format("%Y-%m-%d"))
        } else {
            write!(f, "{}", self.0.format("%Y-%m-%dT%H:%M"))
        }
    }
}

impl Serialize for Moment {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Moment {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Moment::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{s}` is not a date (YYYY-MM-DD) or a local date and time (YYYY-MM-DDTHH:MM)"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_its_midnight_and_writes_back_as_a_date() {
        let m: Moment = serde_json::from_value(serde_json::json!("2026-10-26")).unwrap();
        assert!(m.is_midnight());
        assert_eq!(m, NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!(serde_json::to_value(m).unwrap(), "2026-10-26");
    }

    #[test]
    fn a_local_time_parses_and_writes_back_to_the_minute() {
        let m: Moment = serde_json::from_value(serde_json::json!("2026-10-26T14:00")).unwrap();
        assert_eq!(m.to_string(), "2026-10-26T14:00");
        assert_eq!(
            m.ceil_date(),
            NaiveDate::from_ymd_opt(2026, 10, 27).unwrap()
        );
        assert_ne!(m, NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        let bad = serde_json::from_value::<Moment>(serde_json::json!("26/10/2026"));
        assert!(bad.unwrap_err().to_string().contains("YYYY-MM-DDTHH:MM"));
    }
}
