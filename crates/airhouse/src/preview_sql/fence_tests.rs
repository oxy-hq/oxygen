//! The edges of both preview fences: names DuckDB reads as files, SQL too
//! deep to walk on an ordinary stack, transactions, sequences, `DESCRIBE`,
//! and `<catalog>.t`.

use super::*;

const KEY: &str = "feat_je_v2_92a1b7";
const CATALOG: Option<&str> = Some("lake");

fn ns() -> PreviewNamespace {
    PreviewNamespace::from_key(KEY).unwrap()
}

fn opts() -> RewriteOptions {
    RewriteOptions {
        catalog: Some("lake".into()),
        read_live_only: false,
    }
}

fn p(live: &str) -> String {
    format!("\"preview_{KEY}__{live}\"")
}

fn shadow(schema: &str, table: &str) -> ShadowMap {
    let mut map = ShadowMap::default();
    map.0
        .insert((schema.into(), table.into()), ShadowState::Shadow);
    map
}

/// Refused by the rewrite, the read overlay and the verifier alike.
fn refused_everywhere(sql: &str) -> String {
    let empty = ShadowMap::default();
    let rewritten = rewrite(sql, &ns(), &empty, &opts());
    assert!(rewritten.is_err(), "the rewrite passed {sql}");
    let overlaid = overlay_reads(sql, &ns(), &empty, &opts());
    assert!(overlaid.is_err(), "the overlay passed {sql}");
    match verify(sql, &ns(), CATALOG) {
        Ok(_) => panic!("the verifier passed {sql}"),
        Err(Refused(msg)) => msg,
    }
}

#[test]
fn names_duckdb_reads_as_files_are_refused() {
    for sql in [
        "SELECT * FROM 's3://lake-bucket/other_tenant/x.parquet'",
        "SELECT * FROM '/etc/passwd'",
        "SELECT * FROM 'orders'",
        "SELECT * FROM \"s3://lake-bucket/other_tenant/x.parquet\"",
        "SELECT * FROM \"x.parquet\"",
        "SELECT * FROM x.parquet",
        "SELECT * FROM toast_pos.csv",
        "SELECT o.* FROM toast_pos.orders o JOIN \"/tmp/y.csv\" y USING (id)",
        "DESCRIBE x.parquet",
        "DESCRIBE 's3://lake-bucket/x.json'",
    ] {
        let msg = refused_everywhere(sql);
        assert!(msg.contains("looks like a file"), "{sql}: {msg}");
    }
    let write = "INSERT INTO toast_pos.daily SELECT * FROM 's3://b/x.csv'";
    assert!(rewrite(write, &ns(), &ShadowMap::default(), &opts()).is_err());
    // Tables that merely resemble a word stay readable.
    for sql in [
        "SELECT * FROM toast_pos.csv_exports",
        "SELECT * FROM parquet",
    ] {
        verify(sql, &ns(), CATALOG).unwrap();
    }
}

#[test]
fn sql_too_deep_for_a_default_stack_is_refused_or_handled_never_aborts() {
    let empty = ShadowMap::default();
    for terms in [20_000, 50_000, 100_000] {
        let sql = format!("SELECT {}", vec!["1"; terms].join("+"));
        let _ = rewrite(&sql, &ns(), &empty, &opts());
        let _ = overlay_reads(&sql, &ns(), &empty, &opts());
        let verified = verify(&sql, &ns(), CATALOG);
        assert_eq!(verified.is_ok(), terms <= 50_000, "{terms} terms");
    }
    let unions = format!("SELECT 1 {}", vec!["UNION ALL SELECT 1"; 20_000].join(" "));
    assert!(verify(&unions, &ns(), CATALOG).is_ok());
    let over = format!("SELECT {}", vec!["1"; 100_000].join("+"));
    let msg = verify(&over, &ns(), CATALOG).unwrap_err().0;
    assert!(msg.contains("nests"), "{msg}");
    // Size is not depth: a 2 MiB literal is one token.
    let long = format!("SELECT '{}'", "x".repeat(2 << 20));
    verify(&long, &ns(), CATALOG).unwrap();
}

#[test]
fn a_transaction_must_open_and_close_inside_the_batch() {
    let empty = ShadowMap::default();
    for sql in [
        "BEGIN",
        "BEGIN; DELETE FROM toast_pos.daily",
        "COMMIT",
        "DELETE FROM toast_pos.daily; ROLLBACK",
        "BEGIN; BEGIN; COMMIT; COMMIT",
        "BEGIN; SELECT 1; COMMIT; COMMIT",
    ] {
        assert!(rewrite(sql, &ns(), &empty, &opts()).is_err(), "{sql}");
        let msg = verify(sql, &ns(), CATALOG).unwrap_err().0;
        assert!(
            msg.contains("transaction") || msg.contains("BEGIN"),
            "{sql}: {msg}"
        );
    }
    let balanced = "BEGIN; DELETE FROM toast_pos.daily; COMMIT; BEGIN; ROLLBACK";
    let out = rewrite(balanced, &ns(), &empty, &opts()).unwrap();
    verify(&out.sql, &ns(), CATALOG).unwrap();
}

#[test]
fn advancing_a_sequence_is_refused() {
    for sql in [
        "SELECT nextval('toast_pos.order_seq')",
        "INSERT INTO toast_pos.daily SELECT nextval('s'), 1",
        "CREATE TABLE toast_pos.t (id BIGINT DEFAULT nextval('s'))",
    ] {
        assert!(
            rewrite(sql, &ns(), &ShadowMap::default(), &opts()).is_err(),
            "{sql}"
        );
        assert!(verify(sql, &ns(), CATALOG).is_err(), "{sql}");
    }
}

#[test]
fn write_targets_named_like_file_types_are_tables() {
    // DuckDB reads files only where it reads tables; a write target named
    // `json` is a table named json.
    let empty = ShadowMap::default();
    for sql in [
        "INSERT INTO toast_pos.json VALUES (1)",
        "INSERT INTO \"toast_pos\".\"json\" VALUES (1)",
        "UPDATE toast_pos.json SET a = 1",
        "DELETE FROM toast_pos.parquet WHERE a = 1",
        "MERGE INTO toast_pos.json t USING toast_pos.orders o ON t.id = o.id \
         WHEN MATCHED THEN DELETE",
        "CREATE TABLE toast_pos.csv (id INT)",
        "ALTER TABLE toast_pos.csv ADD COLUMN note VARCHAR",
    ] {
        let out = rewrite(sql, &ns(), &empty, &opts())
            .unwrap_or_else(|e| panic!("the rewrite refused {sql}: {e}"));
        verify(&out.sql, &ns(), CATALOG)
            .unwrap_or_else(|e| panic!("the verifier refused {}: {e}", out.sql));
    }
    // Read, the same name is still refused: no table may be found by it.
    for sql in [
        "SELECT * FROM x.parquet",
        "SELECT * FROM toast_pos.json",
        "DELETE FROM toast_pos.t USING toast_pos.json j WHERE t.id = j.id",
        "UPDATE toast_pos.t SET a = 1 FROM toast_pos.csv c WHERE t.id = c.id",
    ] {
        let msg = rewrite(sql, &ns(), &empty, &opts()).unwrap_err().0;
        assert!(msg.contains("looks like a file"), "{sql}: {msg}");
    }
    let target = format!("{}.\"t\"", p("toast_pos"));
    for sql in [
        "SELECT * FROM toast_pos.json".to_string(),
        format!("DELETE FROM {target} USING toast_pos.json j WHERE true"),
        format!("UPDATE {target} SET a = 1 FROM toast_pos.csv c WHERE true"),
    ] {
        let msg = verify(&sql, &ns(), CATALOG).unwrap_err().0;
        assert!(msg.contains("looks like a file"), "{sql}: {msg}");
    }
}

#[test]
fn describe_reads_the_previews_copy() {
    let shadow = shadow("toast_pos", "orders");
    let sql = "DESCRIBE toast_pos.orders";
    let expected = format!("DESCRIBE {}.\"orders\"", p("toast_pos"));
    assert_eq!(
        overlay_reads(sql, &ns(), &shadow, &opts()).unwrap(),
        expected
    );
    let out = rewrite(sql, &ns(), &shadow, &opts()).unwrap();
    assert_eq!(out.sql, expected);
    assert_eq!(
        out.redirected_reads,
        vec![("toast_pos".into(), "orders".into())]
    );
    let live = overlay_reads("DESCRIBE toast_pos.locations", &ns(), &shadow, &opts());
    assert_eq!(live.unwrap(), "DESCRIBE toast_pos.locations");
    let columns = "SHOW COLUMNS FROM toast_pos.orders";
    let overlaid = overlay_reads(columns, &ns(), &shadow, &opts()).unwrap();
    assert!(
        overlaid.ends_with(&format!("FROM {}.\"orders\"", p("toast_pos"))),
        "{overlaid}"
    );
    assert!(overlay_reads("SHOW COLUMNS FROM x.parquet", &ns(), &shadow, &opts()).is_err());
}

#[test]
fn catalog_dot_table_is_main_for_writes_and_reads_alike() {
    // DuckDB binds `<catalog>.t` as `<catalog>.main.t`; so does the preview.
    let sql = "INSERT INTO lake.daily VALUES (1);
               SELECT * FROM daily; SELECT * FROM main.daily; SELECT * FROM lake.daily";
    let out = rewrite(sql, &ns(), &ShadowMap::default(), &opts()).unwrap();
    assert_eq!(out.writes, vec![("main".into(), "daily".into())]);
    let copy = format!("{}.\"daily\"", p("main"));
    let statements: Vec<&str> = out.sql.split(";\n").collect();
    assert_eq!(statements[0], format!("INSERT INTO lake.{copy} VALUES (1)"));
    assert_eq!(statements[1], format!("SELECT * FROM {copy}"));
    assert_eq!(statements[2], format!("SELECT * FROM {copy}"));
    assert_eq!(statements[3], format!("SELECT * FROM lake.{copy}"));
    verify(&out.sql, &ns(), CATALOG).unwrap();
    // Read-only, the same resolution: `lake.daily` is `main.daily`.
    let read = overlay_reads(
        "SELECT * FROM lake.daily",
        &ns(),
        &shadow("main", "daily"),
        &opts(),
    );
    assert_eq!(read.unwrap(), format!("SELECT * FROM lake.{copy}"));
}

#[test]
fn a_write_reading_its_own_target_reads_the_copy_not_every_live_row() {
    let out = rewrite(
        "INSERT INTO toast_pos.sales_daily SELECT * FROM toast_pos.sales_daily",
        &ns(),
        &ShadowMap::default(),
        &opts(),
    )
    .unwrap();
    let copy = format!("{}.\"sales_daily\"", p("toast_pos"));
    assert_eq!(out.sql, format!("INSERT INTO {copy} SELECT * FROM {copy}"));
    assert!(
        out.preludes
            .iter()
            .any(|p| matches!(p, Prelude::CopyOnWrite { .. }))
    );
}

#[test]
fn reads_prefer_the_previews_copy_unless_told_to_read_live() {
    let mut shadow = shadow("main", "scratch");
    shadow
        .0
        .insert(("toast_pos".into(), "orders".into()), ShadowState::Sample);
    let sql = "SELECT * FROM toast_pos.orders o JOIN toast_pos.locations l USING (location_id) \
               JOIN scratch s USING (location_id)";
    let out = rewrite(sql, &ns(), &shadow, &opts()).unwrap();
    assert!(
        out.sql.contains(&format!("{}.\"orders\"", p("toast_pos"))),
        "{}",
        out.sql
    );
    assert!(out.sql.contains("toast_pos.locations"), "{}", out.sql);
    assert!(
        out.sql.contains(&format!("{}.\"scratch\"", p("main"))),
        "{}",
        out.sql
    );
    let redirected: Vec<(String, String)> = vec![
        ("toast_pos".into(), "orders".into()),
        ("main".into(), "scratch".into()),
    ];
    assert_eq!(out.redirected_reads, redirected);
    let live_only = RewriteOptions {
        read_live_only: true,
        ..opts()
    };
    let out_live = rewrite(sql, &ns(), &shadow, &live_only).unwrap();
    assert!(!out_live.sql.contains("preview_"), "{}", out_live.sql);
    let once = overlay_reads(sql, &ns(), &shadow, &opts()).unwrap();
    assert_eq!(once, out.sql);
    assert_eq!(overlay_reads(&once, &ns(), &shadow, &opts()).unwrap(), once);
    assert!(overlay_reads("DELETE FROM toast_pos.orders", &ns(), &shadow, &opts()).is_err());
}

#[test]
fn engine_and_session_statements_are_refused() {
    for sql in [
        "ATTACH 'x.db' AS x",
        "DETACH x",
        "USE toast_pos",
        "SET search_path = 'toast_pos'",
        "COPY toast_pos.orders TO 's3://bucket/x.parquet'",
        "CALL ducklake_expire_snapshots('lake')",
        "PRAGMA database_list",
        "INSTALL httpfs",
        "LOAD httpfs",
        "CREATE MACRO m(x) AS x + 1",
        "CREATE SEQUENCE toast_pos.s",
        "SELECT 1 INTO toast_pos.t",
        "EXPLAIN ANALYZE DELETE FROM toast_pos.orders",
        "",
        "SELEKT frm",
    ] {
        refused_everywhere(sql);
    }
}

/// `EXPLAIN ANALYZE` runs its body, and a statement's role is read off the
/// outer statement, so explaining anything but a query is refused — even a
/// write the preview owns, which would otherwise be sent as a read.
#[test]
fn verify_refuses_explain_of_anything_but_a_query() {
    for sql in [
        format!("EXPLAIN ANALYZE DROP SCHEMA {} CASCADE", p("toast_pos")),
        format!(
            "EXPLAIN ANALYZE INSERT INTO {}.t VALUES (1)",
            p("toast_pos")
        ),
        format!("EXPLAIN DELETE FROM {}.t", p("toast_pos")),
        format!("EXPLAIN ANALYZE CREATE SCHEMA {}", p("scratch")),
        format!("EXPLAIN ANALYZE UPDATE {}.t SET id = 2", p("toast_pos")),
    ] {
        let Err(Refused(msg)) = verify(&sql, &ns(), CATALOG) else {
            panic!("the verifier passed {sql}");
        };
        assert!(
            msg.contains("only a query may be explained"),
            "{sql}: {msg}"
        );
    }
    let read = format!("EXPLAIN ANALYZE SELECT * FROM {}.t", p("toast_pos"));
    let roles = verify_statements(&read, &ns(), CATALOG).unwrap();
    assert_eq!(roles[0].role, StatementRole::Read);
}

/// The connector scopes its Writer from these roles, so each must name
/// exactly the preview schemas its statement writes.
#[test]
fn verify_statements_names_the_schemas_each_statement_writes() {
    let (pos, site) = (
        format!("preview_{KEY}__toast_pos"),
        format!("preview_{KEY}__site_selection"),
    );
    let sql = format!(
        "BEGIN; \
         SELECT * FROM toast_pos.orders; \
         INSERT INTO {} SELECT * FROM toast_pos.orders; \
         CREATE TABLE lake.{}.cells AS SELECT 1 AS id; \
         ALTER TABLE {}.cells RENAME TO cells_v2; \
         UPDATE {} SET id = 2 FROM {} AS c WHERE c.id = 1; \
         CREATE SCHEMA IF NOT EXISTS {}; \
         DROP SCHEMA lake.{}; \
         COMMIT",
        format_args!("{}.orders", p("toast_pos")),
        p("site_selection"),
        p("site_selection"),
        format_args!("{}.orders", p("toast_pos")),
        format_args!("{}.cells_v2", p("site_selection")),
        p("toast_pos"),
        p("site_selection"),
    );
    let roles: Vec<StatementRole> = verify_statements(&sql, &ns(), CATALOG)
        .unwrap()
        .into_iter()
        .map(|v| v.role)
        .collect();
    assert_eq!(
        roles,
        vec![
            StatementRole::Begin,
            StatementRole::Read,
            StatementRole::Write(vec![pos.clone()]),
            StatementRole::Write(vec![site.clone()]),
            StatementRole::Write(vec![site.clone()]),
            StatementRole::Write(vec![pos.clone()]),
            StatementRole::SchemaDdl(vec![pos.clone()]),
            StatementRole::SchemaDdl(vec![site.clone()]),
            StatementRole::End,
        ]
    );
    // What `verify` returns is exactly these statements' text.
    let texts: Vec<String> = verify_statements(&sql, &ns(), CATALOG)
        .unwrap()
        .into_iter()
        .map(|v| v.sql)
        .collect();
    assert_eq!(texts, verify(&sql, &ns(), CATALOG).unwrap());
    // And each write names the relations it writes; a rename both names.
    let rel = |s: &String, t: &str| (s.clone(), t.to_string());
    let relations: Vec<Vec<(String, String)>> = verify_statements(&sql, &ns(), CATALOG)
        .unwrap()
        .into_iter()
        .map(|v| v.relations)
        .collect();
    assert_eq!(relations[1], vec![], "a read writes nothing");
    assert_eq!(relations[2], vec![rel(&pos, "orders")]);
    assert_eq!(relations[3], vec![rel(&site, "cells")]);
    assert_eq!(
        relations[4],
        vec![rel(&site, "cells"), rel(&site, "cells_v2")]
    );
    assert_eq!(relations[5], vec![rel(&pos, "orders")]);
}
