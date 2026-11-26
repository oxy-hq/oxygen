//! The batch's Airhouse statements, and how the progress read is taken.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;
use oxy_oltp::changes::Column;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{
    PendingView, columns_by_table, ddl, ddl_statements, progress_from, view, view_statements,
    write, write_statements,
};
use crate::oltp_changes::relay::check::Shipped;
use crate::oltp_changes::relay::pass::plan::Ready;
use crate::oltp_changes::relay::pass::target::Target;
use crate::oltp_changes::relay::sink::{AirhouseSink, ChangeSink, SinkError};
use crate::oltp_changes::relay::table::{ChangeTable, ShapeVersion};

fn text(cells: &[&str]) -> Vec<Option<String>> {
    cells.iter().map(|c| Some((*c).to_owned())).collect()
}

/// `orders` at `schema_version`, with columns `columns` (`(name, type)`, in
/// attribute order) keyed by `key`.
fn shape(schema_version: i32, columns: &[(&str, &str)], key: &[&str]) -> ShapeVersion {
    ShapeVersion {
        schema_version,
        captured: columns
            .iter()
            .zip(1..)
            .map(|((name, ty), attnum)| Column {
                attnum,
                name: (*name).to_owned(),
                type_name: (*ty).to_owned(),
            })
            .collect(),
        primary_key: key.iter().map(|k| (*k).to_owned()).collect(),
    }
}

/// `orders` ready to ship from `versions`, the last the latest.
fn ready(insert: &str, versions: &[ShapeVersion]) -> Ready {
    Ready {
        table: ChangeTable::plan("orders", versions).expect("a plan"),
        latest: versions.last().cloned(),
        completes_baseline: None,
        keys: versions
            .iter()
            .map(|v| (v.schema_version, v.primary_key.clone()))
            .collect(),
        inserts: vec![insert.to_owned()],
        seqs: vec![1],
    }
}

fn orders(insert: &str) -> Ready {
    let v1 = shape(1, &[("id", "bigint"), ("total", "numeric(10,2)")], &["id"]);
    ready(insert, &[v1])
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
/// created when Airhouse lists none, or given only the columns it lacks. No
/// view among them: views run on their own, after.
#[test]
fn a_change_table_is_created_or_given_its_new_columns() {
    let ready = [orders("INSERT")];
    let created = ddl_statements("shop", &ready, &HashMap::new());
    assert_eq!(created.len(), 4, "{created:?}");
    assert!(created.iter().all(|s| !s.contains("VIEW")), "{created:?}");
    assert!(created[0].starts_with("CREATE SCHEMA IF NOT EXISTS \"oltp_shop\""));
    assert!(created[1].contains("\"oltp_shop\".\"_progress\""));
    assert!(created[2].contains("\"oltp_shop\".\"_baselines\""));
    assert!(created[3].starts_with("CREATE TABLE IF NOT EXISTS \"oltp_shop\".\"orders__changes\""));

    let existing = HashMap::from([("orders__changes".to_owned(), vec!["id".to_owned()])]);
    let altered = ddl_statements("shop", &ready, &existing);
    assert_eq!(altered.len(), 4, "{altered:?}");
    assert!(
        altered[3].contains("ADD COLUMN IF NOT EXISTS \"total\""),
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

/// `orders`' current-state view, as Airhouse lists it.
fn listed_view(columns: &[&str]) -> HashMap<String, Vec<String>> {
    HashMap::from([(
        "orders".to_owned(),
        columns.iter().map(|c| (*c).to_owned()).collect(),
    )])
}

/// The change table's columns as Airhouse lists them, with `view` beside it.
fn listed(view: &[&str]) -> HashMap<String, Vec<String>> {
    let mut existing = listed_view(view);
    existing.insert(
        "orders__changes".to_owned(),
        vec!["id".to_owned(), "total".to_owned()],
    );
    existing
}

/// The views `ready` needs against `existing`.
fn views(ready: Ready, existing: &HashMap<String, Vec<String>>) -> Vec<PendingView> {
    view_statements(&[ready], existing)
}

#[test]
fn a_view_is_made_with_its_change_table() {
    let made = views(orders("INSERT"), &HashMap::new());
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(made[0].table, "orders");
    assert_eq!(made[0].completed, None);
}

#[test]
fn a_view_is_replaced_when_its_columns_differ_from_the_latest_shape() {
    for stale in [&["id"][..], &["id", "total", "note"], &["total", "id"]] {
        let replaced = views(orders("INSERT"), &listed(stale));
        assert_eq!(replaced.len(), 1, "{stale:?}: {replaced:?}");
    }
}

#[test]
fn an_unchanged_view_is_left_alone() {
    // Airhouse answers in the case DuckDB stored; the comparison ignores it.
    for same in [&["id", "total"][..], &["ID", "Total"]] {
        let left = views(orders("INSERT"), &listed(same));
        assert!(left.is_empty(), "{same:?}: {left:?}");
    }
}

#[test]
fn a_truncate_only_table_gets_no_view() {
    let mut gone = orders("INSERT");
    gone.table = ChangeTable::plan("orders", &[]).expect("a plan");
    gone.latest = None;
    let statements = ddl_statements("shop", std::slice::from_ref(&gone), &HashMap::new());
    assert!(
        statements
            .last()
            .is_some_and(|s| s.starts_with("CREATE TABLE IF NOT EXISTS")),
        "{statements:?}"
    );
    assert!(views(gone, &HashMap::new()).is_empty());
}

/// A completed baseline may follow a key change its columns do not show, so
/// its `c` marker makes the view again, keyed by its own shape version; a
/// begin leaves it, still reading the previous baseline's window by that
/// window's key.
#[test]
fn a_complete_marker_replaces_the_view_even_when_its_columns_match() {
    let mut completes = orders("INSERT");
    completes.completes_baseline = Some(1);
    let replaced = views(completes, &listed(&["id", "total"]));
    assert_eq!(replaced.len(), 1, "{replaced:?}");
    assert_eq!(replaced[0].completed, Some(1));
}

/// Airhouse as a sink that answers the latest complete baseline's version
/// with `complete`, refuses every view when `refuses_views` (as DuckLake
/// might refuse the view's `QUALIFY`), and records every statement it ran
/// and every read.
#[derive(Default)]
struct FakeAirhouse {
    complete: Option<&'static str>,
    refuses_views: bool,
    ran: Mutex<Vec<String>>,
    reads: Mutex<Vec<String>>,
}

const REFUSED: &str = "Not implemented Error: QUALIFY in a view";

fn taken<T: Clone>(list: &Mutex<Vec<T>>) -> Vec<T> {
    list.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

impl FakeAirhouse {
    fn record(&self, statements: &[String]) {
        let mut ran = self.ran.lock().unwrap_or_else(|e| e.into_inner());
        ran.extend(statements.iter().cloned());
    }
}

#[async_trait]
impl ChangeSink for FakeAirhouse {
    async fn has_airhouse(&self, _: &DatabaseConnection, _: Uuid) -> Result<bool, SinkError> {
        Ok(true)
    }

    async fn query(&self, _: Uuid, sql: &str) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        let mut reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
        reads.push(sql.to_owned());
        if sql.contains("\"_baselines\"") {
            return Ok(self.complete.map(|v| text(&[v])).into_iter().collect());
        }
        Ok(Vec::new())
    }

    async fn admin(&self, _: Uuid, statements: &[String]) -> Result<(), SinkError> {
        let view = statements
            .iter()
            .any(|s| s.contains("CREATE OR REPLACE VIEW"));
        if view && self.refuses_views {
            return Err(SinkError(REFUSED.to_owned()));
        }
        self.record(statements);
        Ok(())
    }

    async fn write(&self, _: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.record(statements);
        Ok(())
    }
}

fn shop() -> Target {
    Target {
        name: "shop".to_owned(),
        schema: "app_shop".to_owned(),
        workspace: Uuid::nil(),
        held: Vec::new(),
        stale_logged: false,
    }
}

/// `orders` reshaped with a column and a new key: v1 keyed by `id`, v2 by
/// `(tenant_id, id)`, which adds `tenant_id`.
fn rekeyed() -> [ShapeVersion; 2] {
    [
        shape(1, &[("id", "bigint"), ("total", "numeric(10,2)")], &["id"]),
        shape(
            2,
            &[
                ("id", "bigint"),
                ("total", "numeric(10,2)"),
                ("tenant_id", "bigint"),
            ],
            &["tenant_id", "id"],
        ),
    ]
}

/// The reshape's first batch (its `b`) changes the view's columns, but the
/// view still reads v1's window, so it stays keyed by v1's key, read from
/// the latest complete baseline in `_baselines`; the reshape's `c` batch
/// keys it by v2's, from its own marker, with no read.
#[tokio::test]
async fn a_reshape_keys_its_view_by_the_completed_baselines_shape() {
    let versions = rekeyed();
    let first = views(ready("INSERT", &versions), &listed(&["id", "total"]));
    assert_eq!(first.len(), 1, "the columns differ: {first:?}");
    let sink = FakeAirhouse {
        complete: Some("1"),
        ..FakeAirhouse::default()
    };
    view(&sink, &shop(), &first[0]).await.expect("made");
    let made = taken(&sink.ran);
    assert!(
        made[0].contains("PARTITION BY c.\"id\" ORDER BY"),
        "{made:?}"
    );
    assert!(
        made[0].contains("c.\"tenant_id\""),
        "the latest columns: {made:?}"
    );
    assert_eq!(taken(&sink.reads).len(), 1, "one read of _baselines");

    let mut completing = ready("INSERT", &versions);
    completing.completes_baseline = Some(2);
    let last = views(completing, &listed(&["id", "total", "tenant_id"]));
    let sink = FakeAirhouse::default();
    view(&sink, &shop(), &last[0]).await.expect("made");
    let made = taken(&sink.ran);
    assert!(
        made[0].contains("PARTITION BY c.\"tenant_id\", c.\"id\" ORDER BY"),
        "{made:?}"
    );
    assert!(taken(&sink.reads).is_empty(), "the batch's own marker");
}

/// Before any baseline completes, the view is keyed by the latest shape's
/// key.
#[tokio::test]
async fn before_any_baseline_completes_the_view_takes_the_latest_key() {
    let versions = rekeyed();
    let pending = PendingView {
        table: "orders".to_owned(),
        latest: versions[1].clone(),
        keys: BTreeMap::from([(1, vec!["id".to_owned()])]),
        completed: None,
    };
    let sink = FakeAirhouse::default();
    view(&sink, &shop(), &pending).await.expect("made");
    let made = taken(&sink.ran);
    assert!(
        made[0].contains("PARTITION BY c.\"tenant_id\", c.\"id\" ORDER BY"),
        "{made:?}"
    );
}

/// The change tables are made, the view Airhouse refuses fails alone, and
/// the batch is written all the same.
#[tokio::test]
async fn a_view_airhouse_refuses_does_not_stop_the_batch() {
    let sink = FakeAirhouse {
        refuses_views: true,
        ..FakeAirhouse::default()
    };
    let target = shop();
    let ready = [orders("INSERT A")];
    let views = ddl(&sink, &target, &ready)
        .await
        .expect("the change tables are made");
    assert_eq!(views.len(), 1, "{views:?}");
    write(&sink, &target, 1, 7, &ready)
        .await
        .expect("the batch ships");
    let refused = view(&sink, &target, &views[0])
        .await
        .expect_err("the view is refused");
    assert!(refused.contains(REFUSED), "{refused}");
    let ran = taken(&sink.ran);
    let changes = "CREATE TABLE IF NOT EXISTS \"oltp_shop\".\"orders__changes\"";
    assert!(ran.iter().any(|s| s.starts_with(changes)), "{ran:?}");
    assert!(ran.iter().any(|s| s == "INSERT A"), "{ran:?}");
}
