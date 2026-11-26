//! Unit tests of the type mapping and the change-table plan.

use oxy_oltp::changes::{Column, RESERVED_COLUMNS};

use super::{ChangeTable, DuckType, ShapeVersion, Unsupported};
use crate::oltp_changes::relay::render::{ShipRow, insert_sql, table_ddl};

fn version(schema_version: i32, columns: &[(i16, &str, &str)]) -> ShapeVersion {
    ShapeVersion {
        schema_version,
        captured: columns
            .iter()
            .map(|(attnum, name, ty)| Column {
                attnum: *attnum,
                name: name.to_string(),
                type_name: ty.to_string(),
            })
            .collect(),
        primary_key: Vec::new(),
    }
}

fn names(table: &ChangeTable) -> Vec<&str> {
    table.columns.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn pg_types_map_to_duckdb() {
    use DuckType::*;
    let cases = [
        ("smallint", SmallInt),
        ("integer", Integer),
        ("bigint", BigInt),
        ("real", Float),
        ("double precision", Double),
        ("numeric(10,2)", Decimal(10, 2)),
        ("numeric(38,0)", Decimal(38, 0)),
        ("boolean", Boolean),
        ("uuid", Uuid),
        ("date", Date),
        ("timestamp without time zone", Timestamp),
        ("timestamp(3) without time zone", Timestamp),
        ("timestamp with time zone", TimestampTz),
        ("timestamp(6) with time zone", TimestampTz),
        ("time without time zone", Time),
        ("time(3) without time zone", Time),
        ("integer[]", List(Box::new(Integer))),
        ("timestamp with time zone[]", List(Box::new(TimestampTz))),
        ("text[]", Varchar),
        ("character varying(20)[]", Varchar),
        ("text", Varchar),
        ("character varying(20)", Varchar),
        ("character(1)", Varchar),
        ("json", Json),
        ("jsonb", Json),
        ("jsonb[]", Json),
        ("json[]", Json),
        ("numeric", Varchar),
        ("numeric(50,2)", Varchar),
        ("numeric(5,-2)", Varchar),
        ("interval", Varchar),
        ("bytea", Varchar),
        ("time with time zone", Varchar),
        ("lease_status", Varchar),
        ("lease_status[]", Varchar),
    ];
    for (pg, duck) in cases {
        assert_eq!(DuckType::of(pg), duck, "{pg}");
    }
    assert_eq!(Decimal(10, 2).sql(), "DECIMAL(10,2)");
    assert_eq!(TimestampTz.sql(), "TIMESTAMPTZ");
    assert_eq!(Json.sql(), "VARCHAR");
    assert_eq!(List(Box::new(Integer)).sql(), "INTEGER[]");
}

#[test]
fn an_added_column_joins_the_plan() {
    let v1 = version(1, &[(1, "id", "integer"), (2, "name", "text")]);
    let v2 = version(
        2,
        &[
            (1, "id", "integer"),
            (2, "name", "text"),
            (3, "rent", "numeric(10,2)"),
        ],
    );
    // In schema_version order, whatever order the shapes come in.
    let table = ChangeTable::plan("leases", &[v2, v1]).expect("a plan");
    assert_eq!(table.table, "leases");
    assert_eq!(names(&table), ["id", "name", "rent"]);
    assert_eq!(table.columns[2].attnum, 3);
    assert_eq!(table.columns[2].ty, DuckType::Decimal(10, 2));
    assert!(table.column(1, "rent").is_none(), "v1 never captured rent");
    assert_eq!(table.column(2, "rent").map(|c| c.attnum), Some(3));
    assert!(table.column(3, "id").is_none(), "no such shape");
}

#[test]
fn a_dropped_column_stays_in_the_plan() {
    let v1 = version(1, &[(1, "id", "integer"), (2, "note", "text")]);
    let v2 = version(2, &[(1, "id", "integer")]);
    let table = ChangeTable::plan("leases", &[v1, v2]).expect("a plan");
    assert_eq!(names(&table), ["id", "note"]);
    assert!(table.column(1, "note").is_some());
    assert!(table.column(2, "note").is_none(), "null from then on");
}

#[test]
fn a_rename_is_unsupported() {
    let v1 = version(1, &[(1, "id", "integer"), (2, "note", "text")]);
    let v2 = version(2, &[(1, "id", "integer"), (2, "memo", "text")]);
    let err = ChangeTable::plan("leases", &[v1, v2]).unwrap_err();
    assert_eq!(
        err,
        Unsupported::Renamed {
            attnum: 2,
            from: "note".into(),
            to: "memo".into()
        }
    );
    assert!(err.to_string().contains("renamed"), "{err}");
}

#[test]
fn a_type_change_is_unsupported() {
    // Widening too, though both are VARCHAR in DuckDB.
    let v1 = version(1, &[(1, "code", "character varying(20)")]);
    let v2 = version(2, &[(1, "code", "character varying(40)")]);
    let err = ChangeTable::plan("leases", &[v1, v2]).unwrap_err();
    assert_eq!(
        err,
        Unsupported::Retyped {
            column: "code".into(),
            from: "character varying(20)".into(),
            to: "character varying(40)".into()
        }
    );
    assert!(err.to_string().contains("type"), "{err}");
}

#[test]
fn a_name_reused_by_another_attribute_is_unsupported() {
    // `note` dropped, then added again: a new attribute under the old name.
    let v1 = version(1, &[(1, "id", "integer"), (2, "note", "text")]);
    let v2 = version(2, &[(1, "id", "integer"), (3, "note", "text")]);
    let err = ChangeTable::plan("leases", &[v1, v2]).unwrap_err();
    assert_eq!(
        err,
        Unsupported::NameReused {
            column: "note".into()
        }
    );
    assert!(err.to_string().contains("note"), "{err}");
}

/// DuckDB's identifiers ignore case, so `Note` and `note` would be one
/// change-table column: two attributes so named reuse one name, in one shape
/// or across two.
#[test]
fn names_that_differ_only_in_case_are_one_name() {
    let both = version(1, &[(1, "note", "text"), (2, "Note", "text")]);
    let reused = Unsupported::NameReused {
        column: "Note".into(),
    };
    assert_eq!(ChangeTable::plan("leases", &[both]), Err(reused.clone()));
    let v1 = version(1, &[(1, "note", "text")]);
    let v2 = version(2, &[(2, "Note", "text")]);
    assert_eq!(ChangeTable::plan("leases", &[v1, v2]), Err(reused));
}

/// A change table that already lists a column in another case has it: no
/// `ADD COLUMN` for it.
#[test]
fn an_existing_column_is_found_without_case() {
    let table = ChangeTable::plan("leases", &[version(1, &[(1, "id", "integer")])]);
    let table = table.expect("a plan");
    let upper = ["ID".to_owned()];
    assert!(table_ddl("shop", &table, Some(upper.as_slice())).is_empty());
    let other = ["_seq".to_owned()];
    let added = table_ddl("shop", &table, Some(other.as_slice()));
    assert_eq!(added.len(), 1, "{added:?}");
}

#[test]
fn an_excluded_column_is_not_planned() {
    // `ssn` is live but excluded: the relay leaves it out of `captured`.
    let live: [(i16, &str, &str); 3] = [
        (1, "id", "integer"),
        (2, "ssn", "text"),
        (3, "name", "text"),
    ];
    let excluded = ["ssn"];
    let captured: Vec<_> = live
        .into_iter()
        .filter(|(_, name, _)| !excluded.contains(name))
        .collect();
    let table = ChangeTable::plan("tenants", &[version(1, &captured)]).expect("a plan");
    assert_eq!(names(&table), ["id", "name"]);
    assert!(table.column(1, "ssn").is_none());
}

#[test]
fn a_truncate_only_table_plans_no_attribute_columns() {
    let table = ChangeTable::plan("gone", &[]).expect("a plan");
    assert_eq!(table.table, "gone");
    assert!(table.columns.is_empty());

    let ddl = table_ddl("shop", &table, None);
    let reserved = RESERVED_COLUMNS.map(|c| format!("\"{c}\""));
    assert_eq!(ddl.len(), 1);
    assert!(
        ddl[0].starts_with(
            "CREATE TABLE IF NOT EXISTS \"oltp_shop\".\"gone__changes\" (\"_generation\" BIGINT,"
        ),
        "{}",
        ddl[0]
    );
    assert!(ddl[0].ends_with("\"_invocation_id\" UUID)"), "{}", ddl[0]);
    let none: &[String] = &[];
    assert!(table_ddl("shop", &table, Some(none)).is_empty());

    let truncate = ShipRow {
        generation: 1,
        seq: 9,
        txid: 77,
        op: "truncate",
        changed_at: chrono::DateTime::from_timestamp(1_780_000_000, 0).expect("a time"),
        actor: "unverified".into(),
        invocation: None,
        schema_version: 4,
        image: None,
    };
    let insert = insert_sql("shop", &table, 1, &[truncate]).expect("renders");
    let columns = format!("({})", reserved.join(", "));
    assert!(
        insert.contains(&format!("\"gone__changes\" {columns}")),
        "{insert}"
    );
    assert!(insert.contains("'truncate'"), "{insert}");
}
