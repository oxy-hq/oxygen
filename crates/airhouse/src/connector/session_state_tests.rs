use super::{refuse_on_shared_session, session_state_reason as reason};

#[test]
fn reads_and_persistent_writes_are_shared_safely() {
    for sql in [
        "SELECT 1",
        "with t as (select 1) select * from t",
        "FROM orders LIMIT 5",
        "DESCRIBE orders",
        "SHOW TABLES",
        "EXPLAIN SELECT 1",
        "SUMMARIZE orders",
        "INSERT INTO s.t VALUES (1)",
        "UPDATE s.t SET a = 1",
        "DELETE FROM s.t",
        "CREATE TABLE s.t AS SELECT 1",
        "CREATE OR REPLACE VIEW s.v AS SELECT 1",
        "DROP TABLE s.t",
        "ALTER TABLE s.t ADD COLUMN b INT",
        "SELECT 'SET x = 1; BEGIN' AS settings",
        "SELECT * FROM t WHERE begin_at > now()",
        "SELECT temp FROM weather",
        "SELECT w.temp FROM weather w",
        "SELECT \"temp\" FROM weather",
        "-- SET in a comment\nSELECT 1",
        "/* BEGIN */ SELECT 1",
        "SELECT $$ ; SET x $$",
        "SELECT E'it\\'s ; SET x'",
        "SELECT 1; SELECT 2;",
        "EXPLAIN SELECT 1",
        "EXPLAIN ANALYZE SELECT 1",
        "EXPLAIN (FORMAT json) SELECT 1",
        "SUMMARIZE SELECT * FROM t",
        "DESCRIBE s.orders",
        "DESCRIBE \"my table\"",
        "SELECT a$$b FROM t",
        "SELECT $1",
        "",
    ] {
        assert_eq!(reason(sql), None, "{sql}");
    }
}

#[test]
fn anything_not_known_to_be_safe_is_refused_by_name() {
    for (sql, want) in [
        ("SET search_path = 'other'", "SET"),
        ("set TimeZone = 'UTC'", "SET"),
        ("RESET search_path", "RESET"),
        ("BEGIN", "BEGIN"),
        ("START TRANSACTION", "START"),
        ("COMMIT", "COMMIT"),
        ("ROLLBACK", "ROLLBACK"),
        ("USE other_db", "USE"),
        ("ATTACH 'x.db' AS x", "ATTACH"),
        ("DETACH x", "DETACH"),
        ("PRAGMA threads=1", "PRAGMA"),
        ("LOAD httpfs", "LOAD"),
        ("FORCE INSTALL httpfs", "FORCE"),
        ("CALL enable_profiling()", "CALL"),
        ("IMPORT DATABASE 'x'", "IMPORT"),
        ("EXPORT DATABASE 'x'", "EXPORT"),
        ("CHECKPOINT", "CHECKPOINT"),
        ("VACUUM", "VACUUM"),
        ("COPY t TO 'x.csv'", "COPY"),
        ("PREPARE q AS SELECT 1", "PREPARE"),
        ("CREATE TEMP TABLE t AS SELECT 1", "CREATE TEMP"),
        ("CREATE TEMPORARY VIEW v AS SELECT 1", "CREATE TEMPORARY"),
        ("create or replace temp macro m(x) as x", "CREATE TEMP"),
        ("CREATE SECRET s (TYPE s3)", "CREATE SECRET"),
        ("CREATE PERSISTENT SECRET s (TYPE s3)", "CREATE PERSISTENT"),
        ("CREATE LOCAL TEMP TABLE t (a INT)", "CREATE TEMP"),
        ("DROP SECRET s3_creds", "DROP SECRET"),
        ("DROP TEMPORARY SECRET s", "DROP TEMPORARY"),
        ("DROP PERSISTENT SECRET s", "DROP PERSISTENT"),
        ("DROP TEMP TABLE t", "DROP TEMP"),
        (
            "CREATE GLOBAL TEMPORARY TABLE t (a INT)",
            "CREATE TEMPORARY",
        ),
        (
            "CREATE OR REPLACE LOCAL TEMP VIEW v AS SELECT 1",
            "CREATE TEMP",
        ),
        ("CREATE UNLOGGED TEMP TABLE t (a INT)", "CREATE TEMP"),
        (
            "EXPLAIN ANALYZE CREATE TEMP TABLE t AS SELECT 1",
            "CREATE TEMP",
        ),
        ("EXPLAIN ANALYZE SET x = 1", "EXPLAIN SET"),
        (
            "EXPLAIN (ANALYZE, FORMAT json) INSERT INTO temp.t VALUES (1)",
            "a write to the session catalog `temp`",
        ),
        ("SUMMARIZE CREATE TEMP TABLE t AS SELECT 1", "CREATE TEMP"),
        ("DESCRIBE SET x = 1", "DESCRIBE SET"),
        ("/* lead */ SET x = 1", "SET"),
    ] {
        assert_eq!(reason(sql).as_deref(), Some(want), "{sql}");
    }
}

#[test]
fn writes_into_session_catalogs_are_refused() {
    for sql in [
        "CREATE TABLE temp.t AS SELECT 1",
        "CREATE TABLE temp.main.t (a INT)",
        "CREATE TABLE \"temp\".t (a INT)",
        "CREATE TABLE memory.main.t (a INT)",
        "INSERT INTO temp.t VALUES (1)",
        "ALTER TABLE TEMP.t ADD COLUMN b INT",
    ] {
        let got = reason(sql).unwrap_or_default();
        assert!(got.contains("session catalog"), "{sql}: {got}");
    }
}

#[test]
fn every_statement_of_a_batch_is_checked() {
    assert_eq!(
        reason("SELECT 1; SET search_path = 'x'; SELECT 2").as_deref(),
        Some("SET")
    );
    // A `;` the shared splitter would treat as inside a string, but DuckDB
    // reads as a separator, is still seen.
    assert_eq!(reason("SELECT 'a'';' ; SET x = 1").as_deref(), Some("SET"));
    // `a$$` and `b$$` are identifiers, not a dollar quote hiding the `;`.
    assert_eq!(
        reason("SELECT a$$ FROM t; SET x = 1; SELECT b$$ FROM t").as_deref(),
        Some("SET")
    );
    // `xe'…'` is not an escape string: `\'` ends it, so the `;` separates.
    assert_eq!(
        reason("SELECT xe'\\'; SET x = 1 --'").as_deref(),
        Some("SET")
    );
}

#[test]
fn sql_whose_split_is_in_doubt_is_refused() {
    for (sql, want) in [
        (
            "SELECT 1 /* outer /* inner */ ; SET x = 1 */",
            "a nested comment",
        ),
        ("SELECT 'open", "an unterminated string"),
        ("SELECT 1 /* open", "an unterminated comment"),
        ("SELECT \"open", "an unterminated quoted identifier"),
        ("SELECT $$ open", "an unterminated dollar-quoted string"),
        ("SELECT 1e'\\'; SET x = 1 --'", "an ambiguous string prefix"),
        (
            "SELECT 1$$ $$; SET x = 1; SELECT 1 -- $$",
            "an ambiguous number/identifier boundary",
        ),
        (
            "SELECT 1a$$ FROM t; SET x = 1; SELECT $$",
            "an ambiguous number/identifier boundary",
        ),
    ] {
        assert_eq!(reason(sql).as_deref(), Some(want), "{sql}");
    }
}

#[test]
fn the_refusal_names_the_reason_and_why() {
    let text = refuse_on_shared_session("SET search_path = 'x'")
        .unwrap_err()
        .to_string();
    assert!(text.contains("SET is not allowed here"), "{text}");
    assert!(text.contains("shared with other queries"), "{text}");
    assert!(refuse_on_shared_session("SELECT 1").is_ok());
}

#[test]
fn stacked_wrapping_keywords_are_bounded() {
    assert_eq!(reason("EXPLAIN EXPLAIN SELECT 1"), None);
    let deep = format!("{}SELECT 1", "EXPLAIN ".repeat(100_000));
    let got = reason(&deep).unwrap_or_default();
    assert!(got.contains("nested more than"), "{got}");
}
