//! App events: the pure building blocks every later layer shares. An app
//! publishes a typed event, and functions in other apps of the same org and
//! environment receive it through the Postgres task queue.
//!
//! This module holds identity and shape only, with no database and no host
//! ops:
//!
//! - [`event_id`]: the deterministic id, a UUIDv5 of (org, environment, type,
//!   key). A repeated publish gets the same id, so a causation chain always
//!   points at the one real event.
//! - [`OxyEvent`]: the envelope every listener receives (CloudEvents attribute
//!   names, plus Oxy's own fields as lowercase flat extensions).
//! - The limits a publish is held to.
//! - [`catalog`]: parsing the manifest's app-level `events` catalogue and a
//!   function's `events.publish` and `on` keys.

pub mod catalog;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The namespace of every event id (the UUIDv5 namespace of [`event_id`]).
///
/// This value must never change: ids are derived from it, and the dedupe of a
/// repeated publish (and every stored causation id) depends on the same inputs
/// producing the same id forever.
pub const APP_EVENT_ID_NAMESPACE: Uuid = Uuid::from_u128(0x6f0c_5b1e_2d47_4a93_9e18_c4a7_b35d_f812);

/// A payload's size ceiling, in bytes of its `jsonb` text ([`jsonb_text_len`]):
/// the host measures a publish that way, and the store holds it to the same
/// length (`octet_length(data::text)`) twice — the definer function refuses
/// a longer one, and the relay reads that length without reading the body.
pub const MAX_PAYLOAD_BYTES: usize = 65_536;

/// The length of `value` as Postgres writes a `jsonb` as text — the length
/// the definer function and the relay read in the org's store — computed
/// without sending it:
///
/// - JSON with a space after each `:` and each `,` that separates members or
///   elements (`{"a": 1, "b": [1, 2]}`);
/// - strings escaped as Postgres escapes them (the same set `serde_json`
///   escapes);
/// - each number as Postgres writes the `numeric` it keeps it as: every
///   digit, with no exponent, the literal's own scale, and no negative zero
///   (`1e+300` is 301 bytes, `1.5e-7` is `0.00000015`, `-0.0` is `0.0`).
///
/// Exact for every value `serde_json` can hold: the `app_events_outbox`
/// integration suite pins it against Postgres's own
/// `octet_length(($1::jsonb)::text)`.
pub fn jsonb_text_len(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    /// The brackets, `, ` between each two of `n` parts, and the parts:
    /// saturating, so an unbounded part keeps the whole unbounded.
    fn enclosed(n: usize, parts: impl Iterator<Item = usize>) -> usize {
        parts.fold(
            2usize.saturating_add(2 * n.saturating_sub(1)),
            usize::saturating_add,
        )
    }
    match value {
        Value::Null | Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        // The literal `serde_json` sends, as Postgres writes it back.
        Value::Number(n) => numeric_text_len(&n.to_string()),
        Value::String(s) => quoted_len(s),
        Value::Array(items) => enclosed(items.len(), items.iter().map(jsonb_text_len)),
        Value::Object(members) => enclosed(
            members.len(),
            members.iter().map(|(key, value)| {
                quoted_len(key)
                    .saturating_add(2)
                    .saturating_add(jsonb_text_len(value))
            }),
        ),
    }
}

/// `s` as a JSON string literal, quotes and escapes included, as `serde_json`
/// writes it — and as Postgres does, which escapes the same characters
/// (`"`, `\`, and each control character, as `\n` or `\u001f`).
fn quoted_len(s: &str) -> usize {
    /// Counts what is written, keeping none of it.
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // Writing a string to a counter cannot fail.
    let _ = serde_json::to_writer(&mut count, s);
    count.0
}

/// The length of the JSON number `literal` as Postgres writes it once it is
/// stored, as `numeric` (`numeric_in`, then `numeric_out`): the literal's
/// scale is its digits after the point less its exponent (never below 0),
/// the whole part is written out to every digit (`0` when it has none), and
/// zero carries no sign.
fn numeric_text_len(literal: &str) -> usize {
    let (negative, unsigned) = match literal.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, literal),
    };
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        // `+300` parses, and every literal `serde_json` writes does. One that
        // does not is unbounded: never measured under what the store writes.
        Some((mantissa, exponent)) => match exponent.parse::<i64>() {
            Ok(exponent) => (mantissa, exponent),
            Err(_) => return usize::MAX,
        },
        None => (unsigned, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let scale = (fraction.len() as i64).saturating_sub(exponent).max(0) as usize;
    let point_and_scale = if scale > 0 {
        scale.saturating_add(1)
    } else {
        0
    };
    let digits = whole.len() + fraction.len();
    let leading_zeros = whole
        .bytes()
        .chain(fraction.bytes())
        .take_while(|&d| d == b'0')
        .count();
    if leading_zeros == digits {
        // Zero: `0`, at its scale, never `-0`.
        return point_and_scale.saturating_add(1);
    }
    // Digits before the point once the exponent moves it, from the first
    // that is not a zero.
    let whole_digits = (whole.len() as i64)
        .saturating_add(exponent)
        .saturating_sub(leading_zeros as i64)
        .max(1) as usize;
    usize::from(negative)
        .saturating_add(whole_digits)
        .saturating_add(point_and_scale)
}
/// The deepest causation chain: an event no handler published is depth 0.
pub const MAX_DEPTH: i32 = 8;
/// The longest `delaySeconds` a publish may ask for (24 hours).
pub const MAX_DELAY_SECS: u64 = 86_400;
/// The longest publisher-chosen `key`, in characters (Unicode scalar values,
/// not bytes: a key of 200 multi-byte characters is accepted).
pub const MAX_KEY_LEN: usize = 200;
/// How long events and their delivery records are kept; also the dedupe window.
pub const RETENTION_DAYS: i64 = 30;

/// The platform id of an event: UUIDv5 of `"{org_id}/{environment}/{event_type}/{key}"`
/// under [`APP_EVENT_ID_NAMESPACE`]. `event_type` is the qualified
/// `<publisher-slug>/<name>`.
pub fn event_id(org_id: Uuid, environment: &str, event_type: &str, key: &str) -> Uuid {
    let name = format!("{org_id}/{environment}/{event_type}/{key}");
    Uuid::new_v5(&APP_EVENT_ID_NAMESPACE, name.as_bytes())
}

/// An event type as listeners name it: `<publisher-slug>/<name>`.
pub fn qualified_type(publisher_slug: &str, name: &str) -> String {
    format!("{publisher_slug}/{name}")
}

/// Whether `name` is a valid event name, `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$`:
/// lowercase, starting with a letter, dot-separated non-empty segments of
/// letters and digits (`statement.uploaded`, `statement.uploaded.v2`).
pub fn valid_event_name(name: &str) -> bool {
    let mut segs = name.split('.');
    let alnum = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    let first_ok = segs
        .next()
        .is_some_and(|f| f.bytes().next().is_some_and(|b| b.is_ascii_lowercase()) && alnum(f));
    first_ok && segs.all(alnum)
}

/// The envelope every event reaches a listener in. Field names are the
/// CloudEvents attribute names where one exists; Oxy's extensions are
/// lowercase and flat, as CloudEvents requires of extension attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OxyEvent {
    /// Always `"1.0"` (the CloudEvents spec version).
    pub specversion: String,
    /// The platform event id ([`event_id`]).
    pub id: Uuid,
    /// The qualified type, `locations/statement.uploaded`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// The publisher, `/apps/locations`.
    pub source: String,
    /// When the publish happened (RFC 3339).
    pub time: DateTime<Utc>,
    /// The schema revision the payload was validated against.
    pub dataschema: String,
    /// The payload.
    pub data: serde_json::Value,
    /// Publisher-chosen and stable: the dedupe key.
    pub key: String,
    /// `production`, `staging`, `dev-x`.
    pub environment: String,
    /// The publishing invocation.
    pub invocation: Option<Uuid>,
    /// The person whose action started the chain. Attribution only.
    pub actor: Option<String>,
    /// The event whose handler published this one.
    pub causationid: Option<Uuid>,
    /// The first event or invocation of the chain.
    pub rootid: Uuid,
    /// 0 for an event no handler published.
    pub depth: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> OxyEvent {
        OxyEvent {
            specversion: "1.0".into(),
            id: Uuid::nil(),
            event_type: "locations/statement.uploaded".into(),
            source: "/apps/locations".into(),
            time: DateTime::<Utc>::UNIX_EPOCH,
            dataschema: "locations/statement.uploaded@1".into(),
            data: serde_json::json!({ "statementId": 1 }),
            key: "statement:1".into(),
            environment: "production".into(),
            invocation: None,
            actor: None,
            causationid: None,
            rootid: Uuid::nil(),
            depth: 0,
        }
    }

    #[test]
    fn event_id_is_stable_and_scoped() {
        let org = Uuid::nil();
        let a = event_id(
            org,
            "production",
            "locations/statement.uploaded",
            "statement:1",
        );
        assert_eq!(
            a,
            event_id(
                org,
                "production",
                "locations/statement.uploaded",
                "statement:1"
            )
        );
        assert_ne!(
            a,
            event_id(
                org,
                "staging",
                "locations/statement.uploaded",
                "statement:1"
            )
        );
        assert_ne!(
            a,
            event_id(
                org,
                "production",
                "locations/statement.uploaded",
                "statement:2"
            )
        );
    }

    #[test]
    fn the_envelope_uses_cloudevents_names() {
        let json = serde_json::to_value(sample_event()).unwrap();
        assert_eq!(json["specversion"], "1.0");
        assert!(
            json.get("type").is_some()
                && json.get("causationid").is_some()
                && json.get("rootid").is_some()
        );
        assert!(json.get("causation").is_none());
        let back: OxyEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, sample_event());
    }

    /// What `SELECT octet_length('<json>'::jsonb::text)` returns for each, by
    /// Postgres's own rendering of it.
    #[test]
    fn a_payload_measures_as_postgres_writes_jsonb() {
        for (value, as_postgres_writes_it) in [
            (serde_json::json!({}), "{}"),
            (serde_json::json!([]), "[]"),
            (serde_json::json!({ "a": 1 }), r#"{"a": 1}"#),
            (
                serde_json::json!({ "a": 1, "b": [1, 2, { "c": null }] }),
                r#"{"a": 1, "b": [1, 2, {"c": null}]}"#,
            ),
            (serde_json::json!("é\n\"x\""), r#""é\n\"x\"""#),
            (
                serde_json::json!([[], {}, [true, false]]),
                "[[], {}, [true, false]]",
            ),
        ] {
            assert_eq!(
                jsonb_text_len(&value),
                as_postgres_writes_it.len(),
                "{value} as {as_postgres_writes_it}"
            );
        }
        // Never shorter than compact JSON.
        let v = serde_json::json!({ "k": [1, 2, 3], "s": "x" });
        assert!(jsonb_text_len(&v) > serde_json::to_vec(&v).unwrap().len());
    }

    /// R32: `jsonb` keeps a number as `numeric`, which Postgres writes with
    /// every digit — no exponent, the literal's own scale, never a negative
    /// zero — so `1e300` (5 bytes as `serde_json` sends it) is 301 bytes in
    /// the store. What `SELECT ($1::jsonb)::text` returns for each, given
    /// the value as `serde_json` writes it; the `app_events_outbox`
    /// integration suite asks Postgres itself.
    #[test]
    fn numbers_measure_as_postgres_writes_numeric() {
        let zeros = |n: usize| "0".repeat(n);
        let parsed = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        for (value, as_postgres_writes_it) in [
            (serde_json::json!(1e300), format!("1{}", zeros(300))),
            (serde_json::json!(-1e-7), "-0.0000001".to_string()),
            (serde_json::json!(1.5e-7), "0.00000015".to_string()),
            (serde_json::json!(5e-324), format!("0.{}5", zeros(323))),
            (
                serde_json::json!(f64::MAX),
                format!("17976931348623157{}", zeros(292)),
            ),
            (serde_json::json!(1.0), "1.0".to_string()),
            (serde_json::json!(-0.0), "0.0".to_string()),
            (
                parsed("123456789012345680000"),
                "123456789012345680000".to_string(),
            ),
            (serde_json::json!(1e16), "10000000000000000".to_string()),
            (serde_json::json!(1e15), "1000000000000000.0".to_string()),
            (serde_json::json!(1e-6), "0.000001".to_string()),
            (serde_json::json!(0.1), "0.1".to_string()),
            (serde_json::json!(u64::MAX), u64::MAX.to_string()),
            (serde_json::json!(i64::MIN), i64::MIN.to_string()),
            (serde_json::json!(0), "0".to_string()),
            (
                serde_json::json!({ "xs": [1e300, -1e-7] }),
                format!(r#"{{"xs": [1{}, -0.0000001]}}"#, zeros(300)),
            ),
        ] {
            assert_eq!(
                jsonb_text_len(&value),
                as_postgres_writes_it.len(),
                "{value} as {as_postgres_writes_it}"
            );
        }
    }

    /// R40: an exponent too large to parse measures as `usize::MAX`, never
    /// 0: the host's measure is never under the store's. `serde_json` here
    /// cannot even hold such a number — no `arbitrary_precision`, so `1e400`
    /// is refused at parse — which this pins: with that feature on, numbers
    /// would keep their literal and this path would matter.
    #[test]
    fn an_exponent_past_parsing_measures_as_unbounded() {
        assert!(serde_json::from_str::<serde_json::Value>("1e400").is_err());
        assert_eq!(numeric_text_len("1e400"), 401);
        assert_eq!(numeric_text_len("1e99999999999999999999"), usize::MAX);
        assert_eq!(numeric_text_len("-1.5e-99999999999999999999"), usize::MAX);
        assert!(numeric_text_len("1e9223372036854775807") >= i64::MAX as usize);
    }

    #[test]
    fn names_and_qualified_types() {
        assert!(valid_event_name("statement.uploaded.v2"));
        assert!(!valid_event_name("Statement.Uploaded"));
        assert!(valid_event_name("report.ingested"));
        assert!(valid_event_name("a"));
        for bad in ["a-b", "1a", "a.", "a..b", ".a", "A", "a.B", ""] {
            assert!(!valid_event_name(bad), "{bad}");
        }
        assert_eq!(qualified_type("locations", "x.y"), "locations/x.y");
    }
}
