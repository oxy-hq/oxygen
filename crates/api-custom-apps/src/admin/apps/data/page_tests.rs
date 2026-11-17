//! The browser's confinement and caps, without a database: a name a request
//! carries reaches SQL only as the catalog spells it, a page never holds more
//! than its rows or its bytes allow, and a value never leaves whole when it
//! is large.

use axum::http::StatusCode;
use serde_json::json;

use super::catalog::{Column, ColumnKind, RawColumn, Table, assemble, find_table, kind_of};
use super::page::{
    BYTES_SHOWN, Cell, DEFAULT_ROWS, Dir, MAX_ROWS, OrderTerm, PAGE_BUDGET_BYTES, PageRequest,
    TEXT_CHARS, cell, order_terms, quote_ident, rows_per_page, rows_sql,
};

fn column(name: &str, kind: ColumnKind, primary_key: bool) -> Column {
    Column {
        name: name.into(),
        data_type: "text".into(),
        kind,
        nullable: !primary_key,
        primary_key,
        sortable: true,
    }
}

/// A column of a type Postgres has no ordering for, as the catalog reports one.
fn unsortable(name: &str, data_type: &str, kind: ColumnKind, primary_key: bool) -> Column {
    Column {
        data_type: data_type.into(),
        sortable: false,
        ..column(name, kind, primary_key)
    }
}

fn orders() -> Table {
    Table {
        name: "orders".into(),
        approx_rows: Some(1200),
        columns: vec![
            column("id", ColumnKind::Other, true),
            column("note", ColumnKind::Text, false),
            unsortable("payload", "json", ColumnKind::Json, false),
            column("receipt", ColumnKind::Bytes, false),
        ],
    }
}

fn catalog() -> Vec<Table> {
    vec![
        orders(),
        Table {
            name: "Customers".into(),
            approx_rows: None,
            columns: vec![column("id", ColumnKind::Other, true)],
        },
    ]
}

// ── Confinement ─────────────────────────────────────────────────────────────

/// A table is found by its exact name in the app's own catalog. Another
/// schema's table, a catalog table, a quoted or qualified spelling, an
/// injection and a different case are all simply not there.
#[test]
fn only_a_table_of_the_apps_own_schema_is_found() {
    let catalog = catalog();
    assert_eq!(
        find_table(&catalog, "orders").map(|t| t.name.as_str()),
        Some("orders")
    );
    assert!(find_table(&catalog, "Customers").is_some());
    for hostile in [
        "app_other.orders",
        "\"app_other\".\"orders\"",
        "public.orders",
        "pg_catalog.pg_authid",
        "pg_authid",
        "orders; DROP TABLE orders; --",
        "orders\" UNION SELECT * FROM app_other.secrets --",
        "\"orders\"",
        "ORDERS",
        "customers",
        "",
    ] {
        assert!(
            find_table(&catalog, hostile).is_none(),
            "{hostile:?} must not be found"
        );
    }
}

/// A column to order by must be one of the table's: no expression, no other
/// table's column, no injection.
#[test]
fn only_a_column_of_the_table_orders_a_page() {
    let table = orders();
    for hostile in [
        "missing",
        "id; DROP TABLE orders",
        "(SELECT 1)",
        "ID",
        "orders.id",
    ] {
        let asked = (hostile.to_string(), Dir::Asc);
        let refused = order_terms(&table, Some(&asked)).expect_err("refused");
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{hostile:?}");
        assert_eq!(refused.code, "unknown_column");
    }
}

/// The SQL names the schema and table the catalog returned, quotes every
/// identifier, and binds the page: no text of the request reaches it.
#[test]
fn the_page_sql_is_built_from_catalog_names_only() {
    let table = orders();
    let order = order_terms(&table, Some(&("note".to_string(), Dir::Desc))).expect("ordered");
    let sql = rows_sql("app_ops", &table, &order);
    // The whole statement: every name in it is the catalog's, quoted, and the
    // page is two bound parameters. One relation, one statement.
    assert_eq!(
        sql,
        "SELECT \
         pg_catalog.left(\"id\"::pg_catalog.text, 257), NULL::pg_catalog.int8, \
         pg_catalog.left(\"note\"::pg_catalog.text, 257), \
         pg_catalog.octet_length(\"note\"::pg_catalog.text)::pg_catalog.int8, \
         pg_catalog.left(\"payload\"::pg_catalog.text, 257), NULL::pg_catalog.int8, \
         pg_catalog.encode(pg_catalog.substring(\"receipt\" FROM 1 FOR 32), 'hex'), \
         pg_catalog.octet_length(\"receipt\")::pg_catalog.int8 \
         FROM \"app_ops\".\"orders\" ORDER BY \"note\" DESC, \"id\" ASC LIMIT $1 OFFSET $2"
    );
    assert_eq!(sql.matches(" FROM \"").count(), 1, "one relation: {sql}");
    assert!(!sql.contains(';'), "{sql}");
}

#[test]
fn an_identifier_with_a_quote_in_it_stays_one_identifier() {
    assert_eq!(quote_ident("orders"), "\"orders\"");
    assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    let table = Table {
        name: "a\"; DROP TABLE x; --".into(),
        approx_rows: None,
        columns: vec![column("c\"", ColumnKind::Text, false)],
    };
    let sql = rows_sql("app_ops", &table, &[]);
    assert!(
        sql.contains("FROM \"app_ops\".\"a\"\"; DROP TABLE x; --\""),
        "{sql}"
    );
    assert!(sql.contains("\"c\"\"\""), "{sql}");
}

#[test]
fn a_page_is_ordered_by_the_primary_key_to_keep_its_boundary_stable() {
    let table = orders();
    let default = order_terms(&table, None).expect("ordered");
    assert_eq!(
        default,
        vec![OrderTerm {
            column: "id".into(),
            dir: Dir::Asc
        }]
    );
    // Asking for the key itself does not repeat it.
    let by_id = order_terms(&table, Some(&("id".to_string(), Dir::Desc))).expect("ordered");
    assert_eq!(
        by_id,
        vec![OrderTerm {
            column: "id".into(),
            dir: Dir::Desc
        }]
    );
    let keyless = Table {
        columns: vec![column("note", ColumnKind::Text, false)],
        ..orders()
    };
    assert!(order_terms(&keyless, None).expect("ordered").is_empty());
    assert!(!rows_sql("app_ops", &keyless, &[]).contains("ORDER BY"));
}

/// A column Postgres cannot order (`json`, `xml`, `point`) is refused before
/// the read, with the column's type, rather than failing in Postgres with
/// `42883` and reading as an outage.
#[test]
fn a_column_postgres_cannot_order_is_refused_before_the_read() {
    let table = orders();
    for dir in [Dir::Asc, Dir::Desc] {
        let asked = ("payload".to_string(), dir);
        let refused = order_terms(&table, Some(&asked)).expect_err("refused");
        assert_eq!(refused.status, StatusCode::BAD_REQUEST);
        assert_eq!(refused.code, "unsortable_column");
        assert!(refused.message.contains("json"), "{}", refused.message);
    }
    // A sortable column beside it is still ordered by.
    assert!(order_terms(&table, Some(&("note".to_string(), Dir::Asc))).is_ok());
}

/// A key column Postgres cannot order is left out of the default order and
/// the tie break, so a page of its table still reads.
#[test]
fn a_key_postgres_cannot_order_is_left_out_of_the_order() {
    let table = Table {
        columns: vec![
            unsortable("spot", "point", ColumnKind::Other, true),
            column("id", ColumnKind::Other, true),
            column("note", ColumnKind::Text, false),
        ],
        ..orders()
    };
    let id = OrderTerm {
        column: "id".into(),
        dir: Dir::Asc,
    };
    assert_eq!(
        order_terms(&table, None).expect("ordered"),
        vec![id.clone()]
    );
    let by_note = order_terms(&table, Some(&("note".to_string(), Dir::Desc))).expect("ordered");
    let note = OrderTerm {
        column: "note".into(),
        dir: Dir::Desc,
    };
    assert_eq!(by_note, vec![note, id]);
    let only_point = Table {
        columns: vec![unsortable("spot", "point", ColumnKind::Other, true)],
        ..orders()
    };
    assert!(order_terms(&only_point, None).expect("ordered").is_empty());
}

/// Every value is read through a cap in SQL — `left` for text, `substring`
/// for bytes — so a large value never leaves the database whole.
#[test]
fn every_value_is_capped_in_sql() {
    let sql = rows_sql("app_ops", &orders(), &[]);
    let cut = TEXT_CHARS + 1;
    assert!(
        sql.contains(&format!(
            "pg_catalog.left(\"note\"::pg_catalog.text, {cut})"
        )),
        "{sql}"
    );
    assert!(
        sql.contains(&format!(
            "pg_catalog.left(\"payload\"::pg_catalog.text, {cut})"
        )),
        "{sql}"
    );
    assert!(
        sql.contains(&format!("pg_catalog.left(\"id\"::pg_catalog.text, {cut})")),
        "{sql}"
    );
    assert!(
        sql.contains(&format!(
            "pg_catalog.substring(\"receipt\" FROM 1 FOR {BYTES_SHOWN})"
        )),
        "{sql}"
    );
}

// ── Row caps ────────────────────────────────────────────────────────────────

#[test]
fn a_page_is_fifty_rows_by_default_and_never_more_than_two_hundred() {
    let parse = |limit| PageRequest::parse(limit, None, None, None).expect("a page");
    assert_eq!(parse(None).limit, DEFAULT_ROWS);
    assert_eq!(parse(Some(1)).limit, 1);
    assert_eq!(parse(Some(200)).limit, MAX_ROWS);
    assert_eq!(parse(Some(201)).limit, MAX_ROWS);
    assert_eq!(parse(Some(u32::MAX)).limit, MAX_ROWS);
    let zero = PageRequest::parse(Some(0), None, None, None).expect_err("refused");
    assert_eq!(
        (zero.status, zero.code),
        (StatusCode::BAD_REQUEST, "invalid_page")
    );
}

#[test]
fn a_direction_is_asc_or_desc() {
    let page = PageRequest::parse(None, Some(40), Some("note"), Some("desc")).expect("a page");
    assert_eq!(page.order, Some(("note".to_string(), Dir::Desc)));
    assert_eq!(page.offset, 40);
    let refused = PageRequest::parse(None, None, Some("note"), Some("sideways")).expect_err("no");
    assert_eq!(refused.code, "invalid_page");
    // No column, no order — a direction alone orders nothing.
    let bare = PageRequest::parse(None, None, None, Some("desc")).expect("a page");
    assert_eq!(bare.order, None);
}

// ── Byte caps ───────────────────────────────────────────────────────────────

/// A wide table gets fewer rows, so a page of cells all at their cap still
/// fits the budget; a narrow one gets what it asked for.
#[test]
fn a_page_of_capped_cells_fits_the_byte_budget() {
    assert_eq!(rows_per_page(MAX_ROWS, 4), MAX_ROWS);
    assert_eq!(rows_per_page(10, 4), 10);
    for columns in [1, 10, 40, 100, 400, 1600] {
        let rows = rows_per_page(MAX_ROWS, columns) as usize;
        assert!(rows >= 1, "{columns} columns still read a row");
        assert!(rows <= MAX_ROWS as usize);
        let worst = rows * columns * ((TEXT_CHARS + 1) * 4 + 16);
        assert!(
            worst <= PAGE_BUDGET_BYTES || rows == 1,
            "{columns} columns x {rows} rows = {worst} bytes"
        );
    }
    assert!(
        rows_per_page(MAX_ROWS, 100) < MAX_ROWS,
        "a 100-column page is narrowed"
    );
}

#[test]
fn a_long_text_is_cut_at_the_cap_on_a_character_boundary() {
    let long: String = "é".repeat(TEXT_CHARS + 1);
    let Cell::Text {
        text,
        truncated,
        size,
    } = cell(ColumnKind::Text, Some(long), Some(40_000))
    else {
        panic!("text");
    };
    assert!(truncated);
    assert_eq!(text.chars().count(), TEXT_CHARS);
    assert_eq!(size, Some(40_000));
    let short = cell(ColumnKind::Text, Some("hello".into()), Some(5));
    assert_eq!(json!(short), json!({ "text": "hello" }));
}

#[test]
fn a_bytea_sends_its_first_bytes_and_its_size() {
    let big = cell(
        ColumnKind::Bytes,
        Some("00ff".repeat(BYTES_SHOWN / 2)),
        Some(1 << 20),
    );
    assert_eq!(
        json!(big),
        json!({ "hex": "00ff".repeat(BYTES_SHOWN / 2), "size": 1 << 20, "truncated": true })
    );
    let small = cell(ColumnKind::Bytes, Some("cafe".into()), Some(2));
    assert_eq!(
        json!(small),
        json!({ "hex": "cafe", "size": 2, "truncated": false })
    );
}

#[test]
fn json_is_rendered_as_json_when_it_fits_and_cut_as_text_when_not() {
    let fits = cell(ColumnKind::Json, Some(r#"{"a": [1, 2]}"#.into()), None);
    assert_eq!(json!(fits), json!({ "json": { "a": [1, 2] } }));
    let long = format!("[{}]", "1,".repeat(TEXT_CHARS));
    let Cell::Text {
        truncated, text, ..
    } = cell(ColumnKind::Json, Some(long), None)
    else {
        panic!("a cut JSON value is text");
    };
    assert!(truncated);
    assert_eq!(text.chars().count(), TEXT_CHARS);
}

/// SQL `NULL` is JSON `null`, distinct from the text "NULL" and from "".
#[test]
fn null_is_not_a_string() {
    for kind in [
        ColumnKind::Text,
        ColumnKind::Json,
        ColumnKind::Bytes,
        ColumnKind::Other,
    ] {
        assert_eq!(json!(cell(kind, None, None)), serde_json::Value::Null);
    }
    assert_eq!(
        json!(cell(ColumnKind::Text, Some("NULL".into()), None)),
        json!({ "text": "NULL" })
    );
    assert_eq!(
        json!(cell(ColumnKind::Text, Some(String::new()), None)),
        json!({ "text": "" })
    );
}

// ── Catalog ─────────────────────────────────────────────────────────────────

#[test]
fn a_column_renders_by_its_base_type() {
    assert_eq!(kind_of("bytea", "U"), ColumnKind::Bytes);
    assert_eq!(kind_of("jsonb", "U"), ColumnKind::Json);
    assert_eq!(kind_of("json", "U"), ColumnKind::Json);
    assert_eq!(kind_of("varchar", "S"), ColumnKind::Text);
    assert_eq!(kind_of("int8", "N"), ColumnKind::Other);
    assert_eq!(
        kind_of("_text", "A"),
        ColumnKind::Other,
        "an array is its text form"
    );
}

/// The catalog's `sortable` flag reaches the console with each column, so
/// the grid offers a sort only where Postgres can do one.
#[test]
fn the_catalog_reports_whether_each_column_can_be_ordered() {
    let tables = assemble(
        vec![("orders".into(), 10.0)],
        vec![
            RawColumn {
                table: "orders".into(),
                column: column("id", ColumnKind::Other, true),
            },
            RawColumn {
                table: "orders".into(),
                column: unsortable("payload", "json", ColumnKind::Json, false),
            },
        ],
    );
    let wire = json!(tables[0].columns);
    assert_eq!(wire[0]["sortable"], true);
    assert_eq!(wire[1]["sortable"], false);
    assert_eq!(wire[1]["type"], "json");
}

#[test]
fn the_catalog_groups_columns_under_their_tables_and_never_invents_a_count() {
    let raw = |table: &str, name: &str| RawColumn {
        table: table.into(),
        column: column(name, ColumnKind::Other, false),
    };
    let tables = assemble(
        vec![
            ("a".into(), -1.0),
            ("b".into(), 41.6),
            ("empty".into(), 0.0),
        ],
        vec![
            raw("a", "x"),
            raw("b", "y"),
            raw("a", "z"),
            raw("not_listed", "w"),
        ],
    );
    assert_eq!(tables.len(), 3);
    assert_eq!(tables[0].approx_rows, None, "never analyzed is not zero");
    assert_eq!(tables[1].approx_rows, Some(42));
    assert_eq!(tables[2].approx_rows, Some(0));
    let names = |t: &Table| t.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&tables[0]), ["x", "z"]);
    assert_eq!(names(&tables[1]), ["y"]);
    assert!(tables[2].columns.is_empty());
}
