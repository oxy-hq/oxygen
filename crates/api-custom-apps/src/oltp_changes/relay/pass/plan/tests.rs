//! A batch planned table by table: what ships, what is held, and who each
//! change ships as.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use oxy_oltp::changes::{Column, LogRow};
use uuid::Uuid;

use super::{Planned, Source, UNVERIFIED, attribution, op_name, plan, plan_table};
use crate::oltp_changes::provenance::{ChangeSigner, sign_keyed, verify_keyed};
use crate::oltp_changes::relay::table::ShapeVersion;

const KEY: [u8; 32] = [9; 32];
const SOURCE: Source<'static> = Source {
    writer: "shop",
    schema: "app_shop",
    generation: 1,
};

fn verify(s: &ChangeSigner<'_>, mac: Option<&[u8]>) -> bool {
    verify_keyed(&KEY, s, mac)
}

fn column(attnum: i16, name: &str, type_name: &str) -> Column {
    Column {
        attnum,
        name: name.to_owned(),
        type_name: type_name.to_owned(),
    }
}

fn shape(schema_version: i32, captured: Vec<Column>) -> ShapeVersion {
    ShapeVersion {
        schema_version,
        captured,
    }
}

/// `orders (id, total)` and `notes (id, body)`, each at version 1.
fn shapes() -> HashMap<String, Vec<ShapeVersion>> {
    HashMap::from([
        (
            "orders".to_owned(),
            vec![shape(
                1,
                vec![
                    column(1, "id", "bigint"),
                    column(2, "total", "numeric(10,2)"),
                ],
            )],
        ),
        (
            "notes".to_owned(),
            vec![shape(
                1,
                vec![column(1, "id", "bigint"), column(2, "body", "text")],
            )],
        ),
    ])
}

fn row(seq: i64, table: &str, op: char, image: Option<&str>) -> LogRow {
    LogRow {
        seq,
        txid: 700 + seq as u64,
        table_name: table.to_owned(),
        schema_version: 1,
        op,
        row_image: image.map(str::to_owned),
        actor: None,
        invocation: None,
        provenance: None,
        changed_at: Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap(),
    }
}

fn order(seq: i64, op: char) -> LogRow {
    row(seq, "orders", op, Some(r#"{"id": 1, "total": 12.50}"#))
}

fn note(seq: i64) -> LogRow {
    row(seq, "notes", 'i', Some(r#"{"id": 1, "body": "hi"}"#))
}

fn batch(rows: Vec<LogRow>, shapes: &HashMap<String, Vec<ShapeVersion>>) -> Planned {
    plan(SOURCE, rows, shapes, &verify)
}

fn ready_tables(planned: &Planned) -> Vec<(&str, Vec<i64>)> {
    planned
        .ready
        .iter()
        .map(|r| (r.table.table.as_str(), r.seqs.clone()))
        .collect()
}

#[test]
fn each_table_ships_its_own_rows() {
    let planned = batch(
        vec![order(1, 'i'), note(2), order(3, 'u'), order(4, 'd')],
        &shapes(),
    );
    assert!(planned.refused.is_empty(), "{:?}", planned.refused);
    assert_eq!(
        ready_tables(&planned),
        vec![("notes", vec![2]), ("orders", vec![1, 3, 4])]
    );
    let orders = planned.ready[1].inserts.concat();
    assert!(
        orders.contains(r#""oltp_shop"."orders__changes""#),
        "{orders}"
    );
    for op in ["'insert'", "'update'", "'delete'"] {
        assert!(orders.contains(op), "{op} in {orders}");
    }
}

#[test]
fn an_unknown_op_holds_only_its_table() {
    let planned = batch(vec![order(1, 'x'), note(2)], &shapes());
    assert_eq!(ready_tables(&planned), vec![("notes", vec![2])]);
    assert_eq!(planned.refused.len(), 1);
    assert_eq!(planned.refused[0].table, "orders");
    assert_eq!(planned.refused[0].rows, 1);
    assert!(planned.refused[0].reason.contains("unknown op"));
}

#[test]
fn a_row_without_its_shape_holds_its_table() {
    let mut newer = order(1, 'i');
    newer.schema_version = 2;
    let planned = batch(vec![newer, order(2, 'u'), note(3)], &shapes());
    assert_eq!(ready_tables(&planned), vec![("notes", vec![3])]);
    assert_eq!(planned.refused[0].table, "orders");
    assert_eq!(planned.refused[0].rows, 2);
    assert!(
        planned.refused[0].reason.contains("schema_version 2"),
        "{}",
        planned.refused[0].reason
    );
}

/// A table with no shapes (dropped, or no longer captured) ships its
/// truncates, and is held for any other row.
#[test]
fn a_truncate_needs_no_shape() {
    let truncate = row(1, "gone", 't', None);
    let planned = batch(vec![truncate.clone()], &shapes());
    assert_eq!(ready_tables(&planned), vec![("gone", vec![1])]);
    assert!(planned.ready[0].table.columns.is_empty());
    assert!(planned.ready[0].inserts[0].contains("'truncate'"));

    let insert = row(2, "gone", 'i', Some(r#"{"id": 1}"#));
    let planned = batch(vec![truncate, insert], &shapes());
    assert!(planned.ready.is_empty());
    assert_eq!(planned.refused[0].table, "gone");
}

/// The rows use only version 2, but version 1 named attribute 2 otherwise:
/// planning from every recorded shape finds the rename.
#[test]
fn every_recorded_shape_plans_the_table() {
    let mut shapes = shapes();
    shapes.get_mut("orders").unwrap().push(shape(
        2,
        vec![
            column(1, "id", "bigint"),
            column(2, "amount", "numeric(10,2)"),
        ],
    ));
    let mut renamed = row(1, "orders", 'i', Some(r#"{"id": 1, "amount": 3}"#));
    renamed.schema_version = 2;
    let planned = batch(vec![renamed, note(2)], &shapes);
    assert_eq!(ready_tables(&planned), vec![("notes", vec![2])]);
    assert!(
        planned.refused[0].reason.contains("renamed"),
        "{}",
        planned.refused[0].reason
    );
}

#[test]
fn an_unrenderable_value_holds_its_table() {
    let mut huge = order(1, 'i');
    huge.txid = u64::MAX;
    let mut no_image = order(2, 'u');
    no_image.row_image = None;
    for bad in [huge, no_image] {
        let planned = batch(vec![bad, note(3)], &shapes());
        assert_eq!(ready_tables(&planned), vec![("notes", vec![3])]);
        assert_eq!(planned.refused[0].table, "orders");
    }
}

#[test]
fn a_signed_row_ships_its_actor_and_invocation() {
    let mut signed = order(1, 'i');
    let invocation = Uuid::from_u128(42);
    signed.actor = Some("person-1".to_owned());
    signed.invocation = Some(invocation);
    let mac = sign_keyed(
        &KEY,
        &ChangeSigner {
            txid: signed.txid,
            environment: "production",
            writer: "app_shop",
            actor: "person-1",
            invocation: Some(invocation),
        },
    );
    signed.provenance = Some(mac.to_vec());
    assert_eq!(
        attribution(&signed, SOURCE, &verify),
        ("person-1".to_owned(), Some(invocation))
    );
}

/// No MAC, a MAC from another transaction, a MAC under another writer's
/// schema, or no actor at all: each ships as `unverified`, with no
/// invocation.
#[test]
fn an_unsigned_or_forged_row_ships_unverified() {
    let unverified = (UNVERIFIED.to_owned(), None);
    let mut row = order(1, 'i');
    row.actor = Some("person-1".to_owned());
    row.invocation = Some(Uuid::from_u128(42));
    assert_eq!(attribution(&row, SOURCE, &verify), unverified);

    let signer = |txid: u64, writer: &'static str| ChangeSigner {
        txid,
        environment: "production",
        writer,
        actor: "person-1",
        invocation: Some(Uuid::from_u128(42)),
    };
    for (txid, writer) in [(row.txid + 1, "app_shop"), (row.txid, "app_other")] {
        row.provenance = Some(sign_keyed(&KEY, &signer(txid, writer)).to_vec());
        assert_eq!(attribution(&row, SOURCE, &verify), unverified);
    }

    row.provenance = Some(sign_keyed(&KEY, &signer(row.txid, "app_shop")).to_vec());
    row.actor = None;
    assert_eq!(attribution(&row, SOURCE, &verify), unverified);
}

/// A table is ready only with an `INSERT` that writes its `seq`s, so a batch
/// never confirms a `seq` it did not send.
#[test]
fn a_table_without_rows_is_never_ready() {
    let shapes = shapes();
    let refused = plan_table(SOURCE, "orders", &[], &shapes["orders"], &verify);
    assert_eq!(refused.unwrap_err(), "no rows to write");
}

#[test]
fn ops_ship_by_name() {
    assert_eq!(op_name('i'), Some("insert"));
    assert_eq!(op_name('u'), Some("update"));
    assert_eq!(op_name('d'), Some("delete"));
    assert_eq!(op_name('t'), Some("truncate"));
    assert_eq!(op_name('x'), None);
}
