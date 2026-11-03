//! Filling a table that already exists ([`Prelude::refill_plans`]).

use super::*;

fn pair(schema: &str, table: &str) -> (String, String) {
    (schema.into(), table.into())
}

fn copy() -> Prelude {
    Prelude::CopyOnWrite {
        live: pair("app_w", "orders"),
        preview: pair("app_w__dev_a1", "orders"),
    }
}

fn columns(names: &[&str]) -> Vec<String> {
    names.iter().map(|c| c.to_string()).collect()
}

#[test]
fn the_probes_look_for_the_preview_table_and_the_columns_both_sides_have() {
    assert_eq!(
        copy().preview_table_probe(Some("lake")).unwrap(),
        "SELECT count(*) FROM information_schema.tables WHERE table_catalog = 'lake' AND \
         lower(table_schema) = 'app_w__dev_a1' AND lower(table_name) = 'orders'"
    );
    assert_eq!(
        copy().preview_table_probe(None).unwrap(),
        "SELECT count(*) FROM information_schema.tables WHERE \
         lower(table_schema) = 'app_w__dev_a1' AND lower(table_name) = 'orders'"
    );
    assert_eq!(
        copy().shared_columns_probe(Some("lake")).unwrap(),
        "SELECT p.column_name FROM information_schema.columns p WHERE \
         p.table_catalog = 'lake' AND lower(p.table_schema) = 'app_w__dev_a1' AND \
         lower(p.table_name) = 'orders' AND EXISTS (SELECT 1 FROM information_schema.columns l \
         WHERE l.table_catalog = 'lake' AND lower(l.table_schema) = 'app_w' AND \
         lower(l.table_name) = 'orders' AND lower(l.column_name) = lower(p.column_name)) \
         ORDER BY p.ordinal_position"
    );
    let ensure = Prelude::EnsureSchema {
        live: "app_w".into(),
        preview: "app_w__dev_a1".into(),
    };
    assert_eq!(ensure.preview_table_probe(None), None);
    assert_eq!(ensure.shared_columns_probe(None), None);
    assert_eq!(ensure.refill_plans(None, 1, 10, &columns(&["id"])), None);
}

#[test]
fn a_refill_empties_the_table_then_fills_its_shared_columns() {
    let plans = copy()
        .refill_plans(Some("lake"), 10, 10, &columns(&["id", "Total"]))
        .unwrap();
    assert_eq!(
        plans
            .iter()
            .map(|p| p.statement.as_str())
            .collect::<Vec<_>>(),
        [
            "DELETE FROM \"lake\".\"app_w__dev_a1\".\"orders\"",
            "INSERT INTO \"lake\".\"app_w__dev_a1\".\"orders\" (\"id\", \"Total\") \
             SELECT \"id\", \"Total\" FROM \"lake\".\"app_w\".\"orders\"",
        ]
    );
    for plan in &plans {
        assert_eq!(plan.live, pair("app_w", "orders"));
        assert_eq!(plan.state, ShadowState::Shadow);
    }
}

#[test]
fn a_refill_over_the_cap_or_without_shared_columns_only_empties() {
    for (rows, shared) in [(11, columns(&["id"])), (0, columns(&[]))] {
        let plans = copy().refill_plans(None, rows, 10, &shared).unwrap();
        assert_eq!(plans.len(), 1, "{rows} rows, {shared:?}");
        assert_eq!(
            plans[0].statement,
            "DELETE FROM \"app_w__dev_a1\".\"orders\""
        );
        assert_eq!(
            (plans[0].live.clone(), plans[0].state),
            (pair("app_w", "orders"), ShadowState::Partial)
        );
    }
}

/// The sibling's migrated table has a column production does not (`region`),
/// production has one the migration dropped (`legacy`), and a column's case
/// differs. The refill keeps `region`, skips `legacy`, and copies the rest.
#[test]
fn a_refill_keeps_the_columns_the_siblings_migrations_made() {
    let db = duckdb::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE SCHEMA app_w; CREATE SCHEMA app_w__dev_a1; \
         CREATE TABLE app_w.orders (id INTEGER, legacy TEXT, \"Total\" INTEGER); \
         INSERT INTO app_w.orders VALUES (1, 'x', 10), (2, 'y', 20); \
         CREATE TABLE app_w__dev_a1.orders (region TEXT DEFAULT 'eu', total INTEGER, id INTEGER); \
         INSERT INTO app_w__dev_a1.orders (total, id) VALUES (99, 99);",
    )
    .unwrap();
    let first = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(first(&copy().preview_table_probe(None).unwrap()), 1);

    let probe = copy().shared_columns_probe(None).unwrap();
    let mut stmt = db.prepare(&probe).unwrap();
    let shared: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(shared, ["total", "id"], "the preview's order, shared only");

    for plan in copy().refill_plans(None, 2, 10, &shared).unwrap() {
        db.execute_batch(&plan.statement).unwrap();
    }
    let mut stmt = db
        .prepare("SELECT region, total, id FROM app_w__dev_a1.orders ORDER BY id")
        .unwrap();
    let rows: Vec<(String, i32, i32)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [("eu".to_string(), 10, 1), ("eu".to_string(), 20, 2)],
        "the old row is gone, region keeps its default"
    );
}
