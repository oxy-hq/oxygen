//! The backstop on the real bundled DuckDB (see `read_only_backstop`).

use std::path::Path;

use agentic_connector::{DatabaseConnector, DuckDbConnection, DuckDbConnector, LoadStrategy};
use agentic_core::result::CellValue;

const CSV: &str = "id,name\n1,a\n2,b\n3,c\n";

/// `<root>/data/t.csv` and an empty `<root>/out`.
fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let out = root.path().join("out");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(data.join("t.csv"), CSV).unwrap();
    (root, data, out)
}

fn writes(data: &Path, out: &Path) -> Vec<String> {
    let (data, out) = (data.display(), out.display());
    vec![
        "DELETE FROM t".into(),
        "CREATE TABLE u AS SELECT 1 AS x".into(),
        format!("COPY (SELECT 1) TO '{out}/x.csv'"),
        format!("COPY (SELECT 1) TO '{data}/t.csv'"),
        format!("ATTACH '{out}/y.db'"),
        "SELECT * FROM enable_logging()".into(),
        "SET enable_external_access = true".into(),
        "RESET enable_external_access".into(),
        "SELECT 1; DELETE FROM t".into(),
    ]
}

async fn refuses_writes(c: &DuckDbConnector, data: &Path, out: &Path) {
    assert!(c.supports_read_only());
    for sql in writes(data, out) {
        let got = c.execute_query_read_only(&sql, 10).await;
        assert!(got.is_err(), "{sql} must be refused, got {got:?}");
    }
    assert!(!out.join("x.csv").exists(), "COPY wrote x.csv");
    assert!(!out.join("y.db").exists(), "ATTACH created y.db");
    assert_eq!(std::fs::read_to_string(data.join("t.csv")).unwrap(), CSV);

    // The view over the CSV still reads, on the same connector.
    let rows = c
        .execute_query_read_only("SELECT * FROM t ORDER BY id", 10)
        .await
        .expect("a view over the agent's file reads");
    assert_eq!(rows.result.total_row_count, 3);
    assert_eq!(rows.result.rows[2].0[1], CellValue::Text("c".into()));
    // Reading a file outside the registered ones is not a backdoor.
    let outside = out.join("other.csv");
    std::fs::write(&outside, CSV).unwrap();
    let outside = format!("SELECT * FROM read_csv('{}')", outside.display());
    assert!(c.execute_query_read_only(&outside, 10).await.is_err());
}

#[tokio::test]
async fn read_only_refuses_writes_on_duckdb() {
    for strategy in [LoadStrategy::View, LoadStrategy::Materialized] {
        let (_root, data, out) = dirs();
        let c = DuckDbConnector::from_directory(&data, strategy).unwrap();
        refuses_writes(&c, &data, &out).await;
        // `t` is untouched where it is a table the rollback protects.
        let n = c.execute_query("SELECT count(*) FROM t", 1).await.unwrap();
        assert_eq!(n.result.rows[0].0[0], CellValue::Number(3.0));
    }
}

/// On a cached directory database the lock must not reach the clones
/// other runs read: their external access stays on.
#[tokio::test]
async fn read_only_refuses_writes_on_a_cached_duckdb_without_locking_its_clones() {
    let (_root, data, out) = dirs();
    let c = DuckDbConnector::from_directory_cached(&data, LoadStrategy::View).unwrap();
    refuses_writes(&c, &data, &out).await;

    let other = DuckDbConnector::from_directory_cached(&data, LoadStrategy::View).unwrap();
    let setting = other
        .execute_query(
            "SELECT current_setting('enable_external_access')::VARCHAR",
            1,
        )
        .await
        .unwrap();
    assert_eq!(setting.result.rows[0].0[0], CellValue::Text("true".into()));
    // `c` itself keeps its full-access session for everything else.
    let own = c
        .execute_query(
            "SELECT current_setting('enable_external_access')::VARCHAR",
            1,
        )
        .await
        .unwrap();
    assert_eq!(own.result.rows[0].0[0], CellValue::Text("true".into()));
}

/// A connection handed in from outside may be shared (the app's pooled
/// local database is), so the connector cannot lock it: no backstop.
#[tokio::test]
async fn a_duckdb_connector_over_a_handed_in_connection_has_no_read_only_path() {
    let c = DuckDbConnector::new(DuckDbConnection::open_in_memory().unwrap());
    assert!(!c.supports_read_only());
    let err = c.execute_query_read_only("SELECT 1", 1).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "read-only execution is not supported by DuckDB"
    );
}

/// The statement check binds the agent's SQL before anything runs, and
/// binding is not inert: it opens files and URLs a query names. So that
/// check must be as closed as the database: a URL gets no connection, and an
/// outside path is refused for access, not reported missing.
#[tokio::test]
async fn binding_the_agent_sql_touches_no_url_and_no_outside_file() {
    let (_root, data, out) = dirs();
    let c = DuckDbConnector::from_directory(&data, LoadStrategy::View).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("SELECT * FROM read_csv('http://127.0.0.1:{port}/x.csv')");
    assert!(c.execute_query_read_only(&url, 1).await.is_err());
    match listener.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the check reached the URL: {other:?}"),
    }

    let missing = out.join("missing.csv");
    let sql = format!("SELECT * FROM read_csv('{}')", missing.display());
    let err = c.execute_query_read_only(&sql, 1).await.unwrap_err();
    assert!(
        err.to_string().contains("disabled by configuration"),
        "an outside path must be refused for access, never looked up: {err}"
    );
}
