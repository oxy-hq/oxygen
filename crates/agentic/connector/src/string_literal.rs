//! How each engine reads the body of a `'…'` literal.
//!
//! Doubling `'` is not enough everywhere. On an engine that also reads a
//! backslash as an escape, a value ending in `\` swallows the closing quote,
//! and whatever follows the literal is read as part of it (or the reverse:
//! the rest of the value is read as SQL). One escape function is therefore
//! wrong for some engine whichever one it is, so the rule lives beside the
//! dialect and every site that writes a value into SQL text asks for it.

use crate::connector::SqlDialect;

/// The escapes an engine reads inside a single-quoted string literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringLiteral {
    /// `''` is the only escape and a backslash is an ordinary character:
    /// DuckDB (so Airhouse and MotherDuck), SQLite, and Postgres with
    /// `standard_conforming_strings` on, its default since 9.1.
    Standard,
    /// A backslash starts an escape, and `''` is a quote as well: ClickHouse,
    /// MySQL (unless `NO_BACKSLASH_ESCAPES`), Snowflake, Redshift, Domo.
    Backslash,
    /// A backslash starts an escape and `''` is **not** a quote — BigQuery
    /// reads it as the end of one literal and the start of the next, or as
    /// the opening of a `'''` triple-quoted one. The quote is written `\'`.
    BackslashOnly,
}

impl StringLiteral {
    /// `value` as a complete literal, quotes included.
    ///
    /// A value with neither `'` nor `\` is written the same under every rule.
    /// The backslash is doubled before the quote is touched, so the escape
    /// added for a quote is never itself doubled.
    pub fn quote(self, value: &str) -> String {
        let body = match self {
            Self::Standard => value.replace('\'', "''"),
            Self::Backslash => value.replace('\\', "\\\\").replace('\'', "''"),
            Self::BackslashOnly => value.replace('\\', "\\\\").replace('\'', "\\'"),
        };
        format!("'{body}'")
    }
}

impl SqlDialect {
    /// How this dialect's engine reads a string literal.
    ///
    /// A connector can know more than its dialect does — Redshift speaks the
    /// Postgres dialect and reads backslashes — so code holding a connector
    /// asks [`DatabaseConnector::string_literal`] instead.
    ///
    /// An engine nobody has classified is treated as reading backslashes: a
    /// doubled backslash on an engine that does not read them changes the
    /// value, where a bare one on an engine that does can end the literal.
    ///
    /// [`DatabaseConnector::string_literal`]: crate::connector::DatabaseConnector::string_literal
    pub fn string_literal(self) -> StringLiteral {
        match self {
            Self::DuckDb | Self::Sqlite | Self::Postgres => StringLiteral::Standard,
            Self::BigQuery => StringLiteral::BackslashOnly,
            Self::Snowflake | Self::Other(_) => StringLiteral::Backslash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backslash, a quote, the two together, and a backslash at the end.
    const VALUES: &[&str] = &["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"];

    /// Read one literal the way an engine under `rule` would, returning the
    /// value it holds and whatever text follows its closing quote.
    fn read(rule: StringLiteral, sql: &str) -> Option<(String, &str)> {
        let body = sql.strip_prefix('\'')?;
        let mut chars = body.char_indices().peekable();
        let mut value = String::new();
        while let Some((at, c)) = chars.next() {
            match c {
                '\\' if rule != StringLiteral::Standard => value.push(chars.next()?.1),
                '\'' if rule != StringLiteral::BackslashOnly
                    && matches!(chars.peek(), Some((_, '\''))) =>
                {
                    chars.next();
                    value.push('\'');
                }
                '\'' => return Some((value, &body[at + 1..])),
                c => value.push(c),
            }
        }
        None
    }

    #[test]
    fn every_rule_keeps_the_value_inside_its_literal() {
        for rule in [
            StringLiteral::Standard,
            StringLiteral::Backslash,
            StringLiteral::BackslashOnly,
        ] {
            for value in VALUES {
                let sql = format!("{} AND tail", rule.quote(value));
                let (held, rest) = read(rule, &sql)
                    .unwrap_or_else(|| panic!("{rule:?}: {value:?} left the literal open: {sql}"));
                assert_eq!(&held, value, "{rule:?}: {sql}");
                assert_eq!(rest, " AND tail", "{rule:?}: {value:?} ended early: {sql}");
            }
        }
    }

    #[test]
    fn each_rule_spells_the_escapes_its_engine_reads() {
        let value = "x\\' OR 1=1 -- ";
        assert_eq!(StringLiteral::Standard.quote(value), "'x\\'' OR 1=1 -- '");
        assert_eq!(
            StringLiteral::Backslash.quote(value),
            "'x\\\\'' OR 1=1 -- '"
        );
        assert_eq!(
            StringLiteral::BackslashOnly.quote(value),
            "'x\\\\\\' OR 1=1 -- '"
        );
        assert_eq!(StringLiteral::Standard.quote("C:\\"), "'C:\\'");
        assert_eq!(StringLiteral::Backslash.quote("C:\\"), "'C:\\\\'");
        assert_eq!(StringLiteral::BackslashOnly.quote("C:\\"), "'C:\\\\'");
        assert_eq!(StringLiteral::BackslashOnly.quote("it's"), "'it\\'s'");
    }

    #[test]
    fn a_value_with_no_quote_and_no_backslash_is_spelled_one_way() {
        for rule in [
            StringLiteral::Standard,
            StringLiteral::Backslash,
            StringLiteral::BackslashOnly,
        ] {
            assert_eq!(rule.quote("plain 100%"), "'plain 100%'", "{rule:?}");
            assert_eq!(rule.quote(""), "''", "{rule:?}");
        }
    }

    #[test]
    fn a_dialect_names_the_rule_its_engine_reads() {
        for standard in [SqlDialect::DuckDb, SqlDialect::Sqlite, SqlDialect::Postgres] {
            assert_eq!(standard.string_literal(), StringLiteral::Standard);
        }
        for backslash in [
            SqlDialect::CLICKHOUSE,
            SqlDialect::MYSQL,
            SqlDialect::Snowflake,
            SqlDialect::Other("DOMO"),
            SqlDialect::Other("something new"),
        ] {
            assert_eq!(backslash.string_literal(), StringLiteral::Backslash);
        }
        assert_eq!(
            SqlDialect::BigQuery.string_literal(),
            StringLiteral::BackslashOnly
        );
    }
}
