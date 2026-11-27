//! Which execution path a statement takes: wrapped and typed, run as-is with
//! its rows, or run for its effect alone.

use agentic_connector::strip_leading_noise;

/// What shape of statement is this — for picking the right execution path?
///
/// Three categories matter to the airhouse connector:
///
/// - `Subquery`: `SELECT` / `WITH` / `FROM` / `TABLE` / `VALUES`. Pure
///   relational expressions that can be wrapped in `CREATE TEMP TABLE AS
///   (…)`, `DESCRIBE (…)`, and `SELECT … FROM (…)`. Goes through the
///   typed path.
/// - `Statement`: `SHOW` / `SUMMARIZE` / `DESCRIBE` / `EXPLAIN` / `PIVOT` /
///   `UNPIVOT`. Top-level DuckDB statements that **return rows** but are
///   **not** subquery-able — `DESCRIBE SHOW` and `SELECT * FROM (SHOW)` both
///   error in DuckDB. Executed directly, all columns surfaced as `Text`.
/// - `DdlDml`: `CREATE` / `INSERT` / `UPDATE` / `DELETE` / `ALTER` / `DROP` /
///   anything else. Executed directly, returns no rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatementKind {
    Subquery,
    Statement,
    DdlDml,
}

/// Classify `sql` by its first keyword, after leading whitespace and `--` /
/// `/* */` comments.
///
/// The comments must be skipped: a verified query file opens with its
/// `/* oxy: database: … */` header, and reading `/*` as the keyword filed
/// every one of them as `DdlDml`, which runs the statement and throws its
/// rows away. Airhouse answered with the rows; the caller got none.
pub(super) fn classify(sql: &str) -> StatementKind {
    let first: String = strip_leading_noise(sql)
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_ascii_uppercase();
    match first.as_str() {
        "SELECT" | "WITH" | "FROM" | "TABLE" | "VALUES" => StatementKind::Subquery,
        "SHOW" | "SUMMARIZE" | "DESCRIBE" | "EXPLAIN" | "PIVOT" | "UNPIVOT" => {
            StatementKind::Statement
        }
        _ => StatementKind::DdlDml,
    }
}

#[cfg(test)]
mod tests {
    use super::{StatementKind, classify};

    /// `examples/example_sql/total_number_of_store.sql`, byte for byte: the
    /// verified query the FSM answered "no rows" for while Airhouse logged
    /// `rows=1`.
    const VERIFIED_QUERY_FILE: &str = "/*\n  oxy:\n    database: local\n    embed:\n      \
        - How many stores do I have\n*/\nSELECT COUNT(DISTINCT Store) AS number_of_stores\n\
        FROM oxymart;";

    #[test]
    fn a_verified_query_file_with_its_header_is_a_subquery() {
        assert_eq!(classify(VERIFIED_QUERY_FILE), StatementKind::Subquery);
        // `execute_query` classifies after `normalize_sql` drops the `;`.
        let normalized = agentic_connector::normalize_sql(VERIFIED_QUERY_FILE);
        assert_eq!(classify(normalized), StatementKind::Subquery);
        assert!(super::super::first_row_statement(VERIFIED_QUERY_FILE).is_some());
    }

    #[test]
    fn leading_comments_are_skipped_whatever_the_statement() {
        for (sql, kind) in [
            ("-- oxy: database: local\nSELECT 1", StatementKind::Subquery),
            (
                "/* a */ -- b\n  WITH t AS (SELECT 1) SELECT * FROM t",
                StatementKind::Subquery,
            ),
            ("/* header */\nFROM t SELECT *", StatementKind::Subquery),
            ("/* header */ SHOW TABLES", StatementKind::Statement),
            ("-- c\nEXPLAIN SELECT 1", StatementKind::Statement),
            (
                "/* header */ INSERT INTO t VALUES (1)",
                StatementKind::DdlDml,
            ),
            ("-- c\nDROP TABLE t", StatementKind::DdlDml),
            ("/* only a comment */", StatementKind::DdlDml),
        ] {
            assert_eq!(classify(sql), kind, "sql: {sql}");
        }
    }

    #[test]
    fn select_family_is_subquery() {
        for sql in [
            "SELECT 1",
            "  select 1",
            "WITH t AS (SELECT 1) SELECT * FROM t",
            "FROM t SELECT *",
            "TABLE t",
            "VALUES (1, 2)",
        ] {
            assert_eq!(classify(sql), StatementKind::Subquery, "sql: {sql}");
        }
    }

    #[test]
    fn duckdb_result_producing_keywords_are_statement_form() {
        // These return rows but cannot be wrapped in `(…)` — DESCRIBE / temp
        // table / subquery wrappings all reject them. Need to be executed
        // directly via the `Statement` path.
        for sql in [
            "SHOW TABLES",
            "show databases",
            "SHOW SCHEMAS",
            "SUMMARIZE my_table",
            "DESCRIBE my_table",
            "EXPLAIN SELECT 1",
            "PIVOT t ON year USING sum(amount)",
            "UNPIVOT t ON a, b",
        ] {
            assert_eq!(classify(sql), StatementKind::Statement, "sql: {sql}");
        }
    }

    #[test]
    fn ddl_dml_is_ddldml() {
        for sql in [
            "CREATE TABLE t (id INT)",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET id = 2",
            "DELETE FROM t",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN c INT",
            "",
        ] {
            assert_eq!(classify(sql), StatementKind::DdlDml, "sql: {sql}");
        }
    }
}
