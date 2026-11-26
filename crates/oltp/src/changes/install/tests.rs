//! [`super`]'s unit tests: the SQL an install, a commit and a removal run,
//! for `leases` in `app_x` (oid 16423), as [`crate::changes::shape`]'s test
//! fixture declares it.

use super::*;
use crate::changes::shape::leases;

fn owner(capture: &CaptureShape) -> Vec<String> {
    let spec = CaptureSpec {
        writer: "x",
        capture,
        schema_version: 1,
    };
    owner_install_sql(&spec, "app_x_rw", IMAGE_ROLE).expect("valid names")
}

const IMAGE_ROLE: &str = "oxy_changes_image_x";

/// The statements after the image setup: the bookkeeping, then the capture
/// functions.
fn after_image(sql: &[String]) -> &[String] {
    let stream = sql
        .iter()
        .position(|s| s.starts_with("INSERT INTO oxy_changes.stream"))
        .expect("the stream seed");
    &sql[stream..]
}

fn writer(capture: &CaptureShape) -> Vec<String> {
    let spec = CaptureSpec {
        writer: "x",
        capture,
        schema_version: 1,
    };
    writer_install_sql(&spec).expect("valid names")
}

fn functions(sql: &[String]) -> Vec<&String> {
    sql.iter()
        .filter(|s| s.starts_with("CREATE OR REPLACE FUNCTION \"app_x\""))
        .collect()
}

#[test]
fn functions_are_definers_with_a_fixed_path() {
    let sql = owner(&leases());
    let created = functions(&sql);
    assert_eq!(created.len(), 4, "{sql:#?}");
    for (function, op) in created.iter().zip(["i", "u", "d", "t"]) {
        assert!(
            function.starts_with(&format!(
                "CREATE OR REPLACE FUNCTION \"app_x\".\"_oxy_capture_16423_{op}\"() \
                 RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER \
                 SET search_path = pg_catalog, pg_temp AS $oxy$\n"
            )),
            "{function}"
        );
        assert!(function.ends_with("END\n$oxy$"), "{function}");
    }
}

#[test]
fn execute_goes_to_the_writer_alone() {
    let sql = owner(&leases());
    for op in ["i", "u", "d", "t"] {
        let function = format!("\"app_x\".\"_oxy_capture_16423_{op}\"()");
        let create = sql
            .iter()
            .position(|s| s.starts_with(&format!("CREATE OR REPLACE FUNCTION {function}")))
            .expect("created");
        let revoke = sql
            .iter()
            .position(|s| s == &format!("REVOKE ALL ON FUNCTION {function} FROM PUBLIC"))
            .expect("revoked from PUBLIC");
        let grant = sql
            .iter()
            .position(|s| s == &format!("GRANT EXECUTE ON FUNCTION {function} TO \"app_x_rw\""))
            .expect("granted to the writer");
        assert!(create < revoke && revoke < grant, "{sql:#?}");
    }
    assert_eq!(
        after_image(&sql)
            .iter()
            .filter(|s| s.starts_with("GRANT"))
            .count(),
        4,
        "nothing else is granted: {sql:#?}"
    );
}

/// The owner's install runs pinned to Postgres's own names, and makes sure
/// of the image function before it creates any capture function that calls
/// it.
#[test]
fn the_install_is_pinned_and_the_image_function_comes_first() {
    let sql = owner(&leases());
    assert_eq!(sql[0], "SET LOCAL search_path = pg_catalog, pg_temp");
    let image = image_sql("x", IMAGE_ROLE).expect("a valid role");
    assert_eq!(&sql[1..=image.len()], image.as_slice());
    assert_eq!(
        sql.len(),
        1 + image.len() + after_image(&sql).len(),
        "{sql:#?}"
    );
    assert!(
        owner_install_sql(
            &CaptureSpec {
                writer: "x",
                capture: &leases(),
                schema_version: 1,
            },
            "app_x_rw",
            "bad role"
        )
        .is_err()
    );
}

#[test]
fn triggers_are_statement_level_and_always_enabled() {
    let sql = writer(&leases());
    let f = |op: &str| format!("EXECUTE FUNCTION \"app_x\".\"_oxy_capture_16423_{op}\"()");
    assert_eq!(
        sql,
        vec![
            format!(
                "CREATE OR REPLACE TRIGGER oxy_capture_i AFTER INSERT ON \"app_x\".\"leases\" \
                 REFERENCING NEW TABLE AS oxy_new FOR EACH STATEMENT {}",
                f("i")
            ),
            format!(
                "CREATE OR REPLACE TRIGGER oxy_capture_u AFTER UPDATE ON \"app_x\".\"leases\" \
                 REFERENCING OLD TABLE AS oxy_old NEW TABLE AS oxy_new FOR EACH STATEMENT {}",
                f("u")
            ),
            format!(
                "CREATE OR REPLACE TRIGGER oxy_capture_d AFTER DELETE ON \"app_x\".\"leases\" \
                 REFERENCING OLD TABLE AS oxy_old FOR EACH STATEMENT {}",
                f("d")
            ),
            format!(
                "CREATE OR REPLACE TRIGGER oxy_capture_t AFTER TRUNCATE ON \"app_x\".\"leases\" \
                 FOR EACH STATEMENT {}",
                f("t")
            ),
            "ALTER TABLE \"app_x\".\"leases\" ENABLE ALWAYS TRIGGER oxy_capture_i".to_string(),
            "ALTER TABLE \"app_x\".\"leases\" ENABLE ALWAYS TRIGGER oxy_capture_u".to_string(),
            "ALTER TABLE \"app_x\".\"leases\" ENABLE ALWAYS TRIGGER oxy_capture_d".to_string(),
            "ALTER TABLE \"app_x\".\"leases\" ENABLE ALWAYS TRIGGER oxy_capture_t".to_string(),
        ]
    );
}

#[test]
fn a_quote_in_a_name_is_escaped() {
    assert_eq!(
        writer_uninstall_sql("app_x", "a\"b")[0],
        "DROP TRIGGER IF EXISTS oxy_capture_i ON \"app_x\".\"a\"\"b\""
    );
    assert_eq!(
        owner_drop_sql(16423, "app_\"x")[0],
        "DROP FUNCTION IF EXISTS \"app_\"\"x\".\"_oxy_capture_16423_i\"()"
    );

    // An install validates rather than escapes the table's name.
    let mut quoted = leases();
    quoted.shape.table = "a\"b".into();
    let spec = CaptureSpec {
        writer: "x",
        capture: &quoted,
        schema_version: 1,
    };
    assert!(writer_install_sql(&spec).is_err());
    assert!(owner_install_sql(&spec, "app_x_rw", IMAGE_ROLE).is_err());

    // A column name the app chose is a literal, quote doubled.
    let mut odd = leases();
    odd.shape.columns[1].name = "o'brien".into();
    odd.captured[1].name = "o'brien".into();
    let sql = owner(&odd).join(";\n");
    assert!(sql.contains("ARRAY['id','o''brien']::text[]"), "{sql}");
    assert!(sql.contains(r#""name":"o''brien""#), "{sql}");
}

#[test]
fn a_table_outside_the_writers_schema_is_refused() {
    let mut elsewhere = leases();
    elsewhere.shape.schema = "app_y".into();
    let spec = CaptureSpec {
        writer: "x",
        capture: &elsewhere,
        schema_version: 1,
    };
    assert_eq!(
        owner_install_sql(&spec, "app_x_rw", IMAGE_ROLE),
        Err(SchemaError::InvalidName("app_y".into()))
    );
    assert!(
        owner_install_sql(
            &CaptureSpec {
                writer: "X",
                ..spec
            },
            "app_x_rw",
            IMAGE_ROLE
        )
        .is_err()
    );
    assert!(
        owner_install_sql(
            &CaptureSpec {
                capture: &leases(),
                ..spec
            },
            "bad role",
            IMAGE_ROLE
        )
        .is_err()
    );
}

/// The stream is seeded only when empty, the table is held unguarded
/// until the commit, and a shape already recorded is kept.
#[test]
fn install_order_bookkeeping() {
    let all = owner(&leases());
    let sql = after_image(&all);
    assert_eq!(
        sql[0],
        "INSERT INTO oxy_changes.stream (generation, capturing) VALUES (1, true) \
         ON CONFLICT (one) DO NOTHING"
    );
    assert!(
        sql[1].contains("VALUES (16423::oid, 'app_x', 'leases', 'x', 1, false)"),
        "{}",
        sql[1]
    );
    assert!(sql[1].ends_with("installed = false"), "{}", sql[1]);
    assert!(
        sql[2].starts_with("INSERT INTO oxy_changes.shapes"),
        "{}",
        sql[2]
    );
    assert!(
        sql[2].contains(
            r#"'[{"attnum":1,"name":"id","type":"bigint"},{"attnum":2,"name":"name","type":"text"}]'::jsonb"#
        ),
        "only the captured columns: {}",
        sql[2]
    );
    assert!(
        sql[2].contains("ARRAY['id']::text[], ARRAY['contact_email']::text[])"),
        "{}",
        sql[2]
    );
    assert!(sql[2].ends_with("ON CONFLICT (relid, schema_version) DO NOTHING"));
    assert_eq!(
        owner_commit_sql(16423, 2),
        "UPDATE oxy_changes.tables SET installed = true, schema_version = 2 \
         WHERE relid = 16423::oid"
    );
    assert_eq!(
        owner_uninstall_sql(16423),
        "UPDATE oxy_changes.tables SET installed = false WHERE relid = 16423::oid"
    );
    let dropped = owner_drop_sql(16423, "app_x");
    assert_eq!(dropped.len(), 5);
    assert_eq!(
        dropped[4],
        "DELETE FROM oxy_changes.tables WHERE relid = 16423::oid"
    );
    assert!(!dropped.iter().any(|s| s.contains("shapes")), "{dropped:?}");
    assert_eq!(writer_uninstall_sql("app_x", "leases").len(), 4);
}

#[test]
fn installed_sql_reads_the_tables_the_step_creates() {
    assert!(INSTALLED_SQL.contains(&format!("FROM {CHANGES_TABLES_TABLE} t")));
    assert!(INSTALLED_SQL.contains(&format!("FROM {CHANGES_SHAPES_TABLE} sh")));
    assert!(INSTALLED_SQL.contains("WHERE t.writer = $1"));
}

#[test]
fn the_dollar_quote_tag_is_one_the_body_lacks() {
    assert_eq!(dollar_quote("\nx\n"), "$oxy$\nx\n$oxy$");
    assert_eq!(
        dollar_quote("'$oxy$' '$oxy1$'"),
        "$oxy2$'$oxy$' '$oxy1$'$oxy2$"
    );
}
