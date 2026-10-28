//! What a pod with no working copy can do with a workspace.
//!
//! A database is *reachable from anywhere* when it can be queried without the
//! workspace working copy: a remote warehouse, a DuckDB with a compiler-made S3
//! mirror or a DuckLake, a key held in a secret. A raw local DuckDB or a key
//! file lives in the checkout, and only the pod holding it can query it. The
//! check is a conservative allowlist: anything not positively classified as
//! file-free is treated as needing the working copy.
//!
//! Two callers: `invocation_placement` (in `oxy-api-custom-apps`), which
//! replays a custom-app function to the Factory when a replica cannot serve its
//! workspace, and the world model's source check, which names the database a
//! query would need the checkout for. It used to also decide, behind
//! `OXY_ANALYTICS_FLEET_UNPIN`, whether a replica could run `/analytics` itself;
//! those routes are now served by any pod by declaration (factory-retirement.md
//! phase 3d).

use std::num::NonZeroUsize;
use std::sync::{Mutex, OnceLock};

use lru::LruCache;
use oxy::config::model::{Config, Database, DatabaseType, DuckDBOptions, SnowflakeAuthType};
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
        // BigQuery: a local key file pins to the FS; a key_path_var (secret) or
        // ambient credentials are serve-safe.
        DatabaseType::Bigquery(b) => b.key_path.is_none(),
        // Snowflake: a PrivateKey path is a local file; password / var / browser
        // auth are serve-safe.
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
pub(crate) fn first_needing_working_copy<'a>(
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

/// True when every database in `config` is serve-safe. An agent with no
/// databases has nothing to execute against and is trivially serve-safe.
fn config_is_serve_safe(config: &Config) -> bool {
    config.databases.iter().all(database_is_serve_safe)
}

/// What a pod with no working copy can do with a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Servability {
    /// Promoted, and every database is reachable without the working copy.
    Anywhere,
    /// Promoted, but a database lives in the working copy — a local DuckDB with
    /// no S3 mirror, a key file. Waiting does not change that; only the pod
    /// holding the checkout can query it.
    NeedsWorkingCopy,
    /// Nothing promoted: the only `config.yml` is the working copy's. A compile
    /// changes that.
    NotCompiled,
    /// Promoted, but its config could not be read just now — a database blip.
    /// Not a property of the workspace, so neither memoised nor fixed by a
    /// compile; the next call asks again.
    Unknown,
}

/// One live revision per invoked workspace plus a promote window's straggler,
/// at ~100 bytes an entry and one config-row read a miss: 1024 overshoots cheaply.
const REVISION_MEMO_CAP: usize = 1024;

/// Serve-safety of a revision's config. A revision is immutable, so the answer
/// holds for its lifetime and the hot path is one map read. An LRU, because
/// every promote mints a new revision id and a superseded one is never asked
/// about again.
fn revision_safety_memo() -> &'static Mutex<LruCache<Uuid, bool>> {
    static MEMO: OnceLock<Mutex<LruCache<Uuid, bool>>> = OnceLock::new();
    MEMO.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(REVISION_MEMO_CAP).expect("REVISION_MEMO_CAP is non-zero"),
        ))
    })
}

/// [`workspace_is_serve_safe`] split into its refusals, for a caller that
/// answers them differently: an uncompiled workspace is worth a compile and a
/// retry, an unreadable config a retry alone, and one whose database is a file
/// in the working copy neither.
///
/// Reads the revision a request would be pinned to
/// (`compiled_reader::resolve_request_revision`, last-known-good walk included)
/// rather than the bare promoted pointer, so it judges the config the caller
/// is about to build from. A lookup that fails is `Unknown` and is not
/// memoised: it cannot be proven safe, and a compile would not fix a database
/// blip.
pub async fn servability(workspace_id: Uuid) -> Servability {
    use crate::server::api::compiled_reader;
    let Some(revision_id) = compiled_reader::resolve_request_revision(workspace_id, None).await
    else {
        return Servability::NotCompiled;
    };
    let known = revision_safety_memo()
        .lock()
        .ok()
        .and_then(|mut memo| memo.get(&revision_id).copied());
    let safe = match known {
        Some(safe) => safe,
        None => match compiled_reader::resolve_workspace_config_at(revision_id).await {
            Ok(value) => {
                let safe = compiled_config_is_serve_safe(value);
                if let Ok(mut memo) = revision_safety_memo().lock() {
                    memo.put(revision_id, safe);
                }
                safe
            }
            Err(e) => {
                tracing::warn!(
                    workspace_id = %workspace_id, %revision_id, error = ?e,
                    "servability: compiled config lookup failed; servability unknown"
                );
                return Servability::Unknown;
            }
        },
    };
    if safe {
        Servability::Anywhere
    } else {
        Servability::NeedsWorkingCopy
    }
}

/// A revision with no config row compiled a workspace that has no `config.yml`:
/// no databases, so nothing that could live in the working copy.
fn compiled_config_is_serve_safe(value: Option<serde_json::Value>) -> bool {
    match value.map(serde_json::from_value::<Config>) {
        None => true,
        Some(Ok(config)) => config_is_serve_safe(&config),
        Some(Err(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::config::model::DuckDB;

    #[test]
    fn a_revision_with_no_config_row_has_no_database_to_be_unsafe() {
        assert!(compiled_config_is_serve_safe(None));
    }

    #[test]
    fn a_compiled_config_is_judged_by_its_databases() {
        // The shape a promoted revision's config row merges back into.
        let remote = serde_json::json!({
            "defaults": null,
            "builder_agent": null,
            "models": [],
            "databases": [{ "name": "pg", "type": "postgres", "host": "db", "database": "d" }],
        });
        assert!(compiled_config_is_serve_safe(Some(remote)));
        let local = serde_json::json!({
            "defaults": null,
            "builder_agent": null,
            "models": [],
            "databases": [{ "name": "local", "type": "duckdb", "dataset": ".db/" }],
        });
        assert!(
            !compiled_config_is_serve_safe(Some(local)),
            "a local DuckDB with no mirror lives in the working copy"
        );
    }

    #[test]
    fn the_revision_memo_forgets_the_least_recent_past_its_cap() {
        // A revision id per promote, for the life of the process: unbounded,
        // this grew one entry per revision ever asked about.
        let mut memo = revision_safety_memo().lock().unwrap();
        let first = Uuid::new_v4();
        memo.put(first, true);
        for _ in 0..REVISION_MEMO_CAP {
            memo.put(Uuid::new_v4(), true);
        }
        assert_eq!(memo.len(), REVISION_MEMO_CAP);
        assert!(memo.peek(&first).is_none(), "the oldest verdict is evicted");
    }

    #[test]
    fn a_config_that_does_not_deserialise_is_not_proven_safe() {
        let broken = serde_json::json!({ "databases": "not-an-array" });
        assert!(!compiled_config_is_serve_safe(Some(broken)));
    }

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
    #[test]
    fn a_key_file_database_is_named_and_a_secret_backed_one_is_not() {
        let databases = vec![
            database(serde_json::json!({
                "name": "bq_file", "type": "bigquery", "key_path": "key.json", "dataset": "d"
            })),
            database(serde_json::json!({
                "name": "bq_secret", "type": "bigquery", "key_path_var": "BQ_KEY", "dataset": "d"
            })),
        ];
        assert_eq!(
            first_needing_working_copy(&databases, [Some("bq_file")]),
            Some("bq_file".to_string())
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
