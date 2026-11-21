//! The backstop on a real Postgres server (see `read_only_backstop`).
//!
//! The connector under test logs in as a role of its own that holds `SELECT`,
//! `INSERT` and `DELETE` on `t`, `USAGE` on `s` and `CREATE` on the schema:
//! every write below is one the role may make, so the read-only transaction
//! is what refuses it. Setup runs as the server's superuser.
//!
//! CI sets `OXY_TEST_REQUIRE_POSTGRES=1`, which turns "no Postgres" from a
//! skip into a failure: these are security tests and must not go green
//! without running.

use std::time::Duration;

use agentic_connector::{DatabaseConnector, ExecutionResult, PostgresConnector};
use agentic_core::result::CellValue;

use crate::postgres_tests::{Dsn, test_dsn};

/// The server, or `None` (skip) when there is none and CI did not ask for
/// one.
pub(super) async fn dsn() -> Option<Dsn> {
    // `test_dsn` panics when `OXY_TEST_REQUIRE_POSTGRES=1` and there is none.
    let dsn = test_dsn().await;
    if dsn.is_none() {
        eprintln!("skipping: Docker not available and OXY_TEST_POSTGRES_URL not set");
    }
    dsn
}

pub(super) fn login(dsn: &Dsn, user: &str, password: &str) -> PostgresConnector {
    PostgresConnector::new(&dsn.host, dsn.port, user, password, &dsn.database)
}

/// A name of this test's own, so tests sharing the server never see each
/// other's schema or role.
fn unique(test: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("ro_{test}_{}_{nanos}", std::process::id())
}

/// Schema `s` with `t` (3 rows), sequence `s.s`, `w()` (a SQL function that
/// inserts — a write the statement's shape cannot show), and the login role
/// `<s>_agent` with the grants described in the module docs.
pub(super) struct Fixture {
    pub(super) dsn: Dsn,
    pub(super) admin: PostgresConnector,
    pub(super) s: String,
}

impl Fixture {
    pub(super) async fn new(test: &str) -> Option<Self> {
        let dsn = dsn().await?;
        let admin = login(&dsn, &dsn.user, &dsn.password);
        let s = unique(test);
        for stmt in [
            format!("CREATE SCHEMA {s}"),
            format!("CREATE TABLE {s}.t (id int, name text, amount numeric)"),
            format!("INSERT INTO {s}.t VALUES (1, 'a', 10.5), (2, 'b', NULL), (3, 'c', 30)"),
            format!("CREATE SEQUENCE {s}.s"),
            format!(
                "CREATE FUNCTION {s}.w() RETURNS int LANGUAGE sql VOLATILE AS \
                 $$ INSERT INTO {s}.t VALUES (99, 'w', 0) RETURNING 1 $$"
            ),
            format!("CREATE ROLE {s}_agent LOGIN PASSWORD 'agent'"),
            format!("GRANT USAGE, CREATE ON SCHEMA {s} TO {s}_agent"),
            format!("GRANT SELECT, INSERT, DELETE ON {s}.t TO {s}_agent"),
            format!("GRANT USAGE ON SEQUENCE {s}.s TO {s}_agent"),
        ] {
            admin.execute_statement(&stmt).await.expect(&stmt);
        }
        Some(Self { dsn, admin, s })
    }

    pub(super) fn agent(&self) -> PostgresConnector {
        login(&self.dsn, &format!("{}_agent", self.s), "agent")
    }

    pub(super) async fn scalar(&self, sql: &str) -> CellValue {
        scalar(&self.admin, sql).await
    }

    pub(super) async fn cleanup(self) {
        let s = &self.s;
        for stmt in [
            format!("DROP OWNED BY {s}_agent"),
            format!("DROP SCHEMA {s} CASCADE"),
            format!("DROP ROLE {s}_agent"),
        ] {
            self.admin.execute_statement(&stmt).await.expect(&stmt);
        }
    }
}

pub(super) async fn scalar(c: &PostgresConnector, sql: &str) -> CellValue {
    let r = c.execute_query(sql, 1).await.expect(sql);
    r.result.rows[0].0[0].clone()
}

#[tokio::test]
async fn read_only_refuses_writes_on_postgres() {
    let Some(f) = Fixture::new("w").await else {
        return;
    };
    let (c, s) = (f.agent(), f.s.clone());
    let copy_target = format!("/tmp/{s}.csv");
    let writes = [
        format!("DELETE FROM {s}.t"),
        format!("DELETE FROM {s}.t RETURNING *"),
        format!("WITH d AS (DELETE FROM {s}.t RETURNING *) SELECT * FROM d"),
        format!("SELECT 1; DELETE FROM {s}.t"),
        format!("SELECT nextval('{s}.s')"),
        format!("CREATE TABLE {s}.u(x int)"),
        format!("COPY (SELECT 1) TO '{copy_target}'"),
        format!("SELECT {s}.w()"),
        "SET default_transaction_read_only = off".to_string(),
    ];
    for sql in &writes {
        let out = c.execute_query_read_only(sql, 10).await;
        assert!(out.is_err(), "{sql} must be refused, got {out:?}");
    }
    // The engine itself refused these: SQLSTATE 25006 is "read-only SQL
    // transaction", and the role holds the privilege each needs.
    for sql in [
        format!("SELECT nextval('{s}.s')"),
        format!("SELECT {s}.w()"),
        format!("DELETE FROM {s}.t RETURNING *"),
    ] {
        let err = c.execute_query_read_only(&sql, 10).await.unwrap_err();
        assert!(err.to_string().contains("25006"), "{sql}: {err}");
    }

    let count = format!("SELECT count(*) FROM {s}.t");
    assert_eq!(f.scalar(&count).await, CellValue::Number(3.0));
    let gone = format!("SELECT (to_regclass('{s}.u') IS NULL)::text");
    assert_eq!(f.scalar(&gone).await, CellValue::Text("true".into()));
    let seq = format!("SELECT last_value FROM {s}.s");
    assert_eq!(
        f.scalar(&seq).await,
        CellValue::Number(1.0),
        "the sequence never advanced (is_called stays false at 1)"
    );
    let listed = format!("SELECT count(*) FROM pg_ls_dir('/tmp') AS f WHERE f = '{s}.csv'");
    assert_eq!(
        f.scalar(&listed).await,
        CellValue::Number(0.0),
        "COPY wrote {copy_target}"
    );

    // Same connector, after every refusal: no transaction was left open
    // or aborted, on the read-only path or the FSM's temp-table path.
    let again = c
        .execute_query_read_only(&count, 10)
        .await
        .expect("a later read-only query works");
    assert_eq!(again.result.rows[0].0[0], CellValue::Number(3.0));
    assert_eq!(
        scalar(&c, "SELECT current_setting('transaction_read_only')").await,
        CellValue::Text("off".into()),
        "the FSM path runs outside any read-only transaction"
    );
    f.cleanup().await;
}

#[tokio::test]
async fn read_only_reads_on_postgres() {
    let Some(f) = Fixture::new("r").await else {
        return;
    };
    let (c, s) = (f.agent(), f.s.clone());
    let sql = format!("SELECT * FROM {s}.t ORDER BY id");

    let got = c.execute_query_read_only(&sql, 1_000).await.unwrap();
    assert_eq!(got.result.rows.len(), 3);
    assert_eq!(got.result.total_row_count, 3);
    assert!(!got.result.truncated);
    same_shape(&got, &c.execute_query(&sql, 1_000).await.unwrap());

    let cut = c.execute_query_read_only(&sql, 2).await.unwrap();
    assert_eq!(cut.result.rows.len(), 2);
    assert_eq!(cut.result.total_row_count, 3);
    assert!(cut.result.truncated);
    assert_eq!(cut.result.rows[0].0[0], CellValue::Number(1.0));
    same_shape(&cut, &c.execute_query(&sql, 2).await.unwrap());

    // A statement that already starts with WITH, and an empty result.
    let with = format!("WITH x AS (SELECT id FROM {s}.t) SELECT id FROM x WHERE id > 1");
    let with = c.execute_query_read_only(&with, 10).await.unwrap();
    assert_eq!(with.result.total_row_count, 2);
    let none = format!("SELECT * FROM {s}.t WHERE id < 0");
    let empty = c.execute_query_read_only(&none, 10).await.unwrap();
    same_shape(&empty, &c.execute_query(&none, 10).await.unwrap());
    f.cleanup().await;
}

/// Columns, cells, counts and per-column stats as `execute_query` reports
/// them.
fn same_shape(got: &ExecutionResult, want: &ExecutionResult) {
    assert_eq!(got.result.columns, want.result.columns);
    let cells = |r: &ExecutionResult| {
        r.result
            .rows
            .iter()
            .map(|r| r.0.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(cells(got), cells(want));
    assert_eq!(got.result.total_row_count, want.result.total_row_count);
    assert_eq!(got.result.truncated, want.result.truncated);
    assert_eq!(got.summary.row_count, want.summary.row_count);
    assert_eq!(got.summary.columns.len(), want.summary.columns.len());
    for (g, w) in got.summary.columns.iter().zip(&want.summary.columns) {
        assert_eq!(g.name, w.name);
        assert_eq!(g.data_type, w.data_type);
        assert_eq!(g.null_count, w.null_count, "{}", g.name);
        assert_eq!(g.distinct_count, w.distinct_count, "{}", g.name);
        assert_eq!(g.min, w.min, "{}", g.name);
        assert_eq!(g.max, w.max, "{}", g.name);
        assert_eq!(g.mean, w.mean, "{}", g.name);
        assert_eq!(g.std_dev, w.std_dev, "{}", g.name);
    }
}

/// A read-only query dropped mid-statement must not leave the shared
/// client inside its transaction for the next caller.
#[tokio::test]
async fn a_dropped_read_only_query_leaves_no_transaction_open() {
    let Some(f) = Fixture::new("cancel").await else {
        return;
    };
    let c = std::sync::Arc::new(f.agent());
    let marker = f.s.clone();
    let sql = format!("SELECT pg_sleep(30)::text AS slept, '{marker}' AS marker");
    let running = {
        let c = c.clone();
        tokio::spawn(async move { c.execute_query_read_only(&sql, 10).await })
    };
    // Wait on a condition, not a duration: the statement is running on
    // the server, so the transaction is open.
    let probe = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' \
         AND query LIKE '%{marker}%' AND query NOT LIKE '%pg_stat_activity%'"
    );
    let mut seen = false;
    for _ in 0..10_000 {
        if f.scalar(&probe).await == CellValue::Number(1.0) {
            seen = true;
            break;
        }
    }
    assert!(seen, "the read-only statement never started");
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());

    // Had the client survived, this would queue behind pg_sleep(30), then
    // run inside the read-only transaction and fail its temp table.
    assert_eq!(
        scalar(&c, "SELECT current_setting('transaction_read_only')").await,
        CellValue::Text("off".into())
    );
    drop(c);
    f.cleanup().await;
}

/// A session-level advisory lock the agent's statement takes is released
/// after it, so it cannot outlive the query on the memoized client.
#[tokio::test]
async fn an_advisory_lock_taken_by_a_read_only_query_is_released() {
    let Some(f) = Fixture::new("lock").await else {
        return;
    };
    let c = f.agent();
    let key = 100_000 + std::process::id() as i64 % 1_000_000;
    c.execute_query_read_only(&format!("SELECT pg_advisory_lock({key})::text"), 1)
        .await
        .expect("taking an advisory lock is allowed in a read-only transaction");
    let held = format!(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND objid = {key} \
         AND objsubid = 1"
    );
    assert_eq!(
        f.scalar(&held).await,
        CellValue::Number(0.0),
        "the lock is still held"
    );
    f.cleanup().await;
}

/// Each read-only transaction bounds its statement: the defaults are set,
/// and a statement past the timeout is cancelled, leaving the connector
/// usable.
#[tokio::test]
async fn a_read_only_query_is_bounded_by_its_timeouts() {
    let Some(f) = Fixture::new("timeout").await else {
        return;
    };
    let settings = "SELECT current_setting('statement_timeout') || ' ' || \
        current_setting('lock_timeout')";
    let got = f
        .agent()
        .execute_query_read_only(settings, 1)
        .await
        .unwrap();
    assert_eq!(got.result.rows[0].0[0], CellValue::Text("1min 10s".into()));

    let quick = f
        .agent()
        .with_read_only_timeouts(Duration::from_millis(200), Duration::from_millis(100));
    let err = quick
        .execute_query_read_only("SELECT pg_sleep(5)::text", 1)
        .await
        .expect_err("past statement_timeout");
    assert!(err.to_string().contains("57014"), "{err}");
    let after = quick
        .execute_query_read_only(&format!("SELECT count(*) FROM {}.t", f.s), 1)
        .await
        .expect("the connector still works");
    assert_eq!(after.result.rows[0].0[0], CellValue::Number(3.0));
    f.cleanup().await;
}
