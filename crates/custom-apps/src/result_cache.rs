//! Short-TTL, project-scoped result cache for the custom-app data API.
//!
//! Stores the serialized JSON response body keyed by
//! `(project_id, namespace, database, sql)` so repeat loads of the same query
//! skip the warehouse. `project_id` in the key is the multi-tenant isolation
//! boundary — results never cross tenants. A `?refresh` request bypasses it.

use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lru::LruCache;
use parking_lot::Mutex;
use uuid::Uuid;

const MAX_ENTRIES: usize = 2048;
/// Default freshness window. Short — custom-app dashboards want recent data;
/// this only collapses bursts (re-renders, multiple widgets, fast reloads).
const DEFAULT_TTL: Duration = Duration::from_secs(30);

type Key = (Uuid, &'static str, String, String);
type Cache = Mutex<LruCache<Key, (Instant, Arc<Vec<u8>>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(MAX_ENTRIES).unwrap())))
}

fn key(project_id: Uuid, ns: &'static str, db: &str, sql: &str) -> Key {
    (project_id, ns, db.to_string(), sql.to_string())
}

/// Cached body if present and not past `DEFAULT_TTL`; else `None`.
pub fn get(project_id: Uuid, ns: &'static str, db: &str, sql: &str) -> Option<Arc<Vec<u8>>> {
    let mut c = cache().lock();
    let k = key(project_id, ns, db, sql);
    match c.get(&k) {
        Some((at, body)) if at.elapsed() < DEFAULT_TTL => Some(body.clone()),
        Some(_) => {
            c.pop(&k);
            None
        }
        None => None,
    }
}

pub fn put(project_id: Uuid, ns: &'static str, db: &str, sql: &str, body: Arc<Vec<u8>>) {
    cache()
        .lock()
        .put(key(project_id, ns, db, sql), (Instant::now(), body));
}

/// Test-only: insert with an explicit TTL by backdating the timestamp.
#[cfg(test)]
pub fn put_with_ttl(
    project_id: Uuid,
    ns: &'static str,
    db: &str,
    sql: &str,
    body: Arc<Vec<u8>>,
    ttl: Duration,
) {
    let at = Instant::now() - (DEFAULT_TTL - ttl);
    cache().lock().put(key(project_id, ns, db, sql), (at, body));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn put_then_get_hits_within_ttl() {
        let p = Uuid::new_v4();
        put(
            p,
            "query",
            "db1",
            "select 1",
            std::sync::Arc::new(b"BODY".to_vec()),
        );
        let got = get(p, "query", "db1", "select 1");
        assert_eq!(got.as_deref().map(|v| v.as_slice()), Some(&b"BODY"[..]));
    }

    #[test]
    fn miss_on_different_project_or_sql_or_db() {
        let p = Uuid::new_v4();
        put(
            p,
            "query",
            "db1",
            "select 1",
            std::sync::Arc::new(b"X".to_vec()),
        );
        assert!(
            get(Uuid::new_v4(), "query", "db1", "select 1").is_none(),
            "different project must miss"
        );
        assert!(
            get(p, "query", "db1", "select 2").is_none(),
            "different sql must miss"
        );
        assert!(
            get(p, "query", "db2", "select 1").is_none(),
            "different db must miss"
        );
    }

    #[test]
    fn expires_after_ttl() {
        let p = Uuid::new_v4();
        put_with_ttl(
            p,
            "query",
            "db1",
            "q",
            std::sync::Arc::new(b"Y".to_vec()),
            Duration::from_millis(0),
        );
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            get(p, "query", "db1", "q").is_none(),
            "expired entry must miss"
        );
    }

    #[test]
    fn namespace_separates_query_and_semantic() {
        let p = Uuid::new_v4();
        put(p, "query", "", "SPEC", std::sync::Arc::new(b"Q".to_vec()));
        put(
            p,
            "semantic",
            "",
            "SPEC",
            std::sync::Arc::new(b"S".to_vec()),
        );
        assert_eq!(
            get(p, "query", "", "SPEC").as_deref().map(|v| v.as_slice()),
            Some(&b"Q"[..])
        );
        assert_eq!(
            get(p, "semantic", "", "SPEC")
                .as_deref()
                .map(|v| v.as_slice()),
            Some(&b"S"[..])
        );

        // The typed vs `untyped` split relies on this too: `/query` writes under
        // "query", `?untyped` under "query-untyped", so the same SQL on the same
        // project can't cross-read the other's cell types (see `oxy-api-custom-apps`' `data_plane/query.rs`).
        put(
            p,
            "query-untyped",
            "",
            "SPEC",
            std::sync::Arc::new(b"U".to_vec()),
        );
        assert_eq!(
            get(p, "query", "", "SPEC").as_deref().map(|v| v.as_slice()),
            Some(&b"Q"[..]),
            "an untyped entry must not clobber the typed one"
        );
        assert_eq!(
            get(p, "query-untyped", "", "SPEC")
                .as_deref()
                .map(|v| v.as_slice()),
            Some(&b"U"[..])
        );
    }
}
