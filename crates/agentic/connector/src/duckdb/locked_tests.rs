//! The lock and the rollback on their own: each case runs on the locked
//! connection directly, past [`admit`](super::admit), so what is refused here
//! is refused by DuckDB's configuration, not by a statement check.

use std::path::{Path, PathBuf};

use super::{LockedDb, rolled_back};
use crate::duckdb::LoadStrategy;

const CSV: &str = "id,name\n1,a\n2,b\n3,c\n";

struct Dirs {
    _root: tempfile::TempDir,
    data: PathBuf,
    out: PathBuf,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let out = root.path().join("out");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(data.join("t.csv"), CSV).unwrap();
    std::fs::write(out.join("other.csv"), CSV).unwrap();
    // A Parquet view too: the loader prefers Parquet, and the lock must keep
    // it readable.
    let parquet = data.join("p.parquet").display().to_string();
    duckdb::Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES (1), (2), (3)) v(id)) TO '{parquet}' (FORMAT parquet)"
        ))
        .unwrap();
    Dirs {
        data: data.canonicalize().unwrap(),
        out: out.canonicalize().unwrap(),
        _root: root,
    }
}

fn run(db: &LockedDb, sql: &str) -> Result<(), String> {
    db.with_conn(|c| c.execute_batch(sql).map_err(|e| e.to_string()))
}

fn count(db: &LockedDb) -> i64 {
    db.with_conn(|c| c.query_row("SELECT count(*) FROM t", [], |r| r.get(0)))
        .unwrap()
}

fn lit(p: &Path) -> String {
    format!("'{}'", p.display())
}

#[test]
fn the_lock_refuses_files_and_settings_while_views_read() {
    for strategy in [LoadStrategy::View, LoadStrategy::Materialized] {
        let d = dirs();
        let db = LockedDb::build(&d.data, strategy).unwrap();
        let x = d.out.join("x.csv");
        let y = d.out.join("y.db");
        let in_data = d.data.join("z.csv");
        for sql in [
            format!("COPY (SELECT 1) TO {}", lit(&x)),
            format!("COPY (SELECT 1) TO {}", lit(&in_data)),
            format!("ATTACH {}", lit(&y)),
            format!("SELECT * FROM read_csv({})", lit(&d.out.join("other.csv"))),
            "SET enable_external_access = true".into(),
            "RESET enable_external_access".into(),
            format!("SET allowed_paths = [{}]", lit(&x)),
            format!("SET allowed_directories = [{}]", lit(&d.out)),
            "SET lock_configuration = false".into(),
            "RESET lock_configuration".into(),
        ] {
            assert!(run(&db, &sql).is_err(), "{strategy:?}: `{sql}` ran");
        }
        assert!(!x.exists() && !y.exists() && !in_data.exists());
        assert_eq!(count(&db), 3, "{strategy:?}: the view or table still reads");
        let parquet: i64 = db
            .with_conn(|c| c.query_row("SELECT count(*) FROM p", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(
            parquet, 3,
            "{strategy:?}: the Parquet view or table still reads"
        );
    }
}

/// `COPY … TO` the very file a view reads, the one path the lock lists. The
/// engine refuses it today only because `COPY` writes a temp file beside it
/// first, which is not listed; with `USE_TMP_FILE false` it is refused only by
/// the statement check, before anything reaches the engine.
#[test]
fn copy_onto_a_view_file() {
    let d = dirs();
    let db = LockedDb::build(&d.data, LoadStrategy::View).unwrap();
    let source = d.data.join("t.csv");
    let copy = format!("COPY (SELECT 9 AS id, 'z' AS name) TO {}", lit(&source));
    let direct = format!("{copy} (USE_TMP_FILE false)");
    for sql in [&copy, &direct] {
        assert!(db.query(sql, 10).is_err(), "`{sql}` was admitted");
    }
    assert_eq!(std::fs::read_to_string(&source).unwrap(), CSV);

    assert!(
        run(&db, &copy).is_err(),
        "the temp file beside it is not listed"
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), CSV);
    let engine = run(&db, &direct);
    assert!(
        engine.is_ok(),
        "DuckDB now refuses a listed file too: the lock covers it, say so in the docs: {engine:?}"
    );
    assert_ne!(std::fs::read_to_string(&source).unwrap(), CSV);
}

#[test]
fn a_write_inside_the_transaction_is_rolled_back() {
    let d = dirs();
    let db = LockedDb::build(&d.data, LoadStrategy::Materialized).unwrap();
    let (deleted, rollback) = db.with_conn(|c| {
        rolled_back(c, |c| {
            c.execute_batch("DELETE FROM t; CREATE TABLE u AS SELECT 1 AS x")
                .map_err(|e| crate::connector::ConnectorError::Other(e.to_string()))
        })
    });
    deleted.unwrap();
    rollback.unwrap();
    assert_eq!(count(&db), 3);
    assert!(run(&db, "SELECT * FROM u").is_err(), "u was rolled back");
}

#[test]
fn query_admits_one_select_and_samples_it() {
    let d = dirs();
    let db = LockedDb::build(&d.data, LoadStrategy::View).unwrap();
    let got = db.query("SELECT * FROM t ORDER BY id;", 2).unwrap();
    assert_eq!(got.result.total_row_count, 3);
    assert_eq!(got.result.rows.len(), 2);
    assert!(got.result.truncated);
    assert_eq!(got.summary.columns.len(), 2);
    for sql in [
        "SELECT 1; SELECT 2",
        "SELECT 1 /* /* */ ; DELETE FROM t; /* */",
        "SELECT * FROM enable_logging()",
        "PRAGMA version",
        "CHECKPOINT",
    ] {
        assert!(db.query(sql, 10).is_err(), "`{sql}` was admitted");
    }
}
