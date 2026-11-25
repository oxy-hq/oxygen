//! A row image's JSON value as a DuckDB literal of its column's type
//! ([`literal`]), rendered from the value's JSON lexeme, never through an
//! `f64`.

use std::borrow::Cow;

use oxy_cameras::airhouse::escape::sql_str;
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::oltp_changes::relay::table::DuckType;

/// `CAST(<value> AS <ty>)`.
pub(super) fn cast(value: &str, ty: &DuckType) -> String {
    format!("CAST({value} AS {})", ty.sql())
}

/// `v`, a row image's JSON value, as a literal of type `ty`; `Err` says why it
/// cannot be one. A literal is one DuckDB accepts, so a write never fails on
/// a value: one that would is refused here, and holds only its table.
///
/// An `Err` names the kind of value and the type it missed, never the value:
/// it becomes a hold's reason, which is stored and logged, and a row's values
/// stay out of both (design §7).
///
/// Rendered from `v`'s JSON lexeme, never through an `f64`:
///
/// - JSON null is `CAST(NULL AS T)`, so a column that is null in every row of
///   a batch still has its type.
/// - `JSON` is the value's exact JSON text, a string scalar with its quotes.
/// - `VARCHAR` is a JSON string's content, or any other value's JSON text.
/// - A number is `CAST(<lexeme> AS T)`, the lexeme verbatim, and must fit an
///   integer column. The strings `NaN`, `Infinity` and `-Infinity` are cast
///   to `FLOAT` or `DOUBLE`; `DECIMAL` has none of them.
/// - Other types take a JSON string, as `CAST('…' AS T)`. A Postgres BC date
///   or timestamp (`0044-03-15 BC`) is rewritten to DuckDB's spelling
///   (`0044-03-15 (BC)`), which DuckDB would otherwise refuse, or read as AD.
/// - A list takes a JSON array, `CAST([…] AS T)`, each element rendered for
///   the list's type.
pub fn literal(v: &RawValue, ty: &DuckType) -> Result<String, String> {
    let raw = v.get().trim();
    match (Lexeme::of(raw), ty) {
        (Lexeme::Null, _) => Ok(cast("NULL", ty)),
        (_, DuckType::Json) => Ok(cast(&sql_str(raw), ty)),
        (Lexeme::String, DuckType::Varchar) => Ok(cast(&sql_str(&json_string(raw)?), ty)),
        (_, DuckType::Varchar) => Ok(cast(&sql_str(raw), ty)),
        (Lexeme::Bool, DuckType::Boolean) => Ok(raw.to_string()),
        (Lexeme::Number, _) => number(raw, ty),
        (Lexeme::String, _) => string(&json_string(raw)?, ty),
        (Lexeme::Array, DuckType::List(element)) => list(raw, element, ty),
        (lexeme, _) => Err(format!("{} where {} is expected", lexeme.kind(), ty.sql())),
    }
}

/// What a JSON value is, from the first byte of its lexeme (which a
/// [`RawValue`] guarantees is valid JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lexeme {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl Lexeme {
    fn of(raw: &str) -> Lexeme {
        match raw.as_bytes().first() {
            Some(b'n') => Lexeme::Null,
            Some(b't' | b'f') => Lexeme::Bool,
            Some(b'"') => Lexeme::String,
            Some(b'[') => Lexeme::Array,
            Some(b'{') => Lexeme::Object,
            _ => Lexeme::Number,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Lexeme::Null => "null",
            Lexeme::Bool => "a boolean",
            Lexeme::Number => "a number",
            Lexeme::String => "a string",
            Lexeme::Array => "an array",
            Lexeme::Object => "an object",
        }
    }
}

/// A JSON string lexeme's content, unescaped.
fn json_string(raw: &str) -> Result<String, String> {
    serde_json::from_str(raw).map_err(|e| unparsed("a string", &e))
}

/// Why a JSON value did not parse, by where and how, never what: a
/// `serde_json` error can quote the value it met.
fn unparsed(kind: &str, e: &serde_json::Error) -> String {
    format!(
        "{kind} that does not parse ({:?} error at column {})",
        e.classify(),
        e.column()
    )
}

/// A JSON number, its lexeme verbatim (a `numeric` keeps every digit), cast
/// to the column's type. Bare, DuckDB would give a VALUES column, or a list,
/// one type across its rows: `0.000000000000000000015` beside
/// `300000000000000000000` makes that a `DECIMAL` wider than 38 digits, and
/// the write fails. Cast, a column's type never depends on the batch.
fn number(raw: &str, ty: &DuckType) -> Result<String, String> {
    let fits = match ty {
        DuckType::SmallInt => raw.parse::<i16>().is_ok(),
        DuckType::Integer => raw.parse::<i32>().is_ok(),
        DuckType::BigInt => raw.parse::<i64>().is_ok(),
        DuckType::Float | DuckType::Double | DuckType::Decimal(..) => true,
        _ => return Err(format!("a number where {} is expected", ty.sql())),
    };
    if fits {
        Ok(cast(raw, ty))
    } else {
        Err(format!("a number out of range for {}", ty.sql()))
    }
}

/// A JSON array as a `LIST` literal, each element rendered for `element`.
fn list(raw: &str, element: &DuckType, ty: &DuckType) -> Result<String, String> {
    let items: Vec<Box<RawValue>> =
        serde_json::from_str(raw).map_err(|e| unparsed("an array", &e))?;
    let items = items
        .iter()
        .map(|item| literal(item, element))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cast(&format!("[{}]", items.join(", ")), ty))
}

fn string(s: &str, ty: &DuckType) -> Result<String, String> {
    match ty {
        DuckType::Float | DuckType::Double if matches!(s, "NaN" | "Infinity" | "-Infinity") => {
            Ok(cast(&sql_str(s), ty))
        }
        DuckType::Uuid => match Uuid::parse_str(s) {
            Ok(id) => Ok(cast(&sql_str(&id.to_string()), ty)),
            Err(_) => Err("a string that is not a UUID where UUID is expected".to_owned()),
        },
        DuckType::Date | DuckType::Timestamp | DuckType::TimestampTz => {
            Ok(cast(&sql_str(&before_christ(s)), ty))
        }
        DuckType::Time => Ok(cast(&sql_str(s), ty)),
        _ => Err(format!("a string where {} is expected", ty.sql())),
    }
}

/// Postgres ends a BC date or timestamp with ` BC` (`0044-03-15 BC`,
/// `0044-03-15T10:00:00+00:00 BC`); DuckDB reads the era after the date
/// (`0044-03-15 (BC)`, `0044-03-15 (BC) 10:00:00+00:00`).
fn before_christ(s: &str) -> Cow<'_, str> {
    let Some(body) = s.strip_suffix(" BC") else {
        return Cow::Borrowed(s);
    };
    match body.split_once('T') {
        Some((date, time)) => Cow::Owned(format!("{date} (BC) {time}")),
        None => Cow::Owned(format!("{body} (BC)")),
    }
}

#[cfg(test)]
mod tests;
