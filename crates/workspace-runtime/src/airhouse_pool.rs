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
//! identity** (`mgd:<workspace>:<user|system>:<role>` for `airhouse_managed`,
//! `static:<workspace>:<db>` for static airhouse). The managed key is **not**
//! per requester — see `project_ctx::airhouse_pool_key` for why, and for the
//! bound it puts on how many identities one workspace can have.
//! This caps an identity's concurrent DP sessions at N (default 3) instead of
//! unbounded — collapsing the 32-session spike — while still allowing N-way
//! query concurrency so a slow analytics query doesn't head-of-line-block the
//! rest. (Cameras ingest is pooled separately at one connection — its writes
//! are light and serialize fine; analytics queries are heavier and benefit from
//! the small pool.)
//!
//! A checkout **reuses an idle open connection before opening a new one**
//! (`pick::choose_slot`): a slot is idle when the pool holds the only
//! reference to its connector, so no caller is still using it. Only when every
//! open connection is in use does it open the next empty slot, and only when all
//! N are open and in use does it share one, round-robin. So an identity holds as
//! many connections as its peak concurrent callers since it was last swept, not
//! N. Every open
//! connection holds one of its tenant's connection slots on the DP whether or
//! not it is running a query, so the old strict round-robin — which opened all N
//! even at concurrency 1 — filled the analytics DP's connection cap at under 1%
//! CPU (prod, 2026-10-29). A dead slot (DP restart, detected via the
//! non-blocking [`AirhouseConnector::is_live`]) is rebuilt in place on next use.
//! Idle identities are swept (default 5 min) so a quiet tenant returns its DP
//! sessions, and a cap on tracked identities bounds total live sessions.
//!
//! # Airhouse's two rules, which a naive pool gets wrong
//!
//! Both are enforced here and each has its own module arguing why, because
//! both look like redundant complexity next to the liveness check:
//!
//! 1. **Credentials expire and airhouse closes the socket** ([`expiry`]). A
//!    connection is authenticated with a 900s ephemeral credential; the slot is
//!    retired ~2 min before that deadline rather than waiting to be hung up on
//!    mid-request. [`ceiling::connect_within_budget`]'s callers mint the
//!    credential and pass its `expires_at` to the connector.
//! 2. **`53300` is backpressure, not a failure** ([`backpressure`]). A connect
//!    refused because the data plane is at its cap is retried with a short
//!    backoff instead of surfacing as a failed query.
//!
//! And a third the per-identity caps could not see: **the process-wide ceiling**
//! ([`ceiling`]). Every cap below is per identity or per identity class and
//! nothing can see their product, so they multiply out to far more connections
//! than the shared data plane admits — and each `oxy serve` replica and worker
//! holds its own process-wide registry on top of that.
//!
//! Tunables: `OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY` (N, default 3),
//! `OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS` (process-wide ceiling, default 24),
//! `OXY_AIRHOUSE_POOL_IDLE_SECS` (default 300),
//! `OXY_AIRHOUSE_POOL_MAX_IDENTITIES` (default 48),
//! `OXY_AIRHOUSE_POOL_MAX_APP_IDENTITIES` (default 16 — `ctx.airhouse`'s
//! `app:<workspace>:<slug>` identities, budgeted apart so app writers and the
//! analytics path never LRU-evict each other. These caps bound LRU churn
//! within a class; they do **not** bound the process total —
//! `OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS` is what does, see [`ceiling`]),
//! `OXY_AIRHOUSE_POOL_MAX_PREVIEW_IDENTITIES` (default 8 — a workspace
//! preview's `preview:[tx:]<workspace>:<key>[:<schemas>]` identities,
//! budgeted apart for the same reason, adding 8 × N),
//! `OXY_AIRHOUSE_POOL_DISABLED=1` (bypass → pre-pool per-request behaviour).
//!
//! [`checkout_exclusive`] holds one connection for a caller that sends several
//! statements which must share a session (a preview batch's `BEGIN … COMMIT`,
//! on its `preview:tx:` identities): nobody else is handed that slot until the
//! lease drops, and [`ExclusiveLease::poison`] empties the slot of a
//! connection that may still be inside the transaction.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentic_connector::DatabaseConnector;
use airhouse::AirhouseConnector;
use oxy_shared::errors::OxyError;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use pick::SlotState;

const DEFAULT_CONNS_PER_IDENTITY: usize = 3;
const DEFAULT_IDLE_SECS: u64 = 300;
/// Analytics identities tracked at once, before LRU eviction starts.
///
/// **Raised from 16 once [`ceiling`] bounded the total.** This cap used to be
/// doing two jobs at once and was sized for the second: limit LRU churn, and
/// stand in as the safety valve on total connections. It was never any good at
/// the second — `conns x identities` could reach 120 with no relation to what
/// the data plane actually admits — and being sized for it made it too tight
/// for the first.
///
/// `airhouse_pool_key` yields **6 identities per workspace** (4 roles +
/// role-less + system), so 16 was exceeded by **three active workspaces**.
/// Past the cap, `keys_to_evict` drops the set back to `max - 1` on every
/// new-identity insert, and the next request for an evicted identity pays a
/// fresh mint, connect and `fetch_schema()`. That is a connect storm into
/// Airhouse's admission gate which scales with **tenant count, not query
/// volume** — a load shape that fills the gate without raising CPU, so no
/// CPU-based autoscaler reacts to it.
///
/// 48 holds 8 workspaces without churn. Raising it can no longer raise the
/// connection total, because [`ceiling`] caps that independently — which is
/// what makes this safe to raise at all.
const DEFAULT_MAX_IDENTITIES: usize = 48;
const DEFAULT_MAX_APP_IDENTITIES: usize = 16;
const DEFAULT_MAX_PREVIEW_IDENTITIES: usize = 8;
/// Identities `ctx.airhouse` builds (`crate::app_airhouse`).
const APP_KEY_PREFIX: &str = "app:";
/// Identities a workspace preview builds (`agentic_wiring::preview_airhouse`).
pub const PREVIEW_KEY_PREFIX: &str = "preview:";
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

type Slot = Arc<Mutex<Option<Arc<AirhouseConnector>>>>;
type SlotGuard = OwnedMutexGuard<Option<Arc<AirhouseConnector>>>;

/// Up to N reused connections for one logical identity. Each slot is built
/// lazily, only when every open one is in use, so a low-load identity holds
/// fewer than N.
struct KeyPool {
    slots: Vec<Slot>,
    /// Round-robin cursor, used only once all N are open and in use.
    next: AtomicUsize,
    /// Unix seconds of the last checkout; drives idle + LRU eviction.
    last_used: AtomicU64,
}

impl KeyPool {
    fn new(n: usize, now: u64) -> Self {
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            slots.push(Arc::new(Mutex::new(None)));
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

/// Reuse (or lazily build) an Airhouse connector for `key`, from up to N
/// per-identity connections — an idle open one first.
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
    checkout_exclusive(key, build).await.map(|lease| lease.conn)
}

/// A pooled connection nobody else is handed until this drops.
pub struct ExclusiveLease {
    conn: Arc<dyn DatabaseConnector>,
    /// The slot's lock; `None` when pooling is disabled (the connection is
    /// this caller's alone anyway).
    slot: Option<SlotGuard>,
}

impl ExclusiveLease {
    pub fn connector(&self) -> &Arc<dyn DatabaseConnector> {
        &self.conn
    }

    /// Empty the slot, so the connection is never handed out again: for one
    /// left in a state the next holder must not inherit (a transaction that
    /// could not be rolled back). The next checkout of the slot builds a new
    /// connection; this one closes when its last holder drops it.
    pub fn poison(&mut self) {
        if let Some(slot) = self.slot.as_mut() {
            **slot = None;
        }
    }
}

/// [`get_or_build`], holding the slot until the lease drops: every other
/// checkout that lands on it — exclusive or not — waits. For statements that
/// must share one session (a transaction). Every caller of such a key must
/// check out this way: a plain [`get_or_build`] hands its connection on and
/// forgets it, so its holder could send into someone else's transaction.
/// Keep transactional identities under keys nothing else uses.
pub async fn checkout_exclusive<F, Fut>(key: String, build: F) -> Result<ExclusiveLease, OxyError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Arc<AirhouseConnector>, OxyError>>,
{
    if pooling_disabled() {
        let conn = build().await?;
        return Ok(ExclusiveLease { conn, slot: None });
    }

    let now = now_secs();
    let pool = key_pool(&key, now).await;
    pool.last_used.store(now, Ordering::Relaxed);
    ensure_sweeper();

    // Holding the per-slot lock across a build only blocks checkouts that land
    // on the SAME slot; other slots / identities proceed, so concurrent demand
    // grows the pool toward N distinct connections.
    let (picked, occupied) = try_pick(&pool, now);
    let (i, mut slot) = match picked {
        Some(picked) => picked,
        None => {
            let i = pool.next.fetch_add(1, Ordering::Relaxed) % pool.slots.len();
            (i, Arc::clone(&pool.slots[i]).lock_owned().await)
        }
    };
    // Decided before the slot is needed mutably below; `Option<&'static str>`
    // is `Copy`, so this holds no borrow.
    let retire = slot.as_ref().and_then(|conn| reuse_block(conn, now));
    if retire.is_none() {
        if let Some(conn) = slot.as_ref() {
            tracing::debug!(pool_key = %key, slot = i, "airhouse pool: reuse");
            let conn = conn.clone() as Arc<dyn DatabaseConnector>;
            return Ok(ExclusiveLease {
                conn,
                slot: Some(slot),
            });
        }
    } else if let Some(reason) = retire {
        tracing::info!(
            pool_key = %key,
            slot = i,
            reason,
            "airhouse pool: slot retired, rebuilding"
        );
        // Drop the retired connection *before* opening its replacement, so its
        // slot in the process budget ([`ceiling`]) is released first. Leaving
        // it in place would make one logical slot need two permits for the
        // length of a rebuild, and credential expiry is **correlated** —
        // identities minted together lapse together — so a wave of retirements
        // would otherwise each wait on a permit the others are holding and
        // then fail at the ceiling.
        //
        // This drops the pool's own reference only: a caller still holding the
        // connector keeps it, and its permit, until the request finishes, which
        // is exactly the behaviour `ceiling` documents.
        drop(slot.take());
    }
    let conn = build().await?;
    *slot = Some(Arc::clone(&conn));
    // `occupied` (open or locked by another checkout) at the time of the scan:
    // a build should only happen once every occupied slot is in use.
    tracing::info!(
        pool_key = %key,
        slot = i,
        occupied,
        width = pool.slots.len(),
        "airhouse pool: build"
    );
    Ok(ExclusiveLease {
        conn,
        slot: Some(slot),
    })
}

/// Lock, without waiting, the slot [`pick::choose_slot`] prefers. A slot
/// another checkout holds (mid-build, or an exclusive lease) is skipped.
/// `None` when every open connection is in use and none is empty: the caller
/// then shares one. Also returns how many slots were occupied (open, or held
/// by another checkout). No `.await` while the guards are held, so this can't
/// deadlock against another scan.
fn try_pick(pool: &KeyPool, now: u64) -> (Option<(usize, SlotGuard)>, usize) {
    let mut guards: Vec<Option<SlotGuard>> = pool
        .slots
        .iter()
        .map(|s| Arc::clone(s).try_lock_owned().ok())
        .collect();
    let states: Vec<Option<SlotState>> = guards
        .iter()
        .map(|g| g.as_ref().map(|g| slot_state(g, now)))
        .collect();
    let occupied = states
        .iter()
        .filter(|s| **s != Some(SlotState::Vacant))
        .count();
    let picked = pick::choose_slot(&states).and_then(|i| guards[i].take().map(|g| (i, g)));
    (picked, occupied)
}

/// Why a slot's connection must not be handed out, or `None` to reuse it.
///
/// Two separate reasons, and **both are needed** — see [`expiry`] for why
/// keeping only the liveness check is the tempting mistake. The credential
/// check is what stops a rebuild from landing on a request's critical path;
/// the liveness check still catches everything else that kills a connection
/// (a DP restart, a network drop).
fn reuse_block(conn: &AirhouseConnector, now: u64) -> Option<&'static str> {
    if !conn.is_live() {
        return Some("connection closed");
    }
    if expiry::credential_spent(conn.credential_deadline(), now, expiry::RETIRE_BUFFER_SECS) {
        return Some("credential near expiry");
    }
    None
}

fn slot_state(slot: &Option<Arc<AirhouseConnector>>, now: u64) -> SlotState {
    match slot {
        None => SlotState::Vacant,
        // Dead, or holding a credential airhouse is about to stop accepting:
        // either way the next checkout rebuilds, so the slot is free to take.
        Some(conn) if reuse_block(conn, now).is_some() => SlotState::Vacant,
        // The slot's own reference is the only one: no caller still holds the
        // connector (a caller's `Arc<dyn DatabaseConnector>` shares this count).
        Some(conn) if Arc::strong_count(conn) == 1 => SlotState::Idle,
        Some(_) => SlotState::InUse,
    }
}

/// Free process-budget permits ([`ceiling`]) held by connections nobody is
/// using, so a connect is refused at the ceiling only when the budget is
/// genuinely in use.
///
/// Permits ride the connector, and a connector sits in its slot until the
/// identity is evicted or a checkout lands on that slot. Nothing else gives a
/// permit back, so without this the ceiling refuses a new identity while the
/// process is completely idle, and after a data-plane restart every dead slot
/// goes on holding one until the idle sweep runs.
///
/// Two passes, oldest identity first. A dead or credential-spent connection
/// can never be handed out again, so it is always cleared. An **idle** one (the
/// slot's reference is the only one) would have been reused, so those are
/// cleared only until the caller has something to take.
///
/// `try_lock_owned` only, and no `.await` while a slot guard is held: a slot
/// another checkout holds — including the caller's own, mid-rebuild — is
/// skipped rather than waited on, so this cannot deadlock against a build.
/// Returns how many permits were actually released, which counts only slots
/// whose reference was the last one; clearing a slot whose connection a request
/// still holds is right but returns its permit later, when that request ends.
async fn reclaim_unused_permits(now: u64) -> usize {
    let pools: Vec<(String, Arc<KeyPool>)> = {
        let reg = registry().read().await;
        let mut v: Vec<(String, u64, Arc<KeyPool>)> = reg
            .iter()
            .map(|(k, p)| {
                (
                    k.clone(),
                    p.last_used.load(Ordering::Relaxed),
                    Arc::clone(p),
                )
            })
            .collect();
        v.sort_by_key(|(_, last_used, _)| *last_used);
        v.into_iter().map(|(k, _, p)| (k, p)).collect()
    };

    let mut freed = 0usize;
    // Pass 1: connections that can never serve again.
    for (key, pool) in &pools {
        for slot in &pool.slots {
            let Ok(mut guard) = Arc::clone(slot).try_lock_owned() else {
                continue;
            };
            let Some(state) = guard
                .as_ref()
                .map(|c| (reuse_block(c, now).is_some(), Arc::strong_count(c) == 1))
            else {
                continue;
            };
            if state.0 {
                *guard = None;
                if state.1 {
                    freed += 1;
                }
                tracing::info!(pool_key = %key, "airhouse pool: cleared an unusable slot");
            }
        }
    }
    if freed > 0 {
        return freed;
    }
    // Pass 2: one idle connection, from the identity used longest ago.
    for (key, pool) in &pools {
        for slot in &pool.slots {
            let Ok(mut guard) = Arc::clone(slot).try_lock_owned() else {
                continue;
            };
            if guard.as_ref().is_some_and(|c| Arc::strong_count(c) == 1) {
                *guard = None;
                tracing::info!(pool_key = %key, "airhouse pool: reclaimed an idle slot's permit");
                return 1;
            }
        }
    }
    freed
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
        max_preview_identities: max_preview_identities(),
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
    max_preview_identities: usize,
}

/// [`keys_to_evict`] run separately over app-write identities, preview
/// identities and everything else, so a workspace with several `ctx.airhouse`
/// apps (or a preview run writing several schemas) cannot push the analytics
/// path's sessions out of the pool, and analytics traffic cannot evict an app
/// mid-burst — each eviction costs a mint and a fresh DuckLake session, the
/// load this pool exists to cap.
fn keys_to_evict_by_budget(entries: &[(String, u64)], now: u64, b: Budgets) -> Vec<String> {
    let (apps, rest): (Vec<_>, Vec<_>) = entries
        .iter()
        .cloned()
        .partition(|(k, _)| k.starts_with(APP_KEY_PREFIX));
    let (previews, others): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|(k, _)| k.starts_with(PREVIEW_KEY_PREFIX));
    let mut out = keys_to_evict(&others, now, b.idle_secs, b.max_identities);
    out.extend(keys_to_evict(&apps, now, b.idle_secs, b.max_app_identities));
    out.extend(keys_to_evict(
        &previews,
        now,
        b.idle_secs,
        b.max_preview_identities,
    ));
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

fn max_preview_identities() -> usize {
    env_usize(
        "OXY_AIRHOUSE_POOL_MAX_PREVIEW_IDENTITIES",
        DEFAULT_MAX_PREVIEW_IDENTITIES,
    )
}

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

pub mod ceiling;
mod expiry;
mod pick;

mod backpressure;

#[cfg(test)]
mod tests;
