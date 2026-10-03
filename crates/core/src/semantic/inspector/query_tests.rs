//! The SQL the inspector builds from a name it was given. No statement here
//! reaches an engine.

use oxy_shared::errors::OxyError;

use super::{build_inspect_queries, build_schema_summary_queries, build_schema_tables_query};
use crate::config::model::{ClickHouse, Database, DatabaseType, Postgres};

fn clickhouse() -> DatabaseType {
    DatabaseType::ClickHouse(ClickHouse::default())
}

fn postgres() -> DatabaseType {
    DatabaseType::Postgres(Postgres::default())
}

fn bigquery(config: &str) -> DatabaseType {
    serde_yaml::from_str(&format!("type: bigquery\n{config}")).expect("a bigquery database")
}

fn database(database_type: DatabaseType) -> Database {
    Database {
        name: "w".to_string(),
        database_type,
    }
}

fn tables_query(database_type: DatabaseType, schema: &str) -> String {
    build_schema_tables_query(&database(database_type), schema).expect("a supported engine")
}

#[test]
fn the_schema_param_is_escaped_per_engine_in_the_tables_query() {
    let schema = "x\\' OR 1=1 -- ";
    let ch = tables_query(clickhouse(), schema);
    assert!(
        ch.contains("WHERE database = 'x\\\\'' OR 1=1 -- '\n"),
        "{ch}"
    );
    let pg = tables_query(postgres(), schema);
    assert!(
        pg.contains("WHERE table_schema = 'x\\'' OR 1=1 -- '\n"),
        "{pg}"
    );
    assert!(
        tables_query(clickhouse(), "sales").contains("WHERE database = 'sales'\n"),
        "a plain schema is spelled as before"
    );
}

#[test]
fn a_bigquery_dataset_is_written_into_its_path_as_before() {
    let sql = tables_query(bigquery("dataset: d"), "sales_2024");
    assert!(
        sql.contains("FROM `sales_2024.INFORMATION_SCHEMA.COLUMNS`\n"),
        "{sql}"
    );
}

/// The `schema` request parameter went into a backtick-quoted path as it
/// came. A backtick ended the identifier; a dot named another project's
/// dataset; a backslash started an escape. Each is now the caller's error —
/// a 400 — and builds no query.
#[test]
fn a_bigquery_schema_param_that_is_not_a_dataset_builds_no_query() {
    for schema in [
        "x` WHERE 1=0 UNION ALL SELECT table_name, 1 FROM `other.INFORMATION_SCHEMA.TABLES",
        "other-project.sales",
        "sales\\",
        "",
    ] {
        let refused =
            build_schema_tables_query(&database(bigquery("dataset: d")), schema).expect_err(schema);
        assert!(
            matches!(refused, OxyError::ArgumentError(_)),
            "{schema:?}: {refused:?}"
        );
        assert_eq!(
            axum::http::StatusCode::from(refused),
            axum::http::StatusCode::BAD_REQUEST,
            "{schema:?}"
        );
    }
}

/// The same names are data on every other engine: a literal, escaped.
#[test]
fn the_same_names_are_a_literal_on_an_engine_that_compares_the_schema() {
    let sql = tables_query(postgres(), "other-project.sales`\\");
    assert!(
        sql.contains("WHERE table_schema = 'other-project.sales`\\'\n"),
        "{sql}"
    );
}

/// `config.yml`'s datasets go into the same kind of path. A dataset, a
/// project-qualified one and the `region-us` fallback are written; the
/// fallback is quoted once, where its own backticks used to make two.
#[test]
fn configured_bigquery_datasets_are_written_into_their_paths() {
    for (config, path) in [
        ("dataset: sales", "`sales.INFORMATION_SCHEMA."),
        (
            "dataset: my-project.sales",
            "`my-project.sales.INFORMATION_SCHEMA.",
        ),
        ("key_path: k.json", "`region-us.INFORMATION_SCHEMA."),
    ] {
        let db = database(bigquery(config));
        let summary = build_schema_summary_queries(&db).expect(config);
        let inspect = build_inspect_queries(&db).expect(config);
        assert_eq!((summary.len(), inspect.len()), (1, 1), "{config}");
        assert!(
            summary[0].contains(&format!("{path}TABLES`")),
            "{summary:?}"
        );
        assert!(
            inspect[0].contains(&format!("{path}COLUMNS`")),
            "{inspect:?}"
        );
    }
}

#[test]
fn a_configured_bigquery_dataset_that_could_leave_its_path_builds_no_query() {
    let db = database(bigquery("dataset: \"x` UNION ALL SELECT 1 FROM `y\""));
    for refused in [
        build_schema_summary_queries(&db).unwrap_err(),
        build_inspect_queries(&db).unwrap_err(),
    ] {
        assert!(
            matches!(refused, OxyError::ConfigurationError(_)),
            "{refused:?}"
        );
    }
}
