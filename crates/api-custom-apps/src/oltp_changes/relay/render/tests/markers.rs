//! A batch's markers, written to `_baselines` beside its rows.

use super::{WRITER, at, duck, row, run, text, texts};
use crate::oltp_changes::relay::render::{
    Marker, MarkerKind, baselines_insert_sql, insert_statements, table_ddl,
};
use crate::oltp_changes::relay::table::ChangeTable;

use super::plan;

fn marker(seq: i64, kind: MarkerKind) -> Marker {
    Marker {
        table: "leases".to_owned(),
        seq,
        kind,
        begin_seq: 1,
        schema_version: 1,
        marked_at: at(1_780_000_000 + seq),
    }
}

#[test]
fn a_marker_batch_written_twice_lands_once() {
    let table: ChangeTable = plan("leases", &[(1, &[(1, "id", "bigint")])]);
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    let snapshot = row(2, "snapshot", 1, Some(r#"{"id": 7}"#));
    let mut statements = insert_statements(WRITER, &table, 1, &[snapshot]).expect("renders");
    let markers = [
        marker(1, MarkerKind::Begin),
        marker(3, MarkerKind::Complete),
    ];
    statements.push(baselines_insert_sql(WRITER, 1, &markers));
    run(&conn, &statements);
    run(&conn, &statements);

    let count = "SELECT marker, CAST(count(*) AS VARCHAR) FROM oltp_shop._baselines \
                 GROUP BY 1 ORDER BY 1";
    assert_eq!(
        texts(&conn, count, 2),
        [[text("begin"), text("1")], [text("complete"), text("1")]]
    );
    let rows = "SELECT CAST(count(*) AS VARCHAR) FROM oltp_shop.leases__changes";
    assert_eq!(texts(&conn, rows, 1), [[text("1")]]);
}

#[test]
fn no_markers_render_no_statement() {
    assert_eq!(baselines_insert_sql(WRITER, 1, &[]), "");
}
