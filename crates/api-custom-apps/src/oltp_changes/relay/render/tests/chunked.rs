//! A large table's rows, sent as one `INSERT` per `INSERT_ROWS`: each its
//! own anti-join over its own `seq` range, together writing every row once.

use super::{WRITER, duck, plan, row, run, text, texts};
use crate::oltp_changes::relay::INSERT_ROWS;
use crate::oltp_changes::relay::render::{ShipRow, insert_statements, table_ddl};

/// Rows `1..=n` of `leases`, each image `{"id": <seq>}`.
fn leases(n: i64) -> Vec<ShipRow> {
    (1..=n)
        .map(|seq| {
            let image = format!(r#"{{"id": {seq}}}"#);
            row(seq, "insert", 1, Some(image.as_str()))
        })
        .collect()
}

#[test]
fn a_large_table_is_sent_in_bounded_inserts() {
    let table = plan("leases", &[(1, &[(1, "id", "bigint")])]);
    let rows = leases(2 * INSERT_ROWS as i64 + 1);
    let inserts = insert_statements(WRITER, &table, 1, &rows).expect("renders");
    assert_eq!(inserts.len(), 3, "two full chunks and one row");
    let ranges = [
        "BETWEEN 1 AND 500",
        "BETWEEN 501 AND 1000",
        "BETWEEN 1001 AND 1001",
    ];
    for (insert, range) in inserts.iter().zip(ranges) {
        assert!(insert.contains(range), "{range} in {insert}");
    }
    assert!(
        insert_statements(WRITER, &table, 1, &[])
            .expect("renders")
            .is_empty()
    );

    // Every row lands once, and the same chunks sent again add nothing.
    let conn = duck();
    run(&conn, &table_ddl(WRITER, &table, None));
    run(&conn, &inserts);
    run(&conn, &inserts);
    let count = "SELECT CAST(count(*) AS VARCHAR), CAST(count(DISTINCT _seq) AS VARCHAR) \
                 FROM oltp_shop.leases__changes";
    assert_eq!(texts(&conn, count, 2), [[text("1001"), text("1001")]]);
}
