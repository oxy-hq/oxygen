//! How a configured database's engine reads a `'…'` literal, without
//! building its connector.
//!
//! The `sqlquote` template filter escapes by this
//! ([`agentic_automation::WorkspaceContext::string_literal`]). It is the
//! answer the database's connector gives from
//! [`agentic_connector::DatabaseConnector::string_literal`], stated over
//! `config.yml`'s own engine type: a step's SQL is rendered before it is
//! reviewed, so the rule is needed before any connector may exist.
//!
//! It lives here because this is the layer that sees both sides. The rule
//! type is the connector's, which `oxy` may not import and which may not
//! import a platform crate itself; the engine type is `oxy`'s.

use agentic_connector::StringLiteral;
use oxy::config::model::DatabaseType;

/// The literal rule for `database`'s engine.
///
/// Deliberately not derived from [`oxy::config::model::Database::dialect`]:
/// that reports Redshift as `postgres`, and Redshift reads a backslash as an
/// escape where Postgres does not. The match is exhaustive so a new engine
/// cannot be added without saying how it reads a literal.
pub(crate) fn string_literal_of(database: &DatabaseType) -> StringLiteral {
    match database {
        // `''` is the only escape; a backslash is an ordinary character.
        // Airhouse is DuckLake behind pgwire, MotherDuck is DuckDB, and both
        // Postgres variants run with `standard_conforming_strings` on.
        DatabaseType::DuckDB(_)
        | DatabaseType::MotherDuck(_)
        | DatabaseType::Airhouse(_)
        | DatabaseType::AirhouseManaged(_)
        | DatabaseType::Postgres(_)
        | DatabaseType::PostgresManaged(_) => StringLiteral::Standard,
        // A backslash escapes, and `''` is a quote as well.
        DatabaseType::ClickHouse(_)
        | DatabaseType::Mysql(_)
        | DatabaseType::Snowflake(_)
        | DatabaseType::Redshift(_)
        | DatabaseType::DOMO(_) => StringLiteral::Backslash,
        // A backslash escapes and `''` is not a quote.
        DatabaseType::Bigquery(_) => StringLiteral::BackslashOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::string_literal_of;
    use agentic_connector::{SqlDialect, StringLiteral};
    use oxy::config::model::{
        Airhouse, AirhouseManaged, ClickHouse, DOMO, DatabaseType, MotherDuck, Mysql, Postgres,
        PostgresManaged, Redshift,
    };

    /// An engine as `config.yml` spells it, for the types with required fields.
    fn configured(yaml: &str) -> DatabaseType {
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    fn every_engine() -> Vec<(DatabaseType, StringLiteral)> {
        use StringLiteral::{Backslash, BackslashOnly, Standard};
        let snowflake = "type: snowflake\naccount: a\nusername: u\nwarehouse: w\n\
                         database: d\npassword_var: SNOWFLAKE_PASSWORD";
        vec![
            (configured("type: duckdb\ndataset: data"), Standard),
            (DatabaseType::MotherDuck(MotherDuck::default()), Standard),
            (DatabaseType::Airhouse(Airhouse::default()), Standard),
            (
                DatabaseType::AirhouseManaged(AirhouseManaged::default()),
                Standard,
            ),
            (DatabaseType::Postgres(Postgres::default()), Standard),
            (
                DatabaseType::PostgresManaged(PostgresManaged::default()),
                Standard,
            ),
            (DatabaseType::ClickHouse(ClickHouse::default()), Backslash),
            (DatabaseType::Mysql(Mysql::default()), Backslash),
            (configured(snowflake), Backslash),
            (DatabaseType::Redshift(Redshift::default()), Backslash),
            (DatabaseType::DOMO(DOMO::default()), Backslash),
            (configured("type: bigquery\ndataset: d"), BackslashOnly),
        ]
    }

    #[test]
    fn each_configured_engine_names_the_rule_it_reads() {
        for (database, want) in every_engine() {
            assert_eq!(string_literal_of(&database), want, "{database}");
        }
    }

    /// The value a quote-only filter let out of its literal on ClickHouse,
    /// written for every engine a workspace can configure. Redshift is the
    /// one its dialect gets wrong: it reports Postgres and reads a backslash.
    #[test]
    fn a_breakout_value_is_written_as_each_engine_reads_it() {
        let breakout = "x\\' OR 1=1 -- ";
        for (database, rule) in every_engine() {
            let written = string_literal_of(&database).quote(breakout);
            let want = match rule {
                StringLiteral::Standard => "'x\\'' OR 1=1 -- '",
                StringLiteral::Backslash => "'x\\\\'' OR 1=1 -- '",
                StringLiteral::BackslashOnly => "'x\\\\\\' OR 1=1 -- '",
            };
            assert_eq!(written, want, "{database}");
            assert_eq!(
                string_literal_of(&database).quote("Paris"),
                "'Paris'",
                "{database}"
            );
        }
        assert_ne!(
            string_literal_of(&DatabaseType::Redshift(Redshift::default())),
            SqlDialect::Postgres.string_literal(),
            "Redshift is not read the way its dialect is"
        );
    }
}
