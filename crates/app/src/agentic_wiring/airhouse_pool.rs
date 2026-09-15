//! Process-wide reuse pool for Airhouse `DatabaseConnector`s (analytics /
//! Data-App / SQL-IDE path).
//!
//! # Why
//!
//! The analytics path resolves its connector through a per-request
//! [`OxyProjectContext`], so every warehouse query opened a **fresh** pgwire
//! connection — and Airhouse backs each connection with its own server-side
//! DuckLake session (in-memory DuckDB + ducklake/h3/spatial + a catalog
//! `ATTACH`). Under load one tenant spiked to ~32 concurrent sessions on a
//! single Airhouse DP and OOM-killed it (4Gi pod); the DP's 4-slot query
//! governor then queued unrelated work (an airway `ALTER TABLE`) until it hit
//! the 30s execution-slot timeout. (Prod incident 2026-06-23.)
//!
//! # What
//!
//! Cache a **small pool of up to N** live `AirhouseConnector`s per **logical
//! identity** (`mgd:<workspace>:<subject|system>:<role>` for `airhouse_managed`,
//! `static:<workspace>:<db>` for static airhouse) and round-robin across them.
//! This caps an identity's concurrent DP sessions at N (default 3) instead of
//! unbounded — collapsing the 32-session spike — while still allowing N-way
//! query concurrency so a slow analytics query doesn't head-of-line-block the
//! rest. (Cameras ingest is pooled separately at one connection — its writes
//! are light and serialize fine; analytics queries are heavier and benefit from
//! the small pool.)
//!
//! Slots fill **lazily** as round-robin cycles through them, so a low-load
//! identity uses fewer than N; a dead slot (DP restart, detected via the
//! non-blocking [`AirhouseConnector::is_live`]) is rebuilt in place on next use.
//! Idle identities are swept (default 5 min) so a quiet tenant returns its DP
//! sessions, and a cap on tracked identities bounds total live sessions.
//!
//! Tunables: `OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY` (N, default 3),
//! `OXY_AIRHOUSE_POOL_IDLE_SECS` (default 300),
//! `OXY_AIRHOUSE_POOL_MAX_IDENTITIES` (default 16),
//! `OXY_AIRHOUSE_POOL_MAX_APP_IDENTITIES` (default 16 — `ctx.airhouse`'s
//! `app:<workspace>:<slug>` identities, budgeted apart so app writers and the
//! analytics path never LRU-evict each other — so the process-wide ceiling is
//! the two caps together, (16 + 16) × N = 96 connections at the defaults),
//! `OXY_AIRHOUSE_POOL_DISABLED=1` (bypass → pre-pool per-request behaviour).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentic_connector::DatabaseConnector;
use airhouse::AirhouseConnector;
use oxy_shared::errors::OxyError;
use tokio::sync::{Mutex, RwLock};

const DEFAULT_CONNS_PER_IDENTITY: usize = 3;
const DEFAULT_IDLE_SECS: u64 = 300;
const DEFAULT_MAX_IDENTITIES: usize = 16;
const DEFAULT_MAX_APP_IDENTITIES: usize = 16;
/// Identities `ctx.airhouse` builds (`agentic_wiring::app_airhouse`).
const APP_KEY_PREFIX: &str = "app:";
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Up to N reused connections for one logical identity. Each slot is built
/// lazily (round-robin fills them), so a low-load identity holds fewer than N.
struct KeyPool {
    slots: Vec<Mutex<Option<Arc<AirhouseConnector>>>>,
    next: AtomicUsize,
    /// Unix seconds of the last checkout; drives idle + LRU eviction.
    last_used: AtomicU64,
}

impl KeyPool {
    fn new(n: usize, now: u64) -> Self {
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            slots.push(Mutex::new(None));
        }
        Self {
            slots,
            next: AtomicUsize::new(0),
            last_used: AtomicU64::new(now),
        }
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<KeyPool>>> {
    static POOL: OnceLock<RwLock<HashMap<String, Arc<KeyPool>>>> = OnceLock::new();
    POOL.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Reuse (or lazily build) an Airhouse connector for `key`, round-robined over
/// up to N per-identity connections.
///
/// `build` runs **only** when the chosen slot is empty or its connection died —
/// on a warm hit no credential is minted and no connection is opened. It
/// returns a concrete `Arc<AirhouseConnector>` so the pool can call `is_live()`;
/// callers get back an `Arc<dyn DatabaseConnector>`.
pub async fn get_or_build<F, Fut>(
    key: String,
    build: F,
) -> Result<Arc<dyn DatabaseConnector>, OxyError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Arc<AirhouseConnector>, OxyError>>,
{
    if pooling_disabled() {
        return build().await.map(|c| c as Arc<dyn DatabaseConnector>);
    }

    let now = now_secs();
    let pool = key_pool(&key, now).await;
    pool.last_used.store(now, Ordering::Relaxed);
    ensure_sweeper();

    // Round-robin a slot. Holding the per-slot lock across a build only blocks
    // checkouts that land on the SAME slot; other slots / identities proceed,
    // so concurrent demand grows the pool toward N distinct connections.
    let i = pool.next.fetch_add(1, Ordering::Relaxed) % pool.slots.len();
    let mut slot = pool.slots[i].lock().await;
    if let Some(conn) = slot.as_ref() {
        if conn.is_live() {
            tracing::debug!(pool_key = %key, slot = i, "airhouse pool: reuse");
            return Ok(conn.clone() as Arc<dyn DatabaseConnector>);
        }
        tracing::info!(pool_key = %key, slot = i, "airhouse pool: slot dead, rebuilding");
    }
    let conn = build().await?;
    *slot = Some(Arc::clone(&conn));
    tracing::info!(pool_key = %key, slot = i, "airhouse pool: build");
    Ok(conn as Arc<dyn DatabaseConnector>)
}

/// Get the identity's pool, creating it (and pruning idle/over-cap identities)
/// on first use.
async fn key_pool(key: &str, now: u64) -> Arc<KeyPool> {
    if let Some(p) = registry().read().await.get(key).cloned() {
        return p;
    }
    let mut guard = registry().write().await;
    if let Some(p) = guard.get(key).cloned() {
        return p;
    }
    evict(&mut guard, now);
    let pool = Arc::new(KeyPool::new(conns_per_identity(), now));
    guard.insert(key.to_string(), Arc::clone(&pool));
    tracing::info!(pool_key = %key, identities = guard.len(), "airhouse pool: new identity");
    pool
}

/// Drop idle and over-cap identities from `map`. Removing an identity only
/// stops *reuse*; any in-flight request holding a connection keeps it until it
/// finishes, then the session closes.
fn evict(map: &mut HashMap<String, Arc<KeyPool>>, now: u64) {
    let snapshot: Vec<(String, u64)> = map
        .iter()
        .map(|(k, p)| (k.clone(), p.last_used.load(Ordering::Relaxed)))
        .collect();
    let budgets = Budgets {
        idle_secs: idle_secs(),
        max_identities: max_identities(),
        max_app_identities: max_app_identities(),
    };
    for key in keys_to_evict_by_budget(&snapshot, now, budgets) {
        map.remove(&key);
    }
}

#[derive(Clone, Copy)]
struct Budgets {
    idle_secs: u64,
    max_identities: usize,
    max_app_identities: usize,
}

/// [`keys_to_evict`] run separately over app-write identities and everything
/// else, so a workspace with several `ctx.airhouse` apps cannot push the
/// analytics path's sessions out of the pool, and analytics traffic cannot
/// evict an app mid-burst — each eviction costs a mint and a fresh DuckLake
/// session, the load this pool exists to cap.
fn keys_to_evict_by_budget(entries: &[(String, u64)], now: u64, b: Budgets) -> Vec<String> {
    let (apps, others): (Vec<_>, Vec<_>) = entries
        .iter()
        .cloned()
        .partition(|(k, _)| k.starts_with(APP_KEY_PREFIX));
    let mut out = keys_to_evict(&others, now, b.idle_secs, b.max_identities);
    out.extend(keys_to_evict(&apps, now, b.idle_secs, b.max_app_identities));
    out
}

/// Pure eviction policy: every identity idle beyond `idle_secs`, plus the
/// least-recently-used identities past `max` (so a new one can fit). Separated
/// from the map so it's unit-testable without real connectors.
fn keys_to_evict(entries: &[(String, u64)], now: u64, idle_secs: u64, max: usize) -> Vec<String> {
    let mut evict: Vec<String> = entries
        .iter()
        .filter(|(_, last)| now.saturating_sub(*last) >= idle_secs)
        .map(|(k, _)| k.clone())
        .collect();

    let survivors: Vec<&(String, u64)> =
        entries.iter().filter(|(k, _)| !evict.contains(k)).collect();
    if max > 0 && survivors.len() >= max {
        let mut by_age = survivors.clone();
        by_age.sort_by_key(|(_, last)| *last); // oldest first
        let overflow = survivors.len() - (max - 1);
        for (k, _) in by_age.into_iter().take(overflow) {
            evict.push(k.clone());
        }
    }
    evict
}

/// Spawn the idle sweeper exactly once (no-op without a Tokio runtime, e.g. in
/// unit tests). Idle eviction otherwise only happens on new-identity insert, so
/// a pool that goes quiet would pin DP sessions until the next build.
fn ensure_sweeper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    STARTED.get_or_init(|| {
        tokio::spawn(async {
            let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
            ticker.tick().await; // skip the immediate tick
            loop {
                ticker.tick().await;
                let now = now_secs();
                let mut guard = registry().write().await;
                let before = guard.len();
                evict(&mut guard, now);
                let removed = before - guard.len();
                if removed > 0 {
                    tracing::info!(
                        removed,
                        identities = guard.len(),
                        "airhouse pool: idle sweep"
                    );
                }
            }
        });
    });
}

// ── env / time helpers ──────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn pooling_disabled() -> bool {
    std::env::var("OXY_AIRHOUSE_POOL_DISABLED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn conns_per_identity() -> usize {
    env_usize(
        "OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY",
        DEFAULT_CONNS_PER_IDENTITY,
    )
}

fn idle_secs() -> u64 {
    env_usize("OXY_AIRHOUSE_POOL_IDLE_SECS", DEFAULT_IDLE_SECS as usize) as u64
}

fn max_identities() -> usize {
    env_usize("OXY_AIRHOUSE_POOL_MAX_IDENTITIES", DEFAULT_MAX_IDENTITIES)
}

fn max_app_identities() -> usize {
    env_usize(
        "OXY_AIRHOUSE_POOL_MAX_APP_IDENTITIES",
        DEFAULT_MAX_APP_IDENTITIES,
    )
}

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::{Budgets, keys_to_evict, keys_to_evict_by_budget};

    fn e(k: &str, last: u64) -> (String, u64) {
        (k.to_string(), last)
    }

    #[test]
    fn app_identities_and_the_analytics_path_do_not_evict_each_other() {
        let entries = [
            e("mgd:w:u1:Reader", 990),
            e("mgd:w:u2:Reader", 995),
            e("app:w:store-ops", 900),
            e("app:w:bookkeeping", 910),
        ];
        let budgets = Budgets {
            idle_secs: 600,
            max_identities: 3,
            max_app_identities: 3,
        };
        // Four identities against one shared cap of 3 would evict the oldest two
        // — both apps. Budgeted apart, each side is under its own cap.
        assert!(keys_to_evict(&entries, 1000, 600, 3).len() == 2);
        assert!(keys_to_evict_by_budget(&entries, 1000, budgets).is_empty());
    }

    #[test]
    fn evicts_only_idle_entries() {
        let entries = [e("a", 100), e("b", 950), e("c", 970)];
        // now=1000, idle=60 → "a" (idle 900) evicted; b (idle 50) and c (idle 30) kept.
        let mut out = keys_to_evict(&entries, 1000, 60, 100);
        out.sort();
        assert_eq!(out, vec!["a".to_string()]);
    }

    #[test]
    fn idle_secs_is_inclusive_boundary() {
        let entries = [e("a", 940)];
        assert_eq!(
            keys_to_evict(&entries, 1000, 60, 100),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn cap_evicts_lru_to_make_room() {
        // All fresh, cap=3 → drop oldest so one new identity fits.
        let entries = [e("a", 10), e("b", 20), e("c", 30)];
        assert_eq!(
            keys_to_evict(&entries, 30, 100_000, 3),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn cap_zero_disables_cap_eviction() {
        let entries = [e("a", 10), e("b", 20)];
        assert!(keys_to_evict(&entries, 30, 100_000, 0).is_empty());
    }

    #[test]
    fn under_cap_and_fresh_evicts_nothing() {
        let entries = [e("a", 10), e("b", 20)];
        assert!(keys_to_evict(&entries, 30, 100_000, 10).is_empty());
    }
}
