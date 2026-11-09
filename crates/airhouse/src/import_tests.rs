//! The load against an in-process DuckDB standing in for the tenant: the same
//! engine Airhouse runs, so the DDL, the casts and the alias names are checked
//! for real. Only DuckLake's catalog and the wire are not modelled.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

struct DuckSink {
    conn: Mutex<duckdb::Connection>,
    /// Fail the n-th INSERT (1-based), to see the load roll back.
    fail_insert: Option<usize>,
    inserts: AtomicUsize,
}

impl DuckSink {
    fn new() -> Self {
        Self {
            conn: Mutex::new(duckdb::Connection::open_in_memory().unwrap()),
            fail_insert: None,
            inserts: AtomicUsize::new(0),
        }
    }

    fn query(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let n = row.as_ref().column_count();
            out.push(
                (0..n)
                    .map(|i| row.get::<_, Option<String>>(i).unwrap())
                    .collect(),
            );
        }
        out
    }

    fn scalar(&self, sql: &str) -> String {
        self.query(sql)[0][0].clone().unwrap_or_default()
    }

    fn table_exists(&self, schema: &str, table: &str) -> bool {
        self.scalar(&format!(
            "SELECT CAST(count(*) AS VARCHAR) FROM information_schema.tables \
             WHERE table_schema = '{schema}' AND table_name = '{table}'"
        )) != "0"
    }
}

#[async_trait]
impl ImportSink for DuckSink {
    async fn execute(&self, sql: &str) -> Result<(), String> {
        if sql.starts_with("INSERT") {
            let n = self.inserts.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_insert == Some(n) {
                return Err(format!("stand-in refused INSERT {n}"));
            }
        }
        self.conn
            .lock()
            .unwrap()
            .execute_batch(sql)
            .map_err(|e| e.to_string())
    }

    async fn count(&self, sql: &str) -> Result<u64, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    }
}

const ORDERS: &str = "id,amount,ordered_on,note\n\
                      1,10.5,2024-01-02,\"plain\"\n\
                      2,,2024-02-03,\"it's, quoted\"\n\
                      3,7.25,2024-03-04,\n";

fn write(dir: &tempfile::TempDir, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn orders(dir: &tempfile::TempDir, if_exists: IfExists) -> ImportFile {
    let path = write(dir, "scratch-upload.csv", ORDERS.as_bytes());
    ImportFile::named_like_a_file_database(path, "orders.csv", if_exists).unwrap()
}

#[tokio::test]
async fn a_csv_lands_with_the_types_a_file_duckdb_gives_it() {
    let dir = tempfile::tempdir().unwrap();
    let sink = DuckSink::new();
    let outcome = load(&sink, &orders(&dir, IfExists::Fail)).await.unwrap();

    assert_eq!(outcome.rows, 3);
    assert_eq!(outcome.table, TableRef::main("orders"));
    let types: Vec<(String, String)> = outcome
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.data_type.clone()))
        .collect();
    assert_eq!(
        types,
        [
            ("id", "BIGINT"),
            ("amount", "DOUBLE"),
            ("ordered_on", "DATE"),
            ("note", "VARCHAR"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    // The table the far side created has those types, not VARCHAR.
    assert_eq!(
        sink.scalar(
            "SELECT data_type FROM information_schema.columns \
             WHERE table_name = 'orders' AND column_name = 'ordered_on'"
        ),
        "DATE"
    );
    assert_eq!(
        sink.query(
            "SELECT CAST(id AS VARCHAR), CAST(amount AS VARCHAR), CAST(ordered_on AS VARCHAR), \
             note FROM main.orders ORDER BY id"
        ),
        vec![
            vec![
                Some("1".into()),
                Some("10.5".into()),
                Some("2024-01-02".into()),
                Some("plain".into())
            ],
            vec![
                Some("2".into()),
                None,
                Some("2024-02-03".into()),
                Some("it's, quoted".into())
            ],
            vec![
                Some("3".into()),
                Some("7.25".into()),
                Some("2024-03-04".into()),
                None
            ],
        ]
    );
}

/// SQL written against a file DuckDB names a file three ways; each resolves.
#[tokio::test]
async fn every_name_a_file_duckdb_gives_the_file_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let sink = DuckSink::new();
    load(&sink, &orders(&dir, IfExists::Fail)).await.unwrap();
    for from in ["orders", "main.orders", "\"orders.csv\"", "orders.csv"] {
        assert_eq!(
            sink.scalar(&format!("SELECT CAST(count(*) AS VARCHAR) FROM {from}")),
            "3",
            "FROM {from}"
        );
    }
}

#[tokio::test]
async fn a_file_larger_than_one_batch_lands_whole() {
    let dir = tempfile::tempdir().unwrap();
    let mut csv = String::from("n,label\n");
    for i in 0..25 {
        csv.push_str(&format!("{i},row {i}\n"));
    }
    let path = write(&dir, "big.csv", csv.as_bytes());
    let file = ImportFile::named_like_a_file_database(path, "big.csv", IfExists::Fail).unwrap();
    let sink = DuckSink::new();
    let outcome = load_with(
        &sink,
        &file,
        Batching {
            max_bytes: usize::MAX,
            max_rows: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(outcome.rows, 25);
    assert_eq!(sink.inserts.load(Ordering::SeqCst), 3, "10 + 10 + 5");
    assert_eq!(
        sink.scalar("SELECT CAST(sum(n) AS VARCHAR) FROM big"),
        (0..25).sum::<i32>().to_string()
    );
}

#[tokio::test]
async fn an_existing_table_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let sink = DuckSink::new();
    sink.execute("CREATE TABLE main.orders (kept INTEGER); INSERT INTO main.orders VALUES (42)")
        .await
        .unwrap();
    let err = load(&sink, &orders(&dir, IfExists::Fail))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ImportError::AlreadyExists(t) if *t == TableRef::main("orders")),
        "{err}"
    );
    assert_eq!(
        sink.scalar("SELECT CAST(kept AS VARCHAR) FROM main.orders"),
        "42"
    );
    assert_eq!(
        sink.inserts.load(Ordering::SeqCst),
        0,
        "the load sent no INSERT"
    );
}

#[tokio::test]
async fn replace_swaps_the_table_and_its_views() {
    let dir = tempfile::tempdir().unwrap();
    let sink = DuckSink::new();
    load(&sink, &orders(&dir, IfExists::Replace)).await.unwrap();
    let path = write(
        &dir,
        "second.csv",
        b"id,amount,ordered_on,note\n9,1,2024-01-01,x\n",
    );
    let again =
        ImportFile::named_like_a_file_database(path, "orders.csv", IfExists::Replace).unwrap();
    load(&sink, &again).await.unwrap();
    assert_eq!(
        sink.scalar("SELECT CAST(count(*) AS VARCHAR) FROM orders.csv"),
        "1"
    );
    assert_eq!(sink.scalar("SELECT CAST(id AS VARCHAR) FROM orders"), "9");
}

#[tokio::test]
async fn a_failed_load_rolls_back_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut sink = DuckSink::new();
    sink.fail_insert = Some(2);
    let mut csv = String::from("n\n");
    for i in 0..5 {
        csv.push_str(&format!("{i}\n"));
    }
    let path = write(&dir, "partial.csv", csv.as_bytes());
    let file = ImportFile::named_like_a_file_database(path, "partial.csv", IfExists::Fail).unwrap();
    let err = load_with(
        &sink,
        &file,
        Batching {
            max_bytes: usize::MAX,
            max_rows: 2,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ImportError::Load(_)), "{err}");
    assert!(!sink.table_exists("main", "partial"));
    assert!(!sink.table_exists("partial", "csv"));
}

#[tokio::test]
async fn parquet_keeps_its_types() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.parquet");
    {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "COPY (SELECT 1::INTEGER AS id, TIMESTAMP '2024-05-06 07:08:09' AS seen_at, \
             [1, 2]::INTEGER[] AS tags, 12.345::DECIMAL(10,3) AS price) \
             TO '{}' (FORMAT parquet)",
            path.display()
        ))
        .unwrap();
    }
    let file =
        ImportFile::named_like_a_file_database(&path, "events.parquet", IfExists::Fail).unwrap();
    assert_eq!(file.format, FileFormat::Parquet);
    let sink = DuckSink::new();
    let outcome = load(&sink, &file).await.unwrap();
    assert_eq!(outcome.rows, 1);
    let types: Vec<&str> = outcome
        .columns
        .iter()
        .map(|c| c.data_type.as_str())
        .collect();
    assert_eq!(
        types,
        ["INTEGER", "TIMESTAMP", "INTEGER[]", "DECIMAL(10,3)"]
    );
    assert_eq!(
        sink.query(
            "SELECT CAST(seen_at AS VARCHAR), CAST(tags AS VARCHAR), CAST(price AS VARCHAR) \
             FROM events.parquet"
        ),
        vec![vec![
            Some("2024-05-06 07:08:09".into()),
            Some("[1, 2]".into()),
            Some("12.345".into())
        ]]
    );
}

#[test]
fn a_file_name_maps_to_the_names_a_file_duckdb_registers() {
    let file = ImportFile::named_like_a_file_database("/tmp/x", "Orders-2024.CSV", IfExists::Fail)
        .unwrap();
    assert_eq!(file.format, FileFormat::Csv);
    assert_eq!(file.target, TableRef::main("orders_2024"));
    assert_eq!(
        file.aliases,
        vec![
            TableRef::main("Orders-2024.CSV"),
            TableRef::new("Orders-2024", "csv")
        ]
    );
    assert_eq!(table_name_for_stem("2024 sales"), "_2024_sales");
    assert_eq!(table_name_for_stem("---"), "unnamed");
}

#[tokio::test]
async fn names_another_owner_holds_are_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "f.csv", b"a\n1\n");
    let sink = DuckSink::new();
    for name in ["app_store_ops.csv", "preview_x.csv", "airhouse_meta.csv"] {
        let err = ImportFile::named_like_a_file_database(&path, name, IfExists::Fail).unwrap_err();
        assert!(matches!(err, ImportError::InvalidName(_)), "{name}: {err}");
    }
    // A file built by hand is checked again at load.
    let mut file =
        ImportFile::named_like_a_file_database(&path, "fine.csv", IfExists::Fail).unwrap();
    file.target = TableRef::new("app_store_ops", "fine");
    let err = load(&sink, &file).await.unwrap_err();
    assert!(matches!(err, ImportError::InvalidName(_)), "{err}");
    assert_eq!(sink.inserts.load(Ordering::SeqCst), 0);
    assert!(
        ImportFile::named_like_a_file_database(&path, "notes.txt", IfExists::Fail).is_err(),
        "only csv and parquet load"
    );
}

#[tokio::test]
async fn an_unreadable_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "broken.parquet", b"this is not parquet");
    let file =
        ImportFile::named_like_a_file_database(path, "broken.parquet", IfExists::Fail).unwrap();
    let sink = DuckSink::new();
    let err = load(&sink, &file).await.unwrap_err();
    assert!(matches!(err, ImportError::Read(_)), "{err}");
    assert!(!sink.table_exists("main", "broken"));
}

/// `read_parquet` must not depend on installing the extension at run time: a
/// pod has no `~/.duckdb` cache and may have no egress, so an autoloaded
/// reader failed there (`could not read the file: out is null`) while CSV,
/// which is built in, loaded. The `parquet` feature on the workspace's
/// `duckdb` links it in; this fails if that feature is dropped.
#[test]
fn the_parquet_reader_is_linked_in_not_downloaded() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    let mode: String = conn
        .query_row(
            "SELECT install_mode FROM duckdb_extensions() WHERE extension_name = 'parquet'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mode, "STATICALLY_LINKED");
}

/// `examples/.db/stores.parquet`, as an upload delivers it (streamed to a
/// scratch file whose name has no extension), lands with every name the
/// example views use.
#[tokio::test]
async fn the_example_stores_parquet_loads_from_a_scratch_file() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/.db/stores.parquet");
    let dir = tempfile::tempdir().unwrap();
    let scratch = write(
        &dir,
        "oxy-airhouse-import-abc123",
        &std::fs::read(fixture).unwrap(),
    );
    let file =
        ImportFile::named_like_a_file_database(scratch, "stores.parquet", IfExists::Fail).unwrap();
    let sink = DuckSink::new();
    let outcome = load(&sink, &file).await.unwrap();
    assert_eq!(outcome.rows, 24);
    assert_eq!(
        sink.scalar(
            "SELECT CAST(count(DISTINCT city) AS VARCHAR) || '/' || \
             CAST(count(DISTINCT region) AS VARCHAR) FROM main.stores"
        ),
        "12/4"
    );
    for from in [
        "main.stores",
        "\"stores.parquet\"",
        "'stores.parquet'",
        "stores.parquet",
    ] {
        assert_eq!(
            sink.scalar(&format!("SELECT CAST(count(*) AS VARCHAR) FROM {from}")),
            "24",
            "FROM {from}"
        );
    }
}
