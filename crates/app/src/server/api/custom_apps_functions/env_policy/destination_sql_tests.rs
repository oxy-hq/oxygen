//! The mapped-destination allowlist, dialect by dialect.

use super::*;

fn fence(dialect: SqlDialect, mapped: &[&str], production: &[&str]) -> DestinationFence {
    DestinationFence {
        dialect,
        mapped: mapped.iter().map(|s| s.to_string()).collect(),
        production: production.iter().map(|s| s.to_string()).collect(),
    }
}

fn sends(f: &DestinationFence, sql: &str) {
    if let Err(held) = admit_mapped_statement(f, sql) {
        panic!(
            "{sql:?} should be sent on {:?}, held: {}",
            f.dialect, held.why
        );
    }
}

fn holds(f: &DestinationFence, sql: &str) -> HeldStatement {
    admit_mapped_statement(f, sql)
        .err()
        .unwrap_or_else(|| panic!("{sql:?} must be held on {:?}", f.dialect))
}

fn snowflake() -> DestinationFence {
    fence(SqlDialect::Snowflake, &["STAGING_DB"], &["PROD_DB"])
}

fn clickhouse() -> DestinationFence {
    fence(SqlDialect::CLICKHOUSE, &["staging"], &["prod"])
}

/// The shapes the fix-round-2 review found past the denylist — all held now,
/// because none is on the allowlist.
#[test]
fn the_reviewers_write_shapes_are_held() {
    let sf = snowflake();
    holds(&sf, "ALTER TABLE orders SWAP WITH PROD_DB.PUBLIC.ORDERS");
    holds(&sf, "ALTER TABLE orders RENAME TO PROD_DB.PUBLIC.ORDERS");
    holds(&sf, "SELECT * INTO PROD_DB.PUBLIC.ORDERS FROM orders");
    let ch = clickhouse();
    holds(
        &ch,
        "CREATE MATERIALIZED VIEW mv TO prod.events AS SELECT * FROM events",
    );
    holds(
        &ch,
        "CREATE TABLE b (a Int32) ENGINE = Buffer(prod, events, 16, 10, 100, 10000, 1000000, 10000000, 100000000)",
    );
    let distributed = holds(
        &ch,
        "CREATE TABLE d (a Int32) ENGINE = Distributed(cluster, prod, events)",
    );
    assert!(
        distributed.why.contains("Distributed"),
        "{}",
        distributed.why
    );
    let pg = fence(SqlDialect::Postgres, &["staging"], &["prod"]);
    holds(&pg, "SELECT a INTO orders_copy FROM orders");
    holds(
        &pg,
        "INSERT INTO t SELECT * FROM (SELECT 1) x WHERE false; SELECT 1",
    );
}

#[test]
fn a_write_target_must_be_unqualified_or_the_mapped_database() {
    let sf = snowflake();
    let held = holds(&sf, "INSERT INTO PROD_DB.PUBLIC.ORDERS (id) VALUES (1)");
    assert!(
        held.why.contains("production's database `PROD_DB`"),
        "{}",
        held.why
    );
    let other = holds(&sf, "UPDATE OTHER_DB.PUBLIC.ORDERS SET x = 1");
    assert!(
        other.why.contains("not the mapped destination"),
        "{}",
        other.why
    );
    for sql in [
        "INSERT INTO ORDERS (id) VALUES (1)",
        "INSERT INTO PUBLIC.ORDERS (id) VALUES (1)",
        "INSERT INTO STAGING_DB.PUBLIC.ORDERS (id) VALUES (1)",
        "INSERT INTO staging_db.public.orders (id) VALUES (1)",
        // Sources may read any database: staging reads production by design.
        "INSERT INTO orders SELECT * FROM PROD_DB.PUBLIC.ORDERS",
        "SELECT * FROM PROD_DB.PUBLIC.ORDERS",
    ] {
        sends(&sf, sql);
    }
}

/// A quoted name is the name exactly: `"staging_db"` is not `STAGING_DB` on
/// Snowflake, so it is not the mapped database.
#[test]
fn a_quoted_qualifier_is_compared_exactly() {
    let sf = snowflake();
    holds(
        &sf,
        r#"INSERT INTO "staging_db".PUBLIC.ORDERS (id) VALUES (1)"#,
    );
    sends(
        &sf,
        r#"INSERT INTO "STAGING_DB".PUBLIC.ORDERS (id) VALUES (1)"#,
    );
    holds(
        &sf,
        r#"INSERT INTO "prod_db".PUBLIC.ORDERS (id) VALUES (1)"#,
    );
    // One quoted identifier holding dots is one table, in the mapped schema —
    // what the host builds from `ctx.warehouse.insert("db", "a.b.c", …)`.
    sends(
        &sf,
        r#"INSERT INTO "PROD_DB.PUBLIC.ORDERS" ("id") VALUES (1)"#,
    );
}

#[test]
fn every_allowed_statement_kind_is_sent() {
    let pg = fence(SqlDialect::Postgres, &["staging"], &["prod"]);
    for sql in [
        "INSERT INTO orders (a, b) VALUES (1, 'x') ON CONFLICT (a) DO UPDATE SET b = excluded.b",
        "UPDATE orders SET b = 'y' WHERE a = 1",
        "DELETE FROM orders WHERE a = 1",
        "MERGE INTO orders o USING prod.public.src s ON o.a = s.a WHEN MATCHED THEN UPDATE SET b = s.b",
        "TRUNCATE TABLE orders",
        "DROP TABLE orders",
        "CREATE TABLE orders (a INT, b TEXT)",
        "CREATE TABLE orders_copy AS SELECT * FROM prod.public.orders",
        "SELECT * FROM prod.public.orders",
    ] {
        sends(&pg, sql);
    }
    let ch = clickhouse();
    for sql in [
        "CREATE TABLE t (a Int32) ENGINE = MergeTree ORDER BY a",
        "CREATE TABLE t (a Int32) ENGINE = ReplacingMergeTree ORDER BY a",
        "CREATE TABLE t (a Int32) ENGINE = Memory",
        "CREATE TABLE t (a Int32) ENGINE = Log",
        "INSERT INTO staging.events VALUES (1)",
        "INSERT INTO events SELECT * FROM prod.events",
    ] {
        sends(&ch, sql);
    }
}

#[test]
fn everything_else_is_held_on_every_dialect() {
    for dialect in [
        SqlDialect::Snowflake,
        SqlDialect::Postgres,
        SqlDialect::DuckDb,
        SqlDialect::BigQuery,
        SqlDialect::CLICKHOUSE,
        SqlDialect::MYSQL,
    ] {
        let f = fence(dialect, &["staging"], &["prod"]);
        for sql in [
            "USE prod",
            "SET x = 1",
            "CREATE DATABASE prod2",
            "CREATE VIEW v AS SELECT 1",
            "ALTER TABLE t ADD COLUMN x INT",
            "INSERT INTO t VALUES (1); INSERT INTO t VALUES (2)",
            "not sql at all (",
        ] {
            holds(&f, sql);
        }
    }
    let duck = fence(SqlDialect::DuckDb, &["md_staging"], &["md_prod"]);
    for sql in [
        "ATTACH 'prod.duckdb' AS p",
        "DETACH p",
        "PRAGMA enable_profiling",
        "COPY t TO 'out.csv'",
        "INSTALL httpfs",
        "CALL refresh()",
    ] {
        holds(&duck, sql);
    }
    let pg = fence(SqlDialect::Postgres, &["staging"], &["prod"]);
    holds(
        &pg,
        "WITH x AS (DELETE FROM prod_rows RETURNING *) INSERT INTO orders SELECT * FROM x",
    );
    holds(&pg, "CREATE TABLE p PARTITION OF orders FOR VALUES IN (1)");
    holds(
        &pg,
        "UPDATE orders o SET b = 'x' FROM prod.public.src s WHERE o.a = s.a; SELECT 1",
    );
}

#[test]
fn postgres_counts_three_parts_as_a_database() {
    let f = fence(SqlDialect::Postgres, &["staging"], &["prod"]);
    holds(&f, "INSERT INTO prod.public.orders VALUES (1)");
    holds(&f, "INSERT INTO other.public.orders VALUES (1)");
    sends(&f, "INSERT INTO public.orders VALUES (1)");
    sends(&f, "INSERT INTO staging.public.orders VALUES (1)");
}

#[test]
fn clickhouse_and_mysql_count_two_parts_as_a_database() {
    let ch = clickhouse();
    holds(&ch, "INSERT INTO prod.events VALUES (1)");
    holds(&ch, "INSERT INTO other.events VALUES (1)");
    holds(
        &ch,
        "INSERT INTO FUNCTION remote('prod:9000', 'prod', 'events') VALUES (1)",
    );
    holds(&ch, "ALTER TABLE events DELETE WHERE id = 1");
    sends(&ch, "INSERT INTO events VALUES (1)");
    sends(&ch, "DELETE FROM events WHERE id = 1");
    let my = fence(SqlDialect::MYSQL, &["staging"], &["prod"]);
    holds(&my, "INSERT INTO prod.orders VALUES (1)");
    sends(&my, "INSERT INTO staging.orders VALUES (1)");
}

/// BigQuery: a dataset is a database here. Only the mapped entry's datasets,
/// never a production one, and never through a project.
#[test]
fn bigquery_writes_only_the_mapped_datasets() {
    let f = fence(SqlDialect::BigQuery, &["sales_staging"], &["sales"]);
    holds(&f, "INSERT INTO sales.orders (id) VALUES (1)");
    holds(
        &f,
        "INSERT INTO `prod-project.sales_staging.orders` (id) VALUES (1)",
    );
    holds(&f, "INSERT INTO other.orders (id) VALUES (1)");
    sends(&f, "INSERT INTO sales_staging.orders (id) VALUES (1)");
    sends(&f, "INSERT INTO `sales_staging.orders` (id) VALUES (1)");
    sends(&f, "INSERT INTO orders (id) VALUES (1)");
    let shared = fence(SqlDialect::BigQuery, &["sales"], &["sales"]);
    holds(&shared, "INSERT INTO sales.orders (id) VALUES (1)");
}

/// MotherDuck and DuckDB: `catalog.schema.table`; `x.table` only for the
/// mapped catalog or `main`.
#[test]
fn duckdb_writes_only_the_mapped_catalog_or_main() {
    let f = fence(SqlDialect::DuckDb, &["md_staging"], &["md_prod", "md_b"]);
    holds(&f, "INSERT INTO md_prod.main.orders VALUES (1)");
    let other_key = holds(&f, "INSERT INTO md_b.orders VALUES (1)");
    assert!(
        other_key.why.contains("production's database `md_b`"),
        "{}",
        other_key.why
    );
    holds(&f, "INSERT INTO md_other.main.orders VALUES (1)");
    holds(&f, "INSERT INTO analytics.orders VALUES (1)");
    sends(&f, "INSERT INTO md_staging.main.orders VALUES (1)");
    sends(&f, "INSERT INTO main.orders VALUES (1)");
    sends(&f, "INSERT INTO orders SELECT * FROM md_prod.main.orders");
}

/// Fix round 3, ruling 1: Snowflake's multi-table INSERT leaves `table` empty
/// and names its targets in the INTO clauses — every form is held.
#[test]
fn a_snowflake_multi_table_insert_is_held_in_every_form() {
    let sf = snowflake();
    for sql in [
        "INSERT ALL INTO orders INTO PROD_DB.PUBLIC.ORDERS SELECT * FROM src",
        "INSERT ALL INTO orders (id) VALUES (id) INTO PROD_DB.PUBLIC.ORDERS SELECT id FROM src",
        "INSERT ALL WHEN id > 1 THEN INTO PROD_DB.PUBLIC.ORDERS ELSE INTO orders SELECT id FROM src",
        "INSERT FIRST WHEN id > 1 THEN INTO orders WHEN id > 0 THEN INTO PROD_DB.PUBLIC.ORDERS SELECT id FROM src",
        "INSERT OVERWRITE ALL INTO orders SELECT * FROM src",
    ] {
        let held = holds(&sf, sql);
        assert!(
            held.why.contains("multi-table")
                || held.why.contains("parse")
                || held.why.contains("classified"),
            "{sql}: {}",
            held.why
        );
    }
}

/// Fix round 3, ruling 2: a name built by a function — Snowflake's
/// `IDENTIFIER('…')` — is not a name the fence can read, so it is held as a
/// target and as a source.
#[test]
fn a_table_named_through_a_function_is_held() {
    let sf = snowflake();
    for sql in [
        "INSERT INTO IDENTIFIER('PROD_DB.PUBLIC.ORDERS') (id) VALUES (1)",
        "DELETE FROM IDENTIFIER('PROD_DB.PUBLIC.ORDERS') WHERE id = 1",
        "INSERT INTO orders SELECT * FROM IDENTIFIER('PROD_DB.PUBLIC.ORDERS')",
    ] {
        let held = holds(&sf, sql);
        assert!(
            held.why.contains("function") || held.why.contains("parse"),
            "{sql}: {}",
            held.why
        );
    }
}

/// Fix round 3 nits.
#[test]
fn reads_calling_side_effect_functions_output_into_and_like_are_held() {
    let sf = snowflake();
    holds(&sf, "SELECT SYSTEM$CANCEL_QUERY('01a2')");
    let pg = fence(SqlDialect::Postgres, &["staging"], &["prod"]);
    holds(&pg, "SELECT dblink_exec('host=prod', 'DELETE FROM orders')");
    holds(&pg, "SELECT pg_advisory_lock(1)");
    // The same function check as staging's ctx.oltp: SQL run from text and
    // Unicode-escaped names are held on a mapped read too.
    holds(
        &pg,
        "SELECT query_to_xml('select pg_notify(''c'', ''p'')', true, false, '')",
    );
    holds(&pg, r#"SELECT U&"pg_notif\0079"('c', 'p')"#);
    sends(
        &pg,
        "SELECT count(*) FROM prod.public.orders WHERE note = 'nextval('",
    );
    let my = fence(SqlDialect::MYSQL, &["staging"], &["prod"]);
    holds(&my, "CREATE TABLE copy LIKE prod.orders");
    // ClickHouse and MySQL database names are case-sensitive.
    holds(&my, "INSERT INTO Staging.orders VALUES (1)");
    sends(&my, "INSERT INTO staging.orders VALUES (1)");
    let ch = clickhouse();
    holds(&ch, "INSERT INTO STAGING.events VALUES (1)");
    let generic = fence(SqlDialect::Other("MSSQL"), &["staging"], &["prod"]);
    let output = admit_mapped_statement(
        &generic,
        "DELETE FROM orders OUTPUT deleted.id INTO prod.audit WHERE id = 1",
    );
    assert!(output.is_err(), "OUTPUT … INTO writes a second table");
}

/// BigQuery dataset names are case-sensitive: `SALES_STAGING` is not the
/// mapped `sales_staging`.
#[test]
fn bigquery_datasets_compare_with_case() {
    let f = fence(SqlDialect::BigQuery, &["sales_staging"], &["sales"]);
    holds(&f, "INSERT INTO SALES_STAGING.orders (id) VALUES (1)");
    holds(&f, "INSERT INTO `Sales_Staging.orders` (id) VALUES (1)");
    sends(&f, "INSERT INTO sales_staging.orders (id) VALUES (1)");
    holds(&f, "INSERT INTO SALES.orders (id) VALUES (1)");
}

/// The fence parses once, through the same guard as `classify`: a statement
/// too deep to check is held unparsed, and one merely too deep for a Tokio
/// worker's stack is checked on the big one — never an abort.
#[test]
fn a_deep_statement_is_held_or_checked_never_aborts() {
    let insert = |terms: usize| {
        let filter = vec!["flag"; terms].join(" OR ");
        format!("INSERT INTO STAGING_DB.PUBLIC.T SELECT * FROM PROD_DB.PUBLIC.S WHERE {filter}")
    };
    let admitted = |sql: String| {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || admit_mapped_statement(&snowflake(), &sql))
            .expect("spawn a 2 MiB thread")
            .join()
            .expect("the fence must never abort a 2 MiB caller")
    };
    admitted(insert(30_000)).expect("30,000 links is checkable");
    let held = admitted(insert(60_000)).expect_err("held");
    assert_eq!(held.verb, "UNCLASSIFIED");
    assert!(held.why.contains("nests"), "{}", held.why);
}
