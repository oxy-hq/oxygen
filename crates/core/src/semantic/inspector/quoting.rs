//! How the schema inspector writes a name into the SQL it sends.
//!
//! A schema name reaches the inspector from two places: `config.yml`, and the
//! `schema` parameter of the per-schema table listing — a request. Either is
//! written into SQL one of two ways, and each has its own rule:
//!
//! - as a **string literal** (`WHERE table_schema = '…'`), escaped the way the
//!   database's engine reads one ([`sql_string_literal`]);
//! - as part of a **BigQuery path** (`` `dataset.INFORMATION_SCHEMA.COLUMNS` ``),
//!   which is not escaped at all: a name is checked against the grammar of
//!   what it claims to be and refused otherwise ([`bigquery_dataset`],
//!   [`bigquery_configured_scope`]). Inside backticks a backtick ends the
//!   identifier and a backslash starts an escape, so there is nothing to
//!   double that is right for every name; a dataset ID has neither.

use oxy_shared::errors::OxyError;

use crate::config::model::DatabaseType;

/// The escapes an engine reads inside a `'…'` literal.
///
/// The same three rules as `agentic_connector::StringLiteral`, which this
/// crate may not import. `oxy-app`'s `string_literal_of` is the mapping the
/// `sqlquote` filter uses; a test there holds the two to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Literal {
    /// `''` is the only escape; a backslash is an ordinary character.
    QuoteDoubled,
    /// A backslash starts an escape, and `''` is a quote as well.
    Backslash,
    /// A backslash starts an escape and `''` is **not** a quote: it is
    /// written `\'`.
    BackslashOnly,
}

/// How `db`'s engine reads a literal. Exhaustive, so a new engine cannot be
/// added without an answer.
///
/// DuckDB (so MotherDuck and Airhouse) and Postgres
/// (`standard_conforming_strings`) read `''` alone. Redshift shares Postgres's
/// wire but, like ClickHouse / Snowflake / MySQL / Domo, reads a backslash.
/// BigQuery reads a backslash and does not read `''` as a quote.
fn literal_rule(db: &DatabaseType) -> Literal {
    match db {
        DatabaseType::DuckDB(_)
        | DatabaseType::MotherDuck(_)
        | DatabaseType::Airhouse(_)
        | DatabaseType::AirhouseManaged(_)
        | DatabaseType::Postgres(_)
        | DatabaseType::PostgresManaged(_) => Literal::QuoteDoubled,
        DatabaseType::ClickHouse(_)
        | DatabaseType::Mysql(_)
        | DatabaseType::Snowflake(_)
        | DatabaseType::Redshift(_)
        | DatabaseType::DOMO(_) => Literal::Backslash,
        DatabaseType::Bigquery(_) => Literal::BackslashOnly,
    }
}

/// `value` as a single-quoted SQL literal for `db`'s engine. A value with no
/// quote and no backslash is spelled the same for every engine.
///
/// The backslash is doubled before the quote is touched, so the escape added
/// for a quote is never itself doubled.
pub fn sql_string_literal(db: &DatabaseType, value: &str) -> String {
    let escaped = match literal_rule(db) {
        Literal::QuoteDoubled => value.replace('\'', "''"),
        Literal::Backslash => value.replace('\\', "\\\\").replace('\'', "''"),
        Literal::BackslashOnly => value.replace('\\', "\\\\").replace('\'', "\\'"),
    };
    format!("'{escaped}'")
}

/// The longest dataset ID BigQuery accepts.
const BIGQUERY_DATASET_MAX: usize = 1024;

/// Whether `name` is a BigQuery dataset ID: ASCII letters, digits and
/// underscores, at least one and at most 1,024 of them. That is BigQuery's
/// whole grammar for one — no dot, no hyphen, no space.
fn is_bigquery_dataset(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= BIGQUERY_DATASET_MAX
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `schema` — a name from a request — as the dataset of a BigQuery path.
///
/// A dataset ID or nothing: the listing this serves takes the `table_schema`
/// values the schema summary returned, and those are bare dataset IDs. A
/// project-qualified name, a region, or anything holding a backtick, a
/// backslash or a dot is the caller's error, not a name to escape.
pub(super) fn bigquery_dataset(schema: &str) -> Result<&str, OxyError> {
    if is_bigquery_dataset(schema) {
        return Ok(schema);
    }
    Err(OxyError::ArgumentError(
        "`schema` is not a BigQuery dataset name: one is letters, digits and underscores, \
         at most 1,024 of them"
            .to_string(),
    ))
}

/// A dataset key from `config.yml` as the leading part of a BigQuery path.
///
/// Config is allowed more than a request is, because it legitimately holds
/// more: a dataset, a project-qualified dataset (`my-project.sales`), or a
/// region (`region-us`, which `Database::datasets` falls back to — written
/// with its own backticks, which are dropped here so the path is quoted
/// once). Each dot-separated part is letters, digits, underscores, hyphens
/// and — for a domain-scoped project — colons; nothing that can end a quoted
/// identifier or start an escape inside one.
pub(super) fn bigquery_configured_scope(key: &str) -> Result<&str, OxyError> {
    let scope = key
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(key);
    let part_ok = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':'))
    };
    if scope.len() <= 2 * BIGQUERY_DATASET_MAX && scope.split('.').all(part_ok) {
        return Ok(scope);
    }
    Err(OxyError::ConfigurationError(format!(
        "BigQuery dataset `{key}` in config.yml is not a dataset, a `project.dataset`, or a \
         `region-…` name"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{ClickHouse, Mysql, Postgres, Redshift};

    fn clickhouse() -> DatabaseType {
        DatabaseType::ClickHouse(ClickHouse::default())
    }

    fn postgres() -> DatabaseType {
        DatabaseType::Postgres(Postgres::default())
    }

    fn bigquery() -> DatabaseType {
        serde_yaml::from_str("type: bigquery\ndataset: d").expect("a bigquery database")
    }

    #[test]
    fn each_engine_is_classified_by_how_it_reads_a_literal() {
        assert_eq!(literal_rule(&clickhouse()), Literal::Backslash);
        assert_eq!(
            literal_rule(&DatabaseType::Mysql(Mysql::default())),
            Literal::Backslash
        );
        // Redshift shares Postgres's wire but reads a backslash.
        assert_eq!(
            literal_rule(&DatabaseType::Redshift(Redshift::default())),
            Literal::Backslash
        );
        assert_eq!(literal_rule(&postgres()), Literal::QuoteDoubled);
        // BigQuery reads a backslash too, and was classified as not reading one.
        assert_eq!(literal_rule(&bigquery()), Literal::BackslashOnly);
    }

    /// A backslash, a quote, the two together, and a trailing backslash. With
    /// the quote alone doubled, the last two ended a ClickHouse literal early.
    #[test]
    fn a_schema_name_cannot_break_out_of_its_literal() {
        let written = |db: &DatabaseType| {
            ["a\\b", "it's", "x\\' OR 1=1 -- ", "C:\\"].map(|v| sql_string_literal(db, v))
        };
        assert_eq!(
            written(&clickhouse()),
            ["'a\\\\b'", "'it''s'", "'x\\\\'' OR 1=1 -- '", "'C:\\\\'"]
        );
        // Postgres reads a backslash as itself, so only the quote is doubled.
        assert_eq!(
            written(&postgres()),
            ["'a\\b'", "'it''s'", "'x\\'' OR 1=1 -- '", "'C:\\'"]
        );
        // BigQuery does not read `''` as a quote: it is `\'`, and a doubled
        // quote there would end the literal.
        assert_eq!(
            written(&bigquery()),
            ["'a\\\\b'", "'it\\'s'", "'x\\\\\\' OR 1=1 -- '", "'C:\\\\'"]
        );
        // A value with neither is the same bytes on all three.
        for db in [clickhouse(), postgres(), bigquery()] {
            assert_eq!(sql_string_literal(&db, "sales"), "'sales'");
        }
    }

    #[test]
    fn a_bigquery_dataset_name_from_a_request_is_a_dataset_id_or_refused() {
        for name in ["sales", "Sales_2024", "_staging", "a", &"d".repeat(1024)] {
            assert_eq!(bigquery_dataset(name).unwrap(), name);
        }
        for name in [
            "",
            // Ends the quoted identifier and starts SQL of the caller's own.
            "x` WHERE 1=0 UNION ALL SELECT a, 1 FROM `other.secret",
            // Another project's dataset.
            "other-project.sales",
            "region-us",
            // Starts an escape inside a quoted identifier.
            "sales\\",
            "sa les",
            "sales;",
            "sales'",
            "ventes_é",
            &"d".repeat(1025),
        ] {
            let refused = bigquery_dataset(name).unwrap_err();
            assert!(
                matches!(refused, OxyError::ArgumentError(_)),
                "{name:?}: {refused:?}"
            );
        }
    }

    #[test]
    fn a_configured_bigquery_scope_is_a_dataset_a_qualified_one_or_a_region() {
        for (key, want) in [
            ("sales", "sales"),
            ("my-project.sales", "my-project.sales"),
            (
                "example.com:my-project.sales",
                "example.com:my-project.sales",
            ),
            ("region-us", "region-us"),
            ("my-project.region-eu", "my-project.region-eu"),
            // `Database::datasets`' fallback carries its own backticks.
            ("`region-us`", "region-us"),
        ] {
            assert_eq!(bigquery_configured_scope(key).unwrap(), want, "{key}");
        }
        for key in [
            "",
            "``",
            "sales`",
            "a`.`b",
            "x` UNION ALL SELECT 1 FROM `y",
            "sales\\",
            "sa les",
            ".sales",
            "sales.",
            "a..b",
        ] {
            let refused = bigquery_configured_scope(key).unwrap_err();
            assert!(
                matches!(refused, OxyError::ConfigurationError(_)),
                "{key:?}: {refused:?}"
            );
        }
    }
}
