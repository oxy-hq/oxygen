use super::*;

fn col(table: &str, column: &str, data_type: &str, nullable: bool) -> CatalogColumn {
    CatalogColumn {
        table: table.into(),
        column: column.into(),
        data_type: data_type.into(),
        nullable,
    }
}

fn orders() -> Vec<CatalogColumn> {
    vec![
        col("orders", "id", "bigint", false),
        col("orders", "total", "integer", true),
    ]
}

#[test]
fn an_identical_schema_is_all_empty() {
    let got = diff(&orders(), &orders());
    assert_eq!(got, OltpDiff::default());
}

#[test]
fn an_added_column_is_listed_with_its_type() {
    let mut branch = orders();
    branch.push(col("orders", "note", "text", true));
    let got = diff(&orders(), &branch);
    assert_eq!(got.columns_added, ["orders.note text"]);
    assert!(got.columns_removed.is_empty() && got.columns_changed.is_empty());
}

#[test]
fn a_removed_column_is_listed_by_name() {
    let branch = vec![col("orders", "id", "bigint", false)];
    let got = diff(&orders(), &branch);
    assert_eq!(got.columns_removed, ["orders.total"]);
    assert!(got.columns_added.is_empty() && got.columns_changed.is_empty());
}

#[test]
fn a_type_change_shows_both_types() {
    let mut branch = orders();
    branch[1] = col("orders", "total", "numeric", true);
    let got = diff(&orders(), &branch);
    assert_eq!(got.columns_changed, ["orders.total: integer → numeric"]);
    assert!(got.columns_added.is_empty() && got.columns_removed.is_empty());
}

#[test]
fn a_nullability_change_is_a_change() {
    let mut branch = orders();
    branch[1] = col("orders", "total", "integer", false);
    let got = diff(&orders(), &branch);
    assert_eq!(
        got.columns_changed,
        ["orders.total: integer → integer not null"]
    );
}

#[test]
fn a_new_or_dropped_table_is_one_line_not_one_per_column() {
    let mut branch = orders();
    branch.push(col("refunds", "id", "bigint", false));
    branch.push(col("refunds", "reason", "text", true));
    let got = diff(&orders(), &branch);
    assert_eq!(got.tables_added, ["refunds"]);
    assert!(got.columns_added.is_empty(), "{got:?}");

    let back = diff(&branch, &orders());
    assert_eq!(back.tables_removed, ["refunds"]);
    assert!(back.columns_removed.is_empty(), "{back:?}");
}

#[test]
fn every_list_is_sorted() {
    let production = vec![
        col("b", "id", "bigint", false),
        col("c", "id", "bigint", false),
    ];
    let branch = vec![
        col("b", "id", "bigint", false),
        col("b", "z", "text", true),
        col("b", "a", "text", true),
        col("d", "id", "bigint", false),
        col("a", "id", "bigint", false),
    ];
    let got = diff(&production, &branch);
    assert_eq!(got.tables_added, ["a", "d"]);
    assert_eq!(got.tables_removed, ["c"]);
    assert_eq!(got.columns_added, ["b.a text", "b.z text"]);
}

/// `information_schema`'s `data_type` hides all of these; `format_type` does
/// not, and a narrowed column is exactly what fails on production's data.
#[test]
fn a_change_of_length_precision_or_element_type_is_a_change() {
    let before = vec![
        col("t", "name", "character varying(20)", true),
        col("t", "price", "numeric(10,2)", true),
        col("t", "tags", "integer[]", true),
    ];
    let after = vec![
        col("t", "name", "character varying(40)", true),
        col("t", "price", "numeric(12,2)", true),
        col("t", "tags", "bigint[]", true),
    ];
    let got = diff(&before, &after);
    assert_eq!(
        got.columns_changed,
        [
            "t.name: character varying(20) → character varying(40)",
            "t.price: numeric(10,2) → numeric(12,2)",
            "t.tags: integer[] → bigint[]",
        ]
    );
    assert!(got.columns_added.is_empty() && got.columns_removed.is_empty());
}

#[test]
fn the_catalog_query_reads_live_tables_and_columns_only() {
    for needle in [
        "format_type(a.atttypid, a.atttypmod)",
        "a.attnum > 0",
        "NOT a.attisdropped",
        "c.relkind IN ('r', 'p')",
        "n.nspname = $1",
    ] {
        assert!(COLUMNS_SQL.contains(needle), "{needle}");
    }
}
