//! A `'…'` literal for SQL whose engine is not known where it is written.
//!
//! The engines Oxy talks to read a string literal three ways: `''` alone is
//! a quote (DuckDB, Postgres); a backslash escapes as well (ClickHouse, MySQL,
//! Snowflake, Redshift, Domo); or a backslash escapes and `''` is *not* a
//! quote (BigQuery). A caller that knows its engine asks for that rule
//! (`agentic_connector::StringLiteral`). This is the other half: what a
//! template filter may write when nothing says which engine will read it.

use thiserror::Error;

/// A value that has no spelling every engine reads the same way.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "sqlquote: the value holds a {found} and the database this text is sent to is not known \
     here, so there is no spelling every engine reads as that value. Apply `sqlquote` in the \
     `execute_sql` task that runs the SQL, where the task's `database` decides the escaping"
)]
pub struct EngineUnknown {
    found: &'static str,
}

/// `value` as a literal that means the same on every engine, or a refusal.
///
/// A value with neither `'` nor `\` is `'value'` everywhere, so it is written.
/// Anything else is refused, because no spelling is safe on all three
/// grammars:
///
/// - a quote must be `''` where the backslash is ordinary and `\'` on
///   BigQuery, where `''` ends the literal — and `\'` ends it on the former;
/// - a backslash left bare ends the literal early where it escapes the
///   closing quote, and doubled it is two characters where it is ordinary;
/// - writing no backslash inside a literal (`'a' || CHR(92) || 'b'`, what
///   `oxy_airlayer_compat::substitute_params` does where Postgres and
///   Redshift cannot be told apart) is not valid everywhere either: MySQL
///   has no `CHR` and reads `||` as `OR`. It is also an expression, not a
///   literal, so a template writing `DATE {{ x | sqlquote }}` stops parsing.
pub fn quote_engine_unknown(value: &str) -> Result<String, EngineUnknown> {
    let found = if value.contains('\'') {
        "single quote"
    } else if value.contains('\\') {
        "backslash"
    } else {
        return Ok(format!("'{value}'"));
    };
    Err(EngineUnknown { found })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_value_is_written_as_every_engine_reads_it() {
        assert_eq!(quote_engine_unknown("plain 100%").unwrap(), "'plain 100%'");
        assert_eq!(quote_engine_unknown("").unwrap(), "''");
        assert_eq!(quote_engine_unknown("2024-01-01").unwrap(), "'2024-01-01'");
    }

    /// A backslash, a quote, the two together, and a backslash at the end:
    /// each breaks out of a quote-doubled literal on some engine.
    #[test]
    fn a_value_no_engine_agrees_on_is_refused() {
        for value in ["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"] {
            let refused = quote_engine_unknown(value).unwrap_err().to_string();
            assert!(refused.contains("is not known"), "{value:?}: {refused}");
        }
    }
}
