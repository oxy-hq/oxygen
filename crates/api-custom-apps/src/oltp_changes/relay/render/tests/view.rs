//! The current-state view, run in DuckDB over a change table and
//! `_baselines`: the latest change per key after the last completed baseline,
//! cut at the last truncate since.

use duckdb::Connection;
use oxy_oltp::changes::Column;

use super::{WRITER, at, duck, row, run, ship, text, texts};
use crate::oltp_changes::relay::render::{
    Marker, MarkerKind, ShipRow, baselines_insert_sql, table_ddl, view_ddl,
};
use crate::oltp_changes::relay::table::{ChangeTable, ShapeVersion};

const VIEW: &str = r#"SELECT * FROM "oltp_shop"."leases" ORDER BY id"#;

/// `leases` at `schema_version` with text columns `columns`, keyed by `id`.
fn shape(schema_version: i32, columns: &[&str]) -> ShapeVersion {
    ShapeVersion {
        schema_version,
        captured: columns
            .iter()
            .zip(1..)
            .map(|(name, attnum)| Column {
                attnum,
                name: (*name).to_owned(),
                type_name: "text".to_owned(),
            })
            .collect(),
        primary_key: vec!["id".to_owned()],
    }
}

/// An in-memory DuckDB with `leases`' change table and view, planned from
/// `shapes`; the view follows the newest of them.
fn leases(shapes: &[ShapeVersion]) -> (Connection, ChangeTable) {
    let table = ChangeTable::plan("leases", shapes).expect("a plan");
    let latest = shapes
        .iter()
        .max_by_key(|s| s.schema_version)
        .expect("a shape");
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    run(
        &conn,
        &[view_ddl(WRITER, "leases", latest, &latest.primary_key)],
    );
    (conn, table)
}

fn flats() -> (Connection, ChangeTable) {
    leases(&[shape(1, &["id", "name"])])
}

fn image(id: &str, name: &str) -> String {
    format!(r#"{{"id": "{id}", "name": "{name}"}}"#)
}

/// A row of `op` carrying `id`'s full image.
fn change(seq: i64, op: &'static str, id: &str, name: &str) -> ShipRow {
    row(seq, op, 1, Some(image(id, name).as_str()))
}

fn marker(seq: i64, kind: MarkerKind, begin_seq: i64) -> Marker {
    Marker {
        table: "leases".to_owned(),
        seq,
        kind,
        begin_seq,
        schema_version: 1,
        marked_at: at(1_780_000_000 + seq),
    }
}

fn begin(seq: i64) -> Marker {
    marker(seq, MarkerKind::Begin, seq)
}

fn complete(seq: i64, begin_seq: i64) -> Marker {
    marker(seq, MarkerKind::Complete, begin_seq)
}

fn mark(conn: &Connection, generation: i64, markers: &[Marker]) {
    run(conn, &[baselines_insert_sql(WRITER, generation, markers)]);
}

#[test]
fn the_view_is_empty_until_a_baseline_completes() {
    let (conn, table) = flats();
    mark(&conn, 1, &[begin(1)]);
    ship(
        &conn,
        &table,
        1,
        &[
            change(2, "snapshot", "1", "Flat 1"),
            change(3, "snapshot", "2", "Flat 2"),
        ],
    );
    assert!(texts(&conn, VIEW, 2).is_empty());
}

#[test]
fn the_view_reads_the_latest_change_per_key_after_the_baseline() {
    let (conn, table) = flats();
    let rows = [
        change(1, "insert", "9", "Before the baseline"),
        change(3, "snapshot", "1", "Flat 1"),
        change(4, "snapshot", "2", "Flat 2"),
        change(6, "update", "1", "Flat 1b"),
        change(7, "delete", "2", "Flat 2"),
    ];
    ship(&conn, &table, 1, &rows);
    mark(&conn, 1, &[begin(2), complete(5, 2)]);
    let rows = texts(&conn, VIEW, 2);
    assert_eq!(rows, [[text("1"), text("Flat 1b")]]);
}

#[test]
fn a_truncate_after_the_baseline_empties_the_view_up_to_it() {
    let (conn, table) = flats();
    let rows = [
        ShipRow {
            image: None,
            ..change(1, "truncate", "", "")
        },
        change(3, "snapshot", "1", "Flat 1"),
        ShipRow {
            image: None,
            ..change(5, "truncate", "", "")
        },
        change(6, "insert", "2", "Flat 2"),
    ];
    ship(&conn, &table, 1, &rows);
    mark(&conn, 1, &[begin(2), complete(4, 2)]);
    // The truncate before the baseline's begin changes nothing; the one after
    // it drops the snapshot, and only the later insert remains.
    assert_eq!(texts(&conn, VIEW, 2), [[text("2"), text("Flat 2")]]);
}

/// The view stays on the last completed baseline while the next copies, and
/// the copying baseline's snapshots show through it as each key's latest
/// image, true as of its `seq`.
#[test]
fn a_copying_baselines_snapshots_show_through_on_the_previous_one() {
    let (conn, table) = flats();
    ship(&conn, &table, 1, &[change(2, "snapshot", "1", "Flat 1")]);
    mark(&conn, 1, &[begin(1), complete(3, 1)]);
    assert_eq!(texts(&conn, VIEW, 2), [[text("1"), text("Flat 1")]]);
    // The next baseline has begun and has not completed: its snapshot shows
    // through as the key's latest image, still one row for the key.
    mark(&conn, 1, &[begin(4)]);
    ship(
        &conn,
        &table,
        1,
        &[change(5, "snapshot", "1", "Flat 1 copied")],
    );
    assert_eq!(texts(&conn, VIEW, 2), [[text("1"), text("Flat 1 copied")]]);
}

#[test]
fn a_later_generation_baseline_replaces_an_earlier_one() {
    let (conn, table) = flats();
    ship(&conn, &table, 1, &[change(2, "snapshot", "1", "Old")]);
    mark(&conn, 1, &[begin(1), complete(3, 1)]);
    assert_eq!(texts(&conn, VIEW, 2), [[text("1"), text("Old")]]);

    // The next generation restarts its `seq`s; its baseline is the one read.
    let newer = ShipRow {
        generation: 2,
        ..change(2, "snapshot", "5", "New")
    };
    ship(&conn, &table, 2, &[newer]);
    mark(&conn, 2, &[begin(0), complete(3, 0)]);
    assert_eq!(texts(&conn, VIEW, 2), [[text("5"), text("New")]]);
}

#[test]
fn the_view_has_the_latest_shapes_columns() {
    let (conn, _) = leases(&[shape(1, &["id", "name", "note"]), shape(2, &["id", "name"])]);
    let described = texts(&conn, r#"DESCRIBE "oltp_shop"."leases""#, 1);
    assert_eq!(described, [[text("id")], [text("name")]]);
}

/// A view rendered for the key `(id, region)` keeps two rows that share `id`.
#[test]
fn a_composite_key_keeps_rows_that_share_one_column() {
    let mut keyed = shape(1, &["id", "region", "name"]);
    keyed.primary_key = vec!["id".to_owned(), "region".to_owned()];
    let table = ChangeTable::plan("leases", &[keyed.clone()]).expect("a plan");
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    run(
        &conn,
        &[view_ddl(WRITER, "leases", &keyed, &keyed.primary_key)],
    );
    let image = |region: &str, name: &str| {
        format!(r#"{{"id": "1", "region": "{region}", "name": "{name}"}}"#)
    };
    let rows = [
        row(2, "snapshot", 1, Some(image("eu", "Flat eu").as_str())),
        row(3, "snapshot", 1, Some(image("us", "Flat us").as_str())),
        row(4, "update", 1, Some(image("eu", "Flat eu b").as_str())),
    ];
    ship(&conn, &table, 1, &rows);
    mark(&conn, 1, &[begin(1), complete(5, 1)]);
    let read = r#"SELECT region, name FROM "oltp_shop"."leases" ORDER BY region"#;
    assert_eq!(
        texts(&conn, read, 2),
        [
            [text("eu"), text("Flat eu b")],
            [text("us"), text("Flat us")]
        ]
    );
}

/// A reshape adds `tenant` and keys the table by `(tenant, id)`. While its
/// baseline copies, the view reads the last completed baseline's window, v1's,
/// where `tenant` is null, plus the copy's snapshots, which show through with
/// it set: keyed by v1's `id`, one row per row; keyed by v2's key, the same
/// row twice.
#[test]
fn a_reshape_copying_keeps_one_row_per_key_by_the_completed_baselines_key() {
    let v1 = shape(1, &["id", "name"]);
    let mut v2 = shape(2, &["id", "name", "tenant"]);
    v2.primary_key = vec!["tenant".to_owned(), "id".to_owned()];
    let table = ChangeTable::plan("leases", &[v1.clone(), v2.clone()]).expect("a plan");
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    let copied = r#"{"id": "1", "tenant": "t1", "name": "Flat 1 copied"}"#;
    let rows = [
        change(2, "snapshot", "1", "Flat 1"),
        row(5, "snapshot", 2, Some(copied)),
    ];
    ship(&conn, &table, 1, &rows);
    mark(&conn, 1, &[begin(1), complete(3, 1), begin(4)]);
    let read = r#"SELECT * FROM "oltp_shop"."leases" ORDER BY id, tenant"#;

    run(&conn, &[view_ddl(WRITER, "leases", &v2, &v1.primary_key)]);
    assert_eq!(
        texts(&conn, read, 3),
        [[text("1"), text("Flat 1 copied"), text("t1")]],
        "keyed by the completed baseline's key"
    );
    run(&conn, &[view_ddl(WRITER, "leases", &v2, &v2.primary_key)]);
    assert_eq!(
        texts(&conn, read, 3).len(),
        2,
        "keyed by the copy's own key"
    );
}
