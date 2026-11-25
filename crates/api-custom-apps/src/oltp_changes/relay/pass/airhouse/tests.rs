//! The batch's Airhouse statements, and how the progress read is taken.

use std::collections::HashMap;

use oxy_oltp::changes::Column;

use super::{columns_by_table, ddl_statements, progress_from, write_statements};
use crate::oltp_changes::relay::check::Shipped;
use crate::oltp_changes::relay::pass::plan::Ready;
use crate::oltp_changes::relay::sink::{AirhouseSink, ChangeSink, SinkError};
use crate::oltp_changes::relay::table::{ChangeTable, ShapeVersion};

fn text(cells: &[&str]) -> Vec<Option<String>> {
    cells.iter().map(|c| Some((*c).to_owned())).collect()
}

fn orders(insert: &str) -> Ready {
    let shape = ShapeVersion {
        schema_version: 1,
        captured: vec![
            Column {
                attnum: 1,
                name: "id".to_owned(),
                type_name: "bigint".to_owned(),
            },
            Column {
                attnum: 2,
                name: "total".to_owned(),
                type_name: "numeric(10,2)".to_owned(),
            },
        ],
    };
    Ready {
        table: ChangeTable::plan("orders", &[shape]).expect("a plan"),
        inserts: vec![insert.to_owned()],
        seqs: vec![1],
    }
}

/// The progress read as [`AirhouseSink`] classifies its failures.
fn airhouse_progress(
    read: Result<Vec<Vec<Option<String>>>, SinkError>,
) -> Result<Option<Shipped>, String> {
    progress_from(read, &|e: &SinkError| AirhouseSink.is_missing(e))
}

fn failed(message: &str) -> Result<Vec<Vec<Option<String>>>, SinkError> {
    Err(SinkError(message.to_owned()))
}

/// A schema or `_progress` not made yet is no progress; any other failure,
/// a connect that says `does not exist` included, is the writer's error, so
/// the restore check never reads it as nothing shipped.
#[test]
fn a_missing_schema_or_table_is_no_progress() {
    for missing in [
        "[XX000] Catalog Error: Table with name _progress does not exist!",
        "[XX000] Catalog Error: Schema with name oltp_shop does not exist!",
        "[42P01] relation \"oltp_shop._progress\" does not exist",
    ] {
        assert_eq!(airhouse_progress(failed(missing)), Ok(None), "{missing}");
    }
    for refused in [
        "[28000] permission denied",
        "connect: connect failed: [28000] role \"oxy_w\" does not exist",
        "connect: connect failed: [XX000] Catalog Error: Table with name x does not exist!",
    ] {
        let error = airhouse_progress(failed(refused)).unwrap_err();
        assert!(error.contains(refused), "{error}");
    }
    assert_eq!(airhouse_progress(Ok(Vec::new())), Ok(None));
    assert_eq!(
        airhouse_progress(Ok(vec![text(&["1", "5"])])),
        Ok(Some(Shipped {
            generation: 1,
            batch: 5
        }))
    );
    assert!(airhouse_progress(Ok(vec![vec![None, None]])).is_err());
}

#[test]
fn the_listed_columns_group_by_table() {
    let listed = vec![
        text(&["_progress", "generation"]),
        text(&["orders__changes", "id"]),
        text(&["orders__changes", "total"]),
    ];
    let by_table = columns_by_table(listed);
    assert_eq!(by_table["orders__changes"], vec!["id", "total"]);
    assert_eq!(by_table["_progress"], vec!["generation"]);
}

/// The schema and `_progress` first, every batch; then a change table
/// created when Airhouse lists none, or given only the columns it lacks.
#[test]
fn a_change_table_is_created_or_given_its_new_columns() {
    let ready = [orders("INSERT")];
    let created = ddl_statements("shop", &ready, &HashMap::new());
    assert_eq!(created.len(), 3, "{created:?}");
    assert!(created[0].starts_with("CREATE SCHEMA IF NOT EXISTS \"oltp_shop\""));
    assert!(created[1].contains("\"oltp_shop\".\"_progress\""));
    assert!(created[2].starts_with("CREATE TABLE IF NOT EXISTS \"oltp_shop\".\"orders__changes\""));

    let existing = HashMap::from([("orders__changes".to_owned(), vec!["id".to_owned()])]);
    let altered = ddl_statements("shop", &ready, &existing);
    assert_eq!(altered.len(), 3, "{altered:?}");
    assert!(
        altered[2].contains("ADD COLUMN IF NOT EXISTS \"total\""),
        "{altered:?}"
    );
}

/// Every ready table's rows, none skipped, then `_progress`, in one
/// transaction: the `seq`s confirmed are exactly those written.
#[test]
fn a_batch_writes_every_tables_rows_then_its_progress() {
    let statements = write_statements("shop", 1, 7, &[orders("INSERT A"), orders("INSERT B")]);
    assert_eq!(statements.len(), 4, "{statements:?}");
    assert_eq!(statements[0], "INSERT A");
    assert_eq!(statements[1], "INSERT B");
    assert_eq!(statements[2], "DELETE FROM \"oltp_shop\".\"_progress\"");
    assert!(
        statements[3].contains("VALUES (1, 7, now())"),
        "{statements:?}"
    );
}
