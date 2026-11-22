//! What a pod with no working copy can do with a workspace, and which
//! database a query would need the working copy for.
//!
//! **The rule: a database is never a property of the whole workspace.** A
//! compiled workspace is served by any pod, whatever its databases are. A
//! database this pod cannot open fails the queries that name it, and only
//! those, and says which database it is. It never routes a request elsewhere
//! and never refuses one that does not query it. INC-174 and INC-175 broke
//! that rule: one file DuckDB in a workspace's config (compiled before the
//! compile gate refused them, 2026-10-29) sent every custom-app function call
//! on the workspace to the Factory, and with the Factory at zero replicas
//! every one answered 503, though none of them read that database.
//! [`Servability`] has no variant for a database, so nothing can bring that
//! back without changing this type.
//!
//! A database is *reachable from anywhere* when it can be queried without the
//! working copy: a remote warehouse, a DuckDB with a compiler-made S3 mirror
//! or a DuckLake, a key held in a secret. A raw local DuckDB or a key file
//! lives in the checkout, and only a pod holding it can query it.
//! [`first_needing_working_copy`] names such a database for a caller about to
//! query it (the world model, which would otherwise report it as empty).
//!
//! Callers of [`servability`]: `invocation_placement` (in
//! `oxy-api-custom-apps`), which refuses or replays a custom-app function call
//! only when nothing is compiled.

use oxy::config::model::{Database, DatabaseType, DuckDBOptions, SnowflakeAuthType};
use uuid::Uuid;

/// True when querying `db` needs no workspace working copy on the serving node.
/// Exhaustive over `DatabaseType` (no wildcard) so a new database kind forces an
/// explicit serve-safety decision here rather than defaulting silently.
fn database_is_serve_safe(db: &Database) -> bool {
    match &db.database_type {
        // Remote / managed connectors: nothing lives on the local working copy.
        DatabaseType::Postgres(_)
        | DatabaseType::Redshift(_)
        | DatabaseType::Mysql(_)
        | DatabaseType::ClickHouse(_)
        | DatabaseType::DOMO(_)
        | DatabaseType::MotherDuck(_)
        | DatabaseType::Airhouse(_)
        | DatabaseType::AirhouseManaged(_)
        // Per-org OLTP: a remote managed Postgres, resolved from `oltp_tenants`.
        // Nothing about it lives on the working copy.
        | DatabaseType::PostgresManaged(_) => true,
        // DuckDB: serve-safe with a compiler-injected S3 mirror, or a natively
        // S3-backed DuckLake. A raw Local/File DuckDB without a mirror needs the
        // working tree's data files.
        DatabaseType::DuckDB(d) => {
            d.s3_mirror.is_some() || matches!(d.options, DuckDBOptions::DuckLake(_))
        }
        // BigQuery: `key_json_var` holds the key itself, so it is serve-safe
        // whatever else is set (it wins at runtime). `key_path` and
        // `key_path_var` both name a key file, which only the checkout has;
        // ambient credentials are serve-safe.
        DatabaseType::Bigquery(b) => {
            b.key_json_var.is_some() || (b.key_path.is_none() && b.key_path_var.is_none())
        }
        // Snowflake: a PrivateKey path is a local file; password, browser auth
        // and `private_key_var` (the key held in the secret store) are
        // serve-safe.
        DatabaseType::Snowflake(s) => !matches!(s.auth_type, SnowflakeAuthType::PrivateKey { .. }),
    }
}

/// The first database in scope that only a pod holding the working copy can
/// query, by name — `None` when every one in scope is reachable from anywhere.
///
/// `datasources` are the `datasource:` names a caller is about to query. A
/// `None` among them is a view that names no database: it may resolve to any,
/// so it puts every configured one in scope. A name `databases` does not list
/// is not this function's to judge; the query says so itself.
pub fn first_needing_working_copy<'a>(
    databases: &[Database],
    datasources: impl IntoIterator<Item = Option<&'a str>>,
) -> Option<String> {
    let mut every = false;
    let mut named = std::collections::HashSet::new();
    for datasource in datasources {
        match datasource {
            Some(name) => {
                named.insert(name);
            }
            None => every = true,
        }
    }
    databases
        .iter()
        .filter(|db| every || named.contains(db.name.as_str()))
        .find(|db| !database_is_serve_safe(db))
        .map(|db| db.name.clone())
}

/// What a pod with no working copy can do with a workspace.
///
/// There is deliberately no variant for "a database in the config is a file
/// in the working copy". A compiled workspace is served by any pod whatever
/// its databases hold; a query against one this pod cannot open fails by
/// itself, naming the database. Answering for the whole workspace from one
/// database is what INC-174 and INC-175 were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Servability {
    /// A compiled revision this request can pin: any pod serves it.
    Anywhere,
    /// Nothing promoted, or nothing pinnable just now: the only `config.yml`
    /// is the working copy's. A compile changes that.
    NotCompiled,
}

/// Whether a pod with no working copy can serve `workspace_id`.
///
/// Reads the revision a request would be pinned to
/// (`compiled_reader::resolve_request_revision`, last-known-good walk and the
/// "its config deserialises" check included), so it judges the revision the
/// caller is about to build from.
pub async fn servability(workspace_id: Uuid) -> Servability {
    use crate::compiled_reader;
    match compiled_reader::resolve_request_revision(workspace_id, None).await {
        Some(_) => Servability::Anywhere,
        None => Servability::NotCompiled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::config::model::DuckDB;

    fn database(entry: serde_json::Value) -> Database {
        serde_json::from_value(entry).expect("a database entry")
    }

    /// A remote Postgres, and a DuckDB whose data sits in the working copy.
    fn remote_and_local() -> Vec<Database> {
        vec![
            database(serde_json::json!({
                "name": "pg", "type": "postgres", "host": "db", "database": "d"
            })),
            database(serde_json::json!({ "name": "local", "type": "duckdb", "dataset": ".db/" })),
        ]
    }

    #[test]
    fn a_file_backed_database_about_to_be_queried_is_named() {
        assert_eq!(
            first_needing_working_copy(&remote_and_local(), [Some("local")]),
            Some("local".to_string())
        );
    }

    #[test]
    fn one_nobody_is_about_to_query_refuses_nothing() {
        let databases = remote_and_local();
        assert_eq!(first_needing_working_copy(&databases, [Some("pg")]), None);
        assert_eq!(
            first_needing_working_copy(&databases, Vec::<Option<&str>>::new()),
            None
        );
        // A name the config does not list is the query's own error to raise.
        assert_eq!(first_needing_working_copy(&databases, [Some("typo")]), None);
    }

    /// The refusal does not depend on how a connector happens to fail: a key
    /// file is a file in the checkout as much as a DuckDB dataset is.
    /// `key_path_var` is one too: the secret holds a path, not the key.
    #[test]
    fn a_key_file_database_is_named_and_a_secret_backed_one_is_not() {
        let databases = vec![
            database(serde_json::json!({
                "name": "bq_file", "type": "bigquery", "key_path": "key.json", "dataset": "d"
            })),
            database(serde_json::json!({
                "name": "bq_path_var", "type": "bigquery", "key_path_var": "BQ_KEY", "dataset": "d"
            })),
            database(serde_json::json!({
                "name": "bq_secret", "type": "bigquery", "key_json_var": "BQ_KEY_JSON", "dataset": "d"
            })),
        ];
        assert_eq!(
            first_needing_working_copy(&databases, [Some("bq_file")]),
            Some("bq_file".to_string())
        );
        assert_eq!(
            first_needing_working_copy(&databases, [Some("bq_path_var")]),
            Some("bq_path_var".to_string())
        );
        assert_eq!(
            first_needing_working_copy(&databases, [Some("bq_secret")]),
            None
        );
    }

    #[test]
    fn a_view_naming_no_database_puts_every_one_in_scope() {
        let databases = remote_and_local();
        assert_eq!(
            first_needing_working_copy(&databases, [None]),
            Some("local".to_string())
        );
        assert_eq!(
            first_needing_working_copy(&databases, [Some("pg"), None]),
            Some("local".to_string())
        );
    }

    #[test]
    fn local_duckdb_without_mirror_is_not_serve_safe() {
        let db = Database {
            name: "local".to_string(),
            database_type: DatabaseType::DuckDB(DuckDB {
                options: DuckDBOptions::Local {
                    file_search_path: ".db/".to_string(),
                },
                s3_mirror: None,
            }),
        };
        assert!(
            !database_is_serve_safe(&db),
            "a raw local DuckDB without an S3 mirror needs the working copy"
        );
    }
}
