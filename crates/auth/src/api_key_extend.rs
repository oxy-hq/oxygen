//! Extend: push out a key's expiry without changing the key (design §3.6).
//!
//! The pure half — parsing the request body and computing the new expiry — so
//! it is testable without a database. The write is
//! [`crate::api_key_domain::ApiKeyService::extend_api_key`].

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

/// The longest single extension by `days`: ten years.
pub const MAX_EXTEND_DAYS: i64 = 3650;

/// What an extension asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendTo {
    /// `max(now, current expiry) + days`. A key that never expires counts
    /// from now: the caller asked for a date, and gets one.
    Days(i64),
    /// An explicit instant, which must be in the future.
    At(DateTime<Utc>),
    /// `{"expires_at": null}`: never expires.
    Never,
}

impl ExtendTo {
    /// Parse the body: exactly one of `{"days": n}`, `{"expires_at": "<rfc3339>"}`
    /// or `{"expires_at": null}`. Anything else is a 400, with the reason.
    pub fn from_json(body: &Value) -> Result<Self, String> {
        let obj = body
            .as_object()
            .ok_or_else(|| "body must be a JSON object".to_string())?;
        if let Some(unknown) = obj.keys().find(|k| *k != "days" && *k != "expires_at") {
            return Err(format!("unknown field '{unknown}'"));
        }
        match (obj.get("days"), obj.get("expires_at")) {
            (Some(_), Some(_)) => Err("send exactly one of 'days' or 'expires_at'".into()),
            (None, None) => Err("send one of 'days' or 'expires_at'".into()),
            (Some(days), None) => days
                .as_i64()
                .filter(|d| (1..=MAX_EXTEND_DAYS).contains(d))
                .map(Self::Days)
                .ok_or_else(|| format!("'days' must be an integer from 1 to {MAX_EXTEND_DAYS}")),
            (None, Some(Value::Null)) => Ok(Self::Never),
            (None, Some(Value::String(s))) => DateTime::parse_from_rfc3339(s)
                .map(|t| Self::At(t.with_timezone(&Utc)))
                .map_err(|_| "'expires_at' must be an RFC 3339 timestamp or null".to_string()),
            (None, Some(_)) => Err("'expires_at' must be an RFC 3339 timestamp or null".into()),
        }
    }

    /// The new expiry, given the current one. `Err` when the result would not
    /// be in the future.
    pub fn resolve(
        self,
        current: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, String> {
        match self {
            Self::Days(days) => {
                let base = current.map_or(now, |c| c.max(now));
                Ok(Some(base + Duration::days(days)))
            }
            Self::At(at) if at <= now => Err("'expires_at' must be in the future".into()),
            Self::At(at) => Ok(Some(at)),
            Self::Never => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_each_documented_body() {
        assert_eq!(
            ExtendTo::from_json(&json!({"days": 30})),
            Ok(ExtendTo::Days(30))
        );
        assert_eq!(
            ExtendTo::from_json(&json!({"expires_at": null})),
            Ok(ExtendTo::Never)
        );
        let at = ExtendTo::from_json(&json!({"expires_at": "2027-01-31T00:00:00Z"})).unwrap();
        assert_eq!(
            at,
            ExtendTo::At("2027-01-31T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
    }

    #[test]
    fn refuses_everything_else() {
        for body in [
            json!({}),
            json!([]),
            json!("30"),
            json!({"days": 30, "expires_at": null}),
            json!({"days": 0}),
            json!({"days": 3651}),
            json!({"days": -1}),
            json!({"days": 1.5}),
            json!({"days": "30"}),
            json!({"expires_at": "tomorrow"}),
            json!({"expires_at": 1_800_000_000}),
            json!({"days": 30, "note": "x"}),
        ] {
            assert!(ExtendTo::from_json(&body).is_err(), "{body}");
        }
    }

    #[test]
    fn days_count_from_the_later_of_now_and_the_current_expiry() {
        let now = Utc::now();
        let later = now + Duration::days(10);
        let earlier = now - Duration::days(10);
        let d = ExtendTo::Days(30);
        assert_eq!(
            d.resolve(Some(later), now),
            Ok(Some(later + Duration::days(30)))
        );
        // An expired key is revived from now, not from its lapsed expiry.
        assert_eq!(
            d.resolve(Some(earlier), now),
            Ok(Some(now + Duration::days(30)))
        );
        assert_eq!(d.resolve(None, now), Ok(Some(now + Duration::days(30))));
    }

    #[test]
    fn an_explicit_date_must_be_in_the_future() {
        let now = Utc::now();
        assert!(ExtendTo::At(now).resolve(None, now).is_err());
        assert!(
            ExtendTo::At(now - Duration::seconds(1))
                .resolve(None, now)
                .is_err()
        );
        let at = now + Duration::days(1);
        assert_eq!(ExtendTo::At(at).resolve(None, now), Ok(Some(at)));
    }

    #[test]
    fn never_clears_the_expiry() {
        let now = Utc::now();
        assert_eq!(ExtendTo::Never.resolve(Some(now), now), Ok(None));
    }
}
