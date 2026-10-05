//! Per-workspace "serve-safe" predicate for the conditional `/analytics` un-pin.
//!
//! A workspace is *serve-safe* when every configured database can be queried
//! WITHOUT the workspace working copy — so the stateless serve fleet can run the
//! analytics agent itself instead of reverse-proxying to the ide. The check is a
//! CONSERVATIVE allowlist: anything we can't positively classify as FS-free (a
//! raw local DuckDB, a local key file) is treated as NOT serve-safe, so the
//! worst case is an unnecessary proxy to the ide (always correct) — never a run
//! mis-served on a node that lacks the file.
//!
//! Gated by `OXY_ANALYTICS_FLEET_UNPIN` (default off) at the middleware call
//! site; this module is pure classification + the compile-boundary read.

use std::num::NonZeroUsize;
use std::sync::{Mutex, OnceLock};

use lru::LruCache;
use oxy::config::model::{Config, Database, DatabaseType, DuckDBOptions, SnowflakeAuthType};
use uuid::Uuid;

/// Whether the conditional `/analytics` fleet un-pin is enabled. Default OFF —
/// when unset the serve fleet proxies every `/analytics` request to the ide,
/// exactly as before, so enabling/rolling back is a single env flag.
pub fn analytics_fleet_unpin_enabled() -> bool {
    std::env::var("OXY_ANALYTICS_FLEET_UNPIN")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Extract the workspace id from an `/api/{workspace_id}/analytics/...` path, or
/// `None` for any other path. Scopes the un-pin to analytics routes only —
/// every other IdeOnly route (and the similarly-named `/analytics-workflows`)
/// keeps proxying.
pub fn analytics_workspace_id(path: &str) -> Option<Uuid> {
    let rest = path.strip_prefix("/api/")?;
    let (ws, tail) = rest.split_once('/')?;
    if tail != "analytics" && !tail.starts_with("analytics/") {
        return None;
    }
    Uuid::parse_str(ws).ok()
}

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
                    "serve_safety: compiled config lookup failed; servability unknown"
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

/// True when the workspace's promoted compiled config has only serve-safe
/// databases — the fleet can run its analytics agent locally instead of proxying
/// to the ide. Reads the compile boundary; any miss / undeserialisable config /
/// DB error resolves to `false` (proxy — the always-correct default).
pub async fn workspace_is_serve_safe(workspace_id: Uuid) -> bool {
    match crate::server::api::compiled_reader::resolve_workspace_config(workspace_id, None).await {
        Ok(Some(value)) => match serde_json::from_value::<Config>(value) {
            Ok(config) => config_is_serve_safe(&config),
            Err(e) => {
                tracing::warn!(
                    workspace_id = %workspace_id, error = ?e,
                    "serve_safety: compiled config did not deserialise; not serve-safe (proxy)"
                );
                false
            }
        },
        // Not promoted / not compiled / transient DB error — can't prove
        // serve-safe, so proxy.
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id, error = ?e,
                "serve_safety: compiled config lookup failed; not serve-safe (proxy)"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::config::model::DuckDB;

    #[test]
    fn analytics_workspace_id_matches_only_analytics_paths() {
        let ws = "11111111-1111-1111-1111-111111111111";
        let parsed = Uuid::parse_str(ws).unwrap();
        // Analytics routes (bare + nested) yield the workspace id.
        assert_eq!(
            analytics_workspace_id(&format!("/api/{ws}/analytics/runs")),
            Some(parsed)
        );
        assert_eq!(
            analytics_workspace_id(&format!("/api/{ws}/analytics")),
            Some(parsed)
        );
        assert_eq!(
            analytics_workspace_id(&format!("/api/{ws}/analytics/runs/abc/answer")),
            Some(parsed)
        );
        // Non-analytics routes → None (they keep proxying), incl. the
        // similarly-named /analytics-workflows and a non-UUID segment.
        assert_eq!(analytics_workspace_id(&format!("/api/{ws}/threads")), None);
        assert_eq!(
            analytics_workspace_id(&format!("/api/{ws}/analytics-workflows/x")),
            None
        );
        assert_eq!(
            analytics_workspace_id("/api/not-a-uuid/analytics/runs"),
            None
        );
        assert_eq!(analytics_workspace_id("/healthz"), None);
    }

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
