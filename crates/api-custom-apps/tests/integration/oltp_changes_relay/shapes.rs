//! A declared table's shape changing under a promote's migration: an added
//! column joins the change table, null for the rows before it; a renamed one
//! holds that table alone, its rows left in the log, while the writer's
//! other tables keep shipping.

use oxy_api_custom_apps::oltp_changes::relay::RelayStats;
use oxy_api_custom_apps::oltp_changes::relay::render::{PROGRESS_TABLE, changes_table_name};

use super::{text, with_relay};
use crate::oltp_changes_fixture::LEASES_SQL;

/// The next migration: a column added to `leases`, and nothing backfilled.
const ADD_REGION_SQL: (&str, &[u8]) = (
    "migrations/0002_region.sql",
    b"alter table leases add column region text;",
);

/// Two tables to declare, each with a text column.
const LEASES_AND_UNITS_SQL: (&str, &[u8]) = (
    "migrations/0001_leases_units.sql",
    b"create table leases (id bigint primary key, name text); \
      create table units (id bigint primary key, label text);",
);

/// The next migration: `units.label` renamed, which the relay cannot follow.
const RENAME_LABEL_SQL: (&str, &[u8]) = (
    "migrations/0002_rename_label.sql",
    b"alter table units rename column label to title;",
);

/// Row 1 ships before the promote that adds `region`, row 2 is captured
/// before it and ships after, row 3 carries it: the change table gains
/// `region` (an `ALTER`, never a new table), null in rows 1 and 2.
#[tokio::test]
async fn an_added_column_is_added_and_null_before() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        assert_eq!(r.pass().await.shipped, 1);
        let before: Vec<String> = r.columns("leases").into_iter().map(|(c, _)| c).collect();
        assert!(!before.contains(&"region".to_string()), "{before:?}");

        r.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        r.app
            .promote("chg-2", true, &[LEASES_SQL, ADD_REGION_SQL])
            .await
            .expect("the promote adds region and reinstalls capture");
        r.app
            .write("insert into leases (id, name, region) values (3, 'Flat 3', 'north')")
            .await;
        assert_eq!(r.pass().await.shipped, 2);

        let region = ("region".to_string(), "VARCHAR".to_string());
        assert!(r.columns("leases").contains(&region), "region is a column");
        let rows = r.changes("leases", "id, name, region");
        assert_eq!(
            rows,
            [
                [text("1"), text("Flat 1"), None],
                [text("2"), text("Flat 2"), None],
                [text("3"), text("Flat 3"), text("north")],
            ]
        );
        assert_eq!(r.waiting(None).await, 0);
    })
    .await;
}

/// `leases` and `units` declared and written; then `units.label` is renamed
/// and both are written again. One pass ships `leases` and holds `units`
/// alone, naming the rename, with its rows left in the log and no change
/// table made for it.
#[tokio::test]
async fn a_renamed_column_holds_only_its_table() {
    with_relay(|r| async move {
        let both = ["leases", "units"];
        r.app
            .promote_declaring("chg-1", &both, &[LEASES_AND_UNITS_SQL])
            .await
            .expect("the promote installs capture on both tables");
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        r.app
            .write("insert into units (id, label) values (1, 'Unit 1')")
            .await;
        r.app
            .promote_declaring("chg-2", &both, &[LEASES_AND_UNITS_SQL, RENAME_LABEL_SQL])
            .await
            .expect("the promote renames the column and reinstalls capture");
        r.app
            .write("insert into units (id, title) values (2, 'Unit 2')")
            .await;
        r.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;

        let held = RelayStats {
            shipped: 2,
            batches: 1,
            held: 2,
            skipped_lease: false,
        };
        assert_eq!(r.pass().await, held);
        let leases = r.changes("leases", "id, name");
        assert_eq!(
            leases,
            [[text("1"), text("Flat 1")], [text("2"), text("Flat 2")]]
        );
        let tables = [PROGRESS_TABLE.to_string(), changes_table_name("leases")];
        assert_eq!(r.tables(), tables, "no change table for units");

        let holds = r.holds().await;
        assert_eq!(holds.len(), 1, "{holds:?}");
        let (writer, table, reason) = &holds[0];
        assert_eq!(
            (writer.as_str(), table.as_str()),
            (r.writer().as_str(), "units")
        );
        assert!(reason.contains("renamed"), "{reason}");
        assert_eq!(r.waiting(Some("units")).await, 2, "units' rows stay");
        assert_eq!(r.waiting(Some("leases")).await, 0, "leases' are shipped");
    })
    .await;
}
