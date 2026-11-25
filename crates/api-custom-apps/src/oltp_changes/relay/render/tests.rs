//! Unit tests of the statements, and the statements run against an
//! in-memory DuckDB, which is what a workspace's Airhouse runs.

use chrono::{DateTime, Utc};
use duckdb::Connection;
use oxy_oltp::changes::Column;
use serde_json::value::RawValue;

use super::{
    ShipRow, Unrenderable, changes_table_name, columns_sql, ensure_schema_ddl, insert_sql, literal,
    parse_image, progress_read_sql, progress_write_sql, table_ddl,
};
use crate::oltp_changes::relay::table::{ChangeTable, DuckType, ShapeVersion};

mod chunked;
mod typed;

use typed::{TYPED, TZ, typed_rows, typed_texts};

const WRITER: &str = "shop";
const INVOCATION: &str = "6f1c2e8a-0000-4000-8000-000000000001";

fn plan(table: &str, versions: &[(i32, &[(i16, &str, &str)])]) -> ChangeTable {
    let versions: Vec<ShapeVersion> = versions
        .iter()
        .map(|(schema_version, columns)| ShapeVersion {
            schema_version: *schema_version,
            captured: columns
                .iter()
                .map(|(attnum, name, ty)| Column {
                    attnum: *attnum,
                    name: name.to_string(),
                    type_name: ty.to_string(),
                })
                .collect(),
        })
        .collect();
    ChangeTable::plan(table, &versions).expect("a plan")
}

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).expect("a time")
}

/// A row of generation 1 captured under `schema_version`, its image as
/// Postgres writes it (`row_image::text`); none for a truncate.
fn row(seq: i64, op: &'static str, schema_version: i32, image: Option<&str>) -> ShipRow {
    ShipRow {
        generation: 1,
        seq,
        txid: 900 + seq as u64,
        op,
        changed_at: at(1_780_000_000 + seq),
        actor: "alice".into(),
        invocation: None,
        schema_version,
        image: image.map(|text| parse_image(text).expect("an image")),
    }
}

fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_string()).expect("JSON")
}

/// An in-memory DuckDB with `WRITER`'s schema and `_progress`.
fn duck() -> Connection {
    let conn = Connection::open_in_memory().expect("an in-memory DuckDB");
    run(&conn, &ensure_schema_ddl(WRITER));
    conn
}

fn run(conn: &Connection, statements: &[String]) {
    for sql in statements {
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("{e}\n{sql}"));
    }
}

/// `rows` of `generation` into `table`'s change table.
fn ship(conn: &Connection, table: &ChangeTable, generation: i64, rows: &[ShipRow]) {
    run(
        conn,
        &[insert_sql(WRITER, table, generation, rows).expect("renders")],
    );
}

/// Every row of `sql`'s first `width` columns, as text.
fn texts(conn: &Connection, sql: &str, width: usize) -> Vec<Vec<Option<String>>> {
    let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{e}\n{sql}"));
    let rows = stmt
        .query_map([], |row| {
            (0..width)
                .map(|i| row.get::<_, Option<String>>(i))
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("a query");
    rows.collect::<Result<_, _>>().expect("rows")
}

fn text(v: &str) -> Option<String> {
    Some(v.to_string())
}

#[test]
fn literals_escape_quotes() {
    let quoted = literal(&raw(r#""o'brien's""#), &DuckType::Varchar).expect("renders");
    assert_eq!(quoted, "CAST('o''brien''s' AS VARCHAR)");
    let dates = literal(&raw(r#"["2026-11-24", null]"#), &DuckType::of("date[]"));
    let both = "CAST([CAST('2026-11-24' AS DATE), CAST(NULL AS DATE)] AS DATE[])";
    assert_eq!(dates.as_deref(), Ok(both));
    let doc = literal(&raw(r#"{"k": "it's"}"#), &DuckType::Json).expect("renders");
    assert_eq!(doc, r#"CAST('{"k": "it''s"}' AS VARCHAR)"#);
    let table = plan("notes", &[(1, &[(1, "body", "text")])]);
    let mut quoted = row(1, "insert", 1, Some(r#"{"body": "x"}"#));
    quoted.actor = "o'brien".into();
    let insert = insert_sql(WRITER, &table, 1, &[quoted]).expect("renders");
    assert!(insert.contains("CAST('o''brien' AS VARCHAR)"), "{insert}");
}

#[test]
fn a_string_in_an_integer_column_is_unrenderable() {
    assert!(literal(&raw(r#""7""#), &DuckType::Integer).is_err());
    let columns: &[(i16, &str, &str)] = &[(1, "id", "integer"), (2, "units", "integer")];
    let table = plan("leases", &[(1, columns)]);
    let rows = [
        row(1, "insert", 1, Some(r#"{"id": 1, "units": 3}"#)),
        row(2, "update", 1, Some(r#"{"id": 1, "units": "three"}"#)),
    ];
    let err = insert_sql(WRITER, &table, 1, &rows).unwrap_err();
    assert_eq!((err.seq, err.column.as_str()), (2, "units"));
    assert!(err.reason.contains("INTEGER"), "{err}");
    // A key its shape did not capture is refused too, not dropped.
    let stray = [row(3, "insert", 1, Some(r#"{"id": 2, "ssn": "x"}"#))];
    let err: Unrenderable = insert_sql(WRITER, &table, 1, &stray).unwrap_err();
    assert_eq!((err.seq, err.column.as_str()), (3, "ssn"));
}

/// What a refusal says becomes a hold's reason, stored and logged: never a
/// row's value.
#[test]
fn a_refusal_names_the_column_never_the_value() {
    let not_an_image = parse_image(r#""s3cret""#).unwrap_err();
    assert!(
        not_an_image.starts_with("row_image is not an object"),
        "{not_an_image}"
    );
    assert!(!not_an_image.contains("s3cret"), "{not_an_image}");

    let table = plan("leases", &[(1, &[(1, "id", "smallint")])]);
    let rows = [row(1, "insert", 1, Some(r#"{"id": 987654}"#))];
    let err = insert_sql(WRITER, &table, 1, &rows).unwrap_err();
    assert_eq!(
        err.to_string(),
        "seq 1, column `id`: a number out of range for SMALLINT"
    );
    let mut huge = row(2, "insert", 1, Some(r#"{"id": 1}"#));
    huge.txid = u64::MAX;
    let err = insert_sql(WRITER, &table, 1, &[huge]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "seq 2, column `_txid`: out of range for BIGINT"
    );
}

/// More digits than an `f64` holds, in a `numeric(38,2)`; and an
/// unconstrained `numeric`, which is `VARCHAR`.
const WIDE: &str = "12345678901234567.89";
const WIDER: &str = "123456789012345678901234567890.123456789";

#[test]
fn a_wide_numeric_keeps_every_digit() {
    let image = format!(r#"{{"n": {WIDE}, "free": {WIDER}}}"#);
    let parsed = parse_image(&image).expect("an image");
    assert_eq!(parsed["n"].get(), WIDE);
    let n = literal(&parsed["n"], &DuckType::Decimal(38, 2));
    assert_eq!(n, Ok(format!("CAST({WIDE} AS DECIMAL(38,2))")));
    let free = literal(&parsed["free"], &DuckType::of("numeric"));
    assert_eq!(free, Ok(format!("CAST('{WIDER}' AS VARCHAR)")));

    let columns: &[(i16, &str, &str)] = &[(1, "n", "numeric(38,2)"), (2, "free", "numeric")];
    let table = plan("ledger", &[(1, columns)]);
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    ship(
        &conn,
        &table,
        1,
        &[row(1, "insert", 1, Some(image.as_str()))],
    );
    let read = "SELECT CAST(n AS VARCHAR), free FROM oltp_shop.ledger__changes";
    assert_eq!(texts(&conn, read, 2), [[text(WIDE), text(WIDER)]]);
}

/// The `timestamp with time zone` column and `_changed_at`, in microseconds
/// since the epoch, whatever the session's time zone.
fn instants(conn: &Connection, table: &str) -> Vec<(Option<i64>, i64)> {
    let read = format!("SELECT tz, _changed_at FROM oltp_shop.{table} ORDER BY _seq");
    let mut stmt = conn.prepare(&read).expect("a query");
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("rows");
    rows.collect::<Result<_, _>>().expect("rows")
}

/// R2: a table whose rows are all truncates has `_` columns only.
fn a_truncate_only_table_round_trips(conn: &Connection) {
    let gone = ChangeTable::plan("gone", &[]).expect("a plan");
    run(conn, &table_ddl(WRITER, &gone, None));
    ship(conn, &gone, 1, &[row(4, "truncate", 0, None)]);
    let read = "SELECT CAST(_seq AS VARCHAR), _op, _actor FROM oltp_shop.gone__changes";
    let marker = [text("4"), text("truncate"), text("alice")];
    assert_eq!(texts(conn, read, 3), [marker]);
}

#[test]
fn ddl_then_insert_round_trips_typed_values() {
    let mut columns: Vec<(i16, &str, &str)> =
        TYPED.iter().map(|(a, c, t, ..)| (*a, *c, *t)).collect();
    columns.push((17, "tz", "timestamp with time zone"));
    let table = plan("everything", &[(1, columns.as_slice())]);
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    ship(&conn, &table, 1, &typed_rows());

    let read = TYPED.map(|(_, c, ..)| format!("CAST(\"{c}\" AS VARCHAR)"));
    let read = format!(
        "SELECT {}, _op, _actor, CAST(_invocation_id AS VARCHAR), CAST(_txid AS VARCHAR) \
         FROM oltp_shop.everything__changes ORDER BY _seq",
        read.join(", ")
    );
    assert_eq!(texts(&conn, &read, TYPED.len() + 4), typed_texts());
    let tz = DateTime::parse_from_rfc3339(TZ).expect("a time");
    let changed = |seq: i64| at(1_780_000_000 + seq).timestamp_micros();
    let expected = [
        (Some(tz.timestamp_micros()), changed(1)),
        (None, changed(2)),
        (None, changed(3)),
    ];
    assert_eq!(instants(&conn, "everything__changes"), expected);

    a_truncate_only_table_round_trips(&conn);
}

/// Rows of `generation` with these `seq`s, each image `{"id": <seq>}`.
fn ids(generation: i64, seqs: &[i64]) -> Vec<ShipRow> {
    seqs.iter()
        .map(|&seq| {
            let image = format!(r#"{{"id": {seq}}}"#);
            let shipped = row(seq, "insert", 1, Some(image.as_str()));
            ShipRow {
                generation,
                ..shipped
            }
        })
        .collect()
}

#[test]
fn a_batch_written_twice_lands_once() {
    let table = plan("leases", &[(1, &[(1, "id", "integer")])]);
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    ship(&conn, &table, 1, &ids(1, &[10, 11, 12]));
    // The relay died after the commit: the same batch, written again.
    ship(&conn, &table, 1, &ids(1, &[10, 11, 12]));
    let count = "SELECT CAST(count(*) AS VARCHAR) FROM oltp_shop.leases__changes";
    assert_eq!(texts(&conn, count, 1), [[text("3")]]);
    // The same seqs in another generation are other changes.
    ship(&conn, &table, 2, &ids(2, &[10, 11, 12]));
    assert_eq!(texts(&conn, count, 1), [[text("6")]]);

    // A later batch overlapping one already shipped (a transaction that
    // committed after the read holds a lower seq): only its new seqs land,
    // each tested on its own, never the whole range.
    ship(&conn, &table, 3, &ids(3, &[10, 12]));
    ship(&conn, &table, 3, &ids(3, &[11, 12, 13]));
    let read = "SELECT CAST(_seq AS VARCHAR) FROM oltp_shop.leases__changes \
                WHERE _generation = 3 ORDER BY _seq";
    let landed = [[text("10")], [text("11")], [text("12")], [text("13")]];
    assert_eq!(texts(&conn, read, 1), landed);
}

#[test]
fn an_added_column_is_null_for_older_rows() {
    let v1: &[(i16, &str, &str)] = &[(1, "id", "integer")];
    let v2: &[(i16, &str, &str)] = &[(1, "id", "integer"), (2, "note", "text")];
    let conn = duck();
    let before = plan("leases", &[(1, v1)]);
    run(&conn, &table_ddl(WRITER, &before, None));
    ship(
        &conn,
        &before,
        1,
        &[row(1, "insert", 1, Some(r#"{"id": 1}"#))],
    );

    let after = plan("leases", &[(1, v1), (2, v2)]);
    let existing: Vec<String> = texts(&conn, &columns_sql(WRITER), 2)
        .into_iter()
        .filter(|r| r[0].as_deref() == Some(changes_table_name("leases").as_str()))
        .filter_map(|r| r[1].clone())
        .collect();
    assert!(existing.iter().any(|c| c == "_seq"), "{existing:?}");
    let alter = table_ddl(WRITER, &after, Some(existing.as_slice()));
    let add =
        "ALTER TABLE \"oltp_shop\".\"leases__changes\" ADD COLUMN IF NOT EXISTS \"note\" VARCHAR";
    assert_eq!(alter, [add]);
    run(&conn, &alter);
    ship(
        &conn,
        &after,
        1,
        &[row(2, "update", 2, Some(r#"{"id": 1, "note": "hi"}"#))],
    );
    let read = "SELECT CAST(id AS VARCHAR), note FROM oltp_shop.leases__changes ORDER BY _seq";
    let older_is_null = [[text("1"), None], [text("1"), text("hi")]];
    assert_eq!(texts(&conn, read, 2), older_is_null);
}

#[test]
fn progress_holds_one_row() {
    let conn = duck();
    assert!(texts(&conn, &progress_read_sql(WRITER), 2).is_empty());
    for batch in [7, 8] {
        let [delete, insert] = progress_write_sql(WRITER, 1, batch);
        run(&conn, &["BEGIN".into(), delete, insert, "COMMIT".into()]);
    }
    let read = progress_read_sql(WRITER);
    let read = format!("SELECT CAST(generation AS VARCHAR), CAST(batch AS VARCHAR) FROM ({read})");
    assert_eq!(texts(&conn, &read, 2), [[text("1"), text("8")]]);
    let count = "SELECT CAST(count(*) AS VARCHAR) FROM oltp_shop._progress";
    assert_eq!(texts(&conn, count, 1), [[text("1")]]);
}
