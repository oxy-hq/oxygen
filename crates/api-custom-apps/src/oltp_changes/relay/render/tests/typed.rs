//! The round trip's rows: one value of every type, the same row all null,
//! and the values that are not plain text, as Postgres writes their images.

use serde_json::Value;
use serde_json::value::RawValue;

use super::{INVOCATION, row, text};
use crate::oltp_changes::relay::render::ShipRow;

/// A `float8` as Postgres's `to_jsonb` writes it: tiny, and huge. Bare in one
/// VALUES column, or one list, DuckDB would type them as a `DECIMAL` wider
/// than 38 digits and refuse the write.
const TINY: &str = "0.000000000000000000015";
const HUGE: &str = "300000000000000000000";
const PAIR: &str = "[0.000000000000000000015, 300000000000000000000]";

/// Per attribute: its number, name and Postgres type, its value in the full
/// row's image ([`image`]), and that value read back as DuckDB text. One per
/// `DuckType`, and [`TINY`] beside [`HUGE`] in a scalar and a list column;
/// `TIMESTAMPTZ` ([`TZ`]) is read back as an instant instead.
pub(super) const TYPED: [(i16, &str, &str, &str, &str); 16] = [
    (1, "s", "smallint", "7", "7"),
    (2, "i", "integer", "-42", "-42"),
    (3, "b", "bigint", "9007199254740993", "9007199254740993"),
    (4, "r", "real", "1.5", "1.5"),
    (5, "d", "double precision", "0.25", "0.25"),
    (6, "n", "numeric(10,2)", "1234.5", "1234.50"),
    (7, "ok", "boolean", "true", "true"),
    (8, "t", "text", "it's", "it's"),
    (9, "u", "uuid", INVOCATION, INVOCATION),
    (10, "dt", "date", "2026-11-24", "2026-11-24"),
    (
        11,
        "ts",
        "timestamp(3) without time zone",
        "2026-11-24T10:00:00.123",
        "2026-11-24 10:00:00.123",
    ),
    (12, "tm", "time without time zone", "10:00:01", "10:00:01"),
    (13, "arr", "integer[]", "[1, null, 3]", "[1, NULL, 3]"),
    (
        14,
        "doc",
        "jsonb",
        r#"{"k": [1, "x"]}"#,
        r#"{"k": [1, "x"]}"#,
    ),
    (15, "x", "double precision", TINY, "1.5e-20"),
    (16, "xs", "double precision[]", PAIR, "[1.5e-20, 3e+20]"),
];

/// Attribute 17, `timestamp with time zone`.
pub(super) const TZ: &str = "2026-11-24T10:00:00.123456+05:30";

/// The values that are not plain text, each in the third row's image and as
/// DuckDB reads it back: the floats' non-numbers, a BC date and timestamp,
/// a jsonb string scalar, which keeps its quotes, and [`HUGE`].
const SPECIAL: [(&str, &str, &str); 6] = [
    ("r", "NaN", "nan"),
    ("d", "-Infinity", "-inf"),
    ("dt", "0044-03-15 BC", "0044-03-15 (BC)"),
    ("ts", "0044-03-15T10:00:00 BC", "0044-03-15 (BC) 10:00:00"),
    ("doc", r#""abc""#, r#""abc""#),
    ("x", HUGE, "3e+20"),
];

/// A row image as Postgres writes it: each value its own lexeme when it is
/// valid JSON, else a JSON string.
fn image(values: &[(&str, &str)]) -> String {
    let lexeme = |v: &str| match serde_json::from_str::<&RawValue>(v) {
        Ok(_) => v.to_string(),
        Err(_) => Value::from(v).to_string(),
    };
    let fields: Vec<String> = values
        .iter()
        .map(|(c, v)| format!("{}: {}", Value::from(*c), lexeme(*v)))
        .collect();
    format!("{{{}}}", fields.join(", "))
}

/// A signed row with every value, one with every value null, and one with
/// [`SPECIAL`] only.
pub(super) fn typed_rows() -> [ShipRow; 3] {
    let mut full: Vec<(&str, &str)> = TYPED.iter().map(|(_, c, _, v, _)| (*c, *v)).collect();
    full.push(("tz", TZ));
    let mut nulls: Vec<(&str, &str)> = TYPED.iter().map(|(_, c, ..)| (*c, "null")).collect();
    nulls.push(("tz", "null"));
    let special: Vec<(&str, &str)> = SPECIAL.iter().map(|(c, v, _)| (*c, *v)).collect();
    let mut signed = row(1, "insert", 1, Some(image(&full).as_str()));
    signed.invocation = Some(INVOCATION.parse().expect("a uuid"));
    [
        signed,
        row(2, "update", 1, Some(image(&nulls).as_str())),
        row(3, "delete", 1, Some(image(&special).as_str())),
    ]
}

/// [`typed_rows`] as they read back: every [`TYPED`] column as text, then
/// `_op`, `_actor`, `_invocation_id` and `_txid`.
pub(super) fn typed_texts() -> Vec<Vec<Option<String>>> {
    let special = |c: &str| {
        SPECIAL
            .iter()
            .find(|(s, ..)| *s == c)
            .map(|(.., v)| v.to_string())
    };
    let full = TYPED.iter().map(|(.., v)| text(v)).collect();
    let specials = TYPED.iter().map(|(_, c, ..)| special(*c)).collect();
    let rows: [(Vec<_>, &str, Option<&str>); 3] = [
        (full, "insert", Some(INVOCATION)),
        (vec![None; TYPED.len()], "update", None),
        (specials, "delete", None),
    ];
    let txids = 901..;
    rows.into_iter()
        .zip(txids)
        .map(|((mut values, op, invocation), txid)| {
            let invocation = invocation.map(str::to_string);
            values.extend([text(op), text("alice"), invocation, Some(txid.to_string())]);
            values
        })
        .collect()
}
