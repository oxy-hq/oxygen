//! [`super`]'s unit tests: validating a declaration against a shape, the
//! columns it captures, and when an install bumps the schema version.

use super::*;

fn shape(columns: Vec<Column>, key: &[&str]) -> TableShape {
    TableShape {
        relid: 1,
        schema: "app_x".into(),
        table: "leases".into(),
        columns,
        primary_key: key.iter().map(|k| k.to_string()).collect(),
    }
}

fn decl(exclude: &[&str]) -> DeclaredTable {
    DeclaredTable {
        name: "leases".into(),
        exclude: exclude.iter().map(|e| e.to_string()).collect(),
    }
}

fn refusal(decl: &DeclaredTable, shape: Option<&TableShape>) -> String {
    validate(decl, shape).expect_err("refused").to_string()
}

#[test]
fn validate_refuses_each_decl_error() {
    let id = || column(1, "id", "bigint");
    let keyed = shape(vec![id(), column(2, "note", "text")], &["id"]);

    assert_eq!(
        refusal(&decl(&[]), None),
        "oltpChanges declares table \"leases\", which does not exist after the app's \
         migrations: create it in a migration, or remove it from oltpChanges.tables"
    );
    assert_eq!(
        refusal(&decl(&[]), Some(&shape(vec![id()], &[]))),
        "oltpChanges table \"leases\" has no primary key, and every change is keyed by it: \
         add one in a migration, or remove the table from oltpChanges.tables"
    );
    assert_eq!(
        refusal(
            &decl(&[]),
            Some(&shape(vec![id(), column(2, "_seq", "bigint")], &["id"]))
        ),
        "oltpChanges table \"leases\" has a column named \"_seq\", a name the change table \
         keeps for its own: rename the column in a migration, or remove the table from \
         oltpChanges.tables"
    );
    assert_eq!(
        refusal(&decl(&["nope"]), Some(&keyed)),
        "oltpChanges table \"leases\" excludes \"nope\", which is not one of its columns: \
         correct the name, or remove it from exclude"
    );
    assert_eq!(
        refusal(&decl(&["id"]), Some(&keyed)),
        "oltpChanges table \"leases\" excludes \"id\", which is part of its primary key, and \
         every change is keyed by it: remove it from exclude"
    );
    for reserved in RESERVED_COLUMNS {
        let with = shape(vec![id(), column(2, reserved, "text")], &["id"]);
        assert!(
            matches!(
                validate(&decl(&[]), Some(&with)),
                Err(DeclError::ReservedColumn { .. })
            ),
            "{reserved}"
        );
    }
}

#[test]
fn validate_refuses_a_table_named_for_the_relays_relations() {
    let keyed = shape(vec![column(1, "id", "bigint")], &["id"]);
    for name in [
        "_progress",
        "_baselines",
        "leases__changes",
        "_Baselines",
        "Leases__CHANGES",
    ] {
        let declared = DeclaredTable {
            name: name.into(),
            exclude: Vec::new(),
        };
        let error = refusal(&declared, Some(&keyed));
        assert!(error.contains(&format!("\"{name}\"")), "{error}");
        assert!(error.contains("oltp_<writer>"), "{error}");
        assert!(
            matches!(
                validate(&declared, Some(&keyed)),
                Err(DeclError::ReservedTable { .. })
            ),
            "{name}"
        );
    }
}

#[test]
fn captured_columns_drop_excluded_ones_in_attnum_order() {
    let unordered = shape(
        vec![
            column(4, "phone", "text"),
            column(1, "id", "bigint"),
            column(3, "email", "text"),
            column(2, "rent", "numeric(10,2)"),
        ],
        &["id"],
    );
    let capture = validate(&decl(&["email", "phone"]), Some(&unordered)).unwrap();
    assert_eq!(
        capture.captured,
        vec![
            column(1, "id", "bigint"),
            column(2, "rent", "numeric(10,2)")
        ]
    );
    assert_eq!(capture.excluded(), vec!["email", "phone"]);
    assert_eq!(leases().excluded(), vec!["contact_email"]);
}

#[test]
fn next_schema_version_bumps_only_on_change() {
    let capture = leases();
    let key = vec!["id".to_string()];
    assert_eq!(next_schema_version(None, &capture), 1);

    let latest =
        |columns: &[Column], key: &[String]| next_schema_version(Some((3, columns, key)), &capture);

    let same = capture.captured.clone();
    assert_eq!(latest(&same, &key), 3, "unchanged: the same version");

    let mut renamed = capture.captured.clone();
    renamed[1].name = "title".into();
    assert_eq!(latest(&renamed, &key), 4, "a renamed column");

    let mut retyped = capture.captured.clone();
    retyped[1].type_name = "varchar(40)".into();
    assert_eq!(latest(&retyped, &key), 4, "a retyped column");

    let mut added = capture.captured.clone();
    added.push(column(4, "rent", "numeric(10,2)"));
    assert_eq!(latest(&added, &key), 4, "a captured column more or less");

    let other_key = vec!["name".to_string()];
    assert_eq!(latest(&same, &other_key), 4, "a changed key");
}

#[test]
fn shape_rows_become_a_shape_with_its_key_in_key_order() {
    let row = |attnum: i16, name: &str, key_position: Option<i32>| ShapeRow {
        relid: 16423,
        schema: "app_x".into(),
        table: "lines".into(),
        attnum,
        name: name.into(),
        type_name: "int".into(),
        key_position,
    };
    assert_eq!(shape_from_rows(&[]), None, "no rows: no table");
    let shape = shape_from_rows(&[
        row(2, "line", Some(2)),
        row(1, "order_id", Some(1)),
        row(3, "qty", None),
    ])
    .unwrap();
    assert_eq!((shape.relid, shape.table.as_str()), (16423, "lines"));
    assert_eq!(
        shape.columns.iter().map(|c| c.attnum).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(shape.primary_key, ["order_id", "line"]);
}

/// The relay reads `shapes.columns` back as these: `{attnum, name, type}`.
#[test]
fn a_column_serializes_as_the_shapes_row_records_it() {
    let json = serde_json::to_value(column(2, "rent", "numeric(10,2)")).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"attnum": 2, "name": "rent", "type": "numeric(10,2)"})
    );
    let back: Column = serde_json::from_value(json).unwrap();
    assert_eq!(back, column(2, "rent", "numeric(10,2)"));
}
