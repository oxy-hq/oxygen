//! In-memory feature-flag cache with a periodic refresh.
//!
//! Reads are pure HashMap lookups — no DB hit at the read site, ever. Loaded
//! at startup, updated in-place on the local admin PATCH, and re-read from the
//! DB every [`refresh_interval`] so a PATCH on ANY instance reaches every other
//! within that window. That refresh is what makes the fleet-wide claim true:
//! the PATCHed instance flips instantly, the rest converge on the next tick.
//! A switch that only flipped one pod would be a trap during an incident.
//!
//! A tighter bound would be Postgres LISTEN/NOTIFY on the flag table; the
//! interval is the cheaper approximation and enough for a safety switch.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock, RwLock};
use std::time::Duration;

use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseConnection, DbErr};

use super::registry;
use super::store;

static CACHE: OnceLock<RwLock<HashMap<&'static str, bool>>> = OnceLock::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Bumped on every LOCAL write (`set`, from a PATCH on this instance). A refresh
/// snapshots it before its read and skips the install if it moved — so a fetch
/// that started before a PATCH cannot overwrite that PATCH with a stale map.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Production default: re-read the flag table every 15s.
const DEFAULT_REFRESH: Duration = Duration::from_millis(15_000);
/// Floor for a *deliberate* positive override, so a too-small test knob cannot
/// spin the loop. Not applied to `0` — see [`parse_refresh_interval`].
const MIN_REFRESH: Duration = Duration::from_millis(50);

/// How often every instance re-reads the flag table, so a PATCH on one reaches
/// the rest. A staleness bound, not a latency guarantee. Millis, env-overridable
/// (`OXY_FEATURE_FLAG_REFRESH_MS`) so a test can drive a sub-second loop.
fn refresh_interval() -> Duration {
    parse_refresh_interval(std::env::var("OXY_FEATURE_FLAG_REFRESH_MS").ok().as_deref())
}

/// Pure parse of the knob, split from the env read so it is testable without a
/// process-wide `set_var` (unsafe under the shared lib-test binary).
///
/// `0` is the value an operator reaches for to mean "off" — but there is no off
/// here, reads are always served from the cache, so `0` (and anything
/// unparseable) resolves to [`DEFAULT_REFRESH`], NOT the [`MIN_REFRESH`] floor.
/// Flooring `0` to 50ms would turn a stray `=0` into 20Hz per process — 400
/// `fetch_all`/s against the control plane on a twenty-pod fleet, from a
/// plausible mistake. Only a deliberate positive value is floored.
fn parse_refresh_interval(raw: Option<&str>) -> Duration {
    match raw.and_then(|s| s.parse::<u64>().ok()) {
        Some(ms) if ms > 0 => Duration::from_millis(ms).max(MIN_REFRESH),
        _ => DEFAULT_REFRESH,
    }
}

/// Start the periodic refresh and do one load — the caller decides whether a
/// failed load is fatal.
///
/// **Fallible on purpose, because the uninitialized fallback is not safe.**
/// While the cache is unloaded, `is_enabled` returns the registry default, and
/// that is OFF for `billing` — paywall enforcement SKIPPED for every org. So an
/// unloaded cache is fail-OPEN on the money flag, and `serve` treats this
/// `Result` as fatal (`?`): it will not accept requests with an unknown billing
/// state.
///
/// The refresh is spawned UNCONDITIONALLY, before the fallible load, so a
/// caller that tolerates a failed load still self-heals on a later tick. Takes
/// NO connection argument — it opens its own.
pub async fn init() -> Result<(), OxyError> {
    spawn_refresh();
    load_and_install().await
}

/// Open a connection, read the flag table, and install the map — unless a local
/// PATCH raced the read, in which case its value already won and this snapshot
/// is stale.
async fn load_and_install() -> Result<(), OxyError> {
    let db = oxy::database::client::establish_connection().await?;
    let gen_before = GENERATION.load(Ordering::Acquire);
    let map = load_map(&db)
        .await
        .map_err(|e| OxyError::Database(format!("feature flags fetch: {e}")))?;
    install_if_current(map, gen_before);
    Ok(())
}

/// Read the flag table and fold it onto the registry defaults.
async fn load_map(db: &DatabaseConnection) -> Result<HashMap<&'static str, bool>, DbErr> {
    let rows = store::fetch_all(db).await?;
    let mut by_key: HashMap<&str, bool> = HashMap::new();
    for row in &rows {
        if registry::get(&row.key).is_some() {
            by_key.insert(row.key.as_str(), row.enabled);
        } else {
            warn_stale_once(&row.key);
        }
    }
    let mut map: HashMap<&'static str, bool> = HashMap::new();
    for flag in registry::FLAGS {
        map.insert(
            flag.key,
            by_key
                .get(flag.key)
                .copied()
                .unwrap_or(flag.default_enabled),
        );
    }
    Ok(map)
}

/// Warn once per process about a DB row whose key the registry no longer
/// declares. `load_map` runs on every refresh tick, so warning per call would
/// log every 15s on every instance for as long as a removed flag's row stays.
/// The row is left in place on purpose: a rollback to a build that still
/// declares the flag reads it back.
fn warn_stale_once(key: &str) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = WARNED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if seen.insert(key.to_string()) {
        tracing::warn!(key, "stale feature flag in DB, ignoring");
    }
}

/// Install a freshly-loaded map — unless a local PATCH landed since `gen_before`
/// was snapshotted, in which case its value stands.
///
/// The generation check happens UNDER the write lock, and [`set`] bumps the
/// counter under the same lock, so check-and-install and bump-and-write are one
/// critical section: a PATCH cannot interleave between our check and our write.
/// Reading `gen_before` before the (awaited) `fetch_all` and re-checking here is
/// what makes "the refresh cannot revert a just-committed PATCH" hold rather
/// than merely narrow.
fn install_if_current(map: HashMap<&'static str, bool>, gen_before: u64) {
    match CACHE.get() {
        Some(cache) => {
            let mut guard = cache.write().unwrap_or_else(|p| p.into_inner());
            if GENERATION.load(Ordering::Acquire) != gen_before {
                return; // a PATCH raced our read; keep its value
            }
            *guard = map;
        }
        // First install — nothing serves yet, so no PATCH can race it.
        None => {
            let _ = CACHE.set(RwLock::new(map));
        }
    }
    INITIALIZED.store(true, Ordering::Release);
}

/// Re-read the flag table every [`refresh_interval`] and install it, so a PATCH
/// on another instance propagates fleet-wide. Spawned unconditionally, so it
/// also repairs a failed initial load. A failed read keeps the current values —
/// a transient blip must not flip a flag — and logs.
///
/// **A raw `tokio::spawn` loop, not a `TaskSpec`, on purpose.** The
/// `oxy-task-spec-default` rule routes background work to the durable queue —
/// but that is for work that must survive instance death and run once across
/// the fleet. This refreshes THIS process's in-memory cache, so it must run in
/// every process and cannot be a queued job. It is detached (no shutdown
/// token): it only reads and swaps a `HashMap`, holds nothing a drain must
/// flush, and the runtime reaps it on exit — so a token would be plumbing for
/// no gain, and plumbing that would also reintroduce an argument to the no-arg
/// `init` whose whole point is that no caller can skip the hook.
fn spawn_refresh() {
    // Read the interval ONCE, at spawn — not per tick. A per-tick `std::env::var`
    // is a syscall on a timer forever, and under Rust 2024 a concurrent `set_var`
    // anywhere in the process is UB against it. The test sets the env before
    // `init` (which calls this), so the knob is still honoured.
    let interval = refresh_interval();
    // Log the resolved value so a mis-set `OXY_FEATURE_FLAG_REFRESH_MS` (e.g. a
    // `0` that quietly became 15s) is visible rather than silent.
    tracing::info!(
        refresh_ms = interval.as_millis() as u64,
        "feature flag cache refresh interval resolved"
    );
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = load_and_install().await {
                tracing::warn!(error = %e, "feature flag refresh failed; keeping current values");
            }
        }
    });
}

/// Returns whether `key` is enabled. Synchronous — pure HashMap lookup after
/// init. While the cache is uninitialized, returns the registry default for
/// `key` and warns ONCE — read sites call this per request, so a per-call log
/// would flood and bury the init warning. Unknown keys return `false`.
pub fn is_enabled(key: &'static str) -> bool {
    if !INITIALIZED.load(Ordering::Acquire) {
        static WARNED: Once = Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                "feature_flags read before the cache is loaded; using registry defaults \
                 until the refresh installs values"
            );
        });
        return registry::default_for(key);
    }
    let Some(cache) = CACHE.get() else {
        return registry::default_for(key);
    };
    let guard = match cache.read() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    match guard.get(key).copied() {
        Some(v) => v,
        None => {
            tracing::warn!(key, "feature_flags::is_enabled called with unknown key");
            false
        }
    }
}

/// Overwrites the cache entry for `key` with `enabled`. Called by the PATCH
/// handler after the DB write commits. Bumps [`GENERATION`] so an in-flight
/// refresh cannot install a snapshot older than this write.
pub fn set(key: &'static str, enabled: bool) {
    let Some(cache) = CACHE.get() else {
        tracing::error!(key, "feature_flags::set called before cache init");
        return;
    };
    let mut guard = match cache.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    // Bump UNDER the write lock, so `install_if_current`'s check-under-lock sees
    // this write as either wholly before or wholly after it — never interleaved.
    GENERATION.fetch_add(1, Ordering::AcqRel);
    guard.insert(key, enabled);
}

#[cfg(test)]
pub fn init_for_tests(values: HashMap<&'static str, bool>) {
    if CACHE.get().is_none() {
        let _ = CACHE.set(RwLock::new(HashMap::new()));
    }
    let cache = CACHE.get().expect("cache slot installed above");
    let mut guard = match cache.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *guard = values;
    INITIALIZED.store(true, Ordering::Release);
}

#[cfg(test)]
pub fn override_for_tests(key: &'static str, enabled: bool) {
    if CACHE.get().is_none() {
        let _ = CACHE.set(RwLock::new(HashMap::new()));
    }
    INITIALIZED.store(true, Ordering::Release);
    set(key, enabled);
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_REFRESH, MIN_REFRESH, parse_refresh_interval};
    use std::time::Duration;

    #[test]
    fn zero_and_garbage_resolve_to_the_default_not_the_floor() {
        // `0` is what an operator sets to mean "off". There is no off — reads
        // are always cache-served — so it must NOT floor to 50ms (20Hz per
        // process forever); it, and anything unparseable, is the 15s default.
        assert_eq!(parse_refresh_interval(Some("0")), DEFAULT_REFRESH);
        assert_eq!(parse_refresh_interval(None), DEFAULT_REFRESH);
        assert_eq!(parse_refresh_interval(Some("")), DEFAULT_REFRESH);
        assert_eq!(parse_refresh_interval(Some("nope")), DEFAULT_REFRESH);
        assert_eq!(parse_refresh_interval(Some("-1")), DEFAULT_REFRESH);
    }

    #[test]
    fn a_deliberate_positive_value_is_honoured_and_floored() {
        // Only a positive value is floored — the test knob stays usable, a
        // too-small one cannot spin the loop.
        assert_eq!(parse_refresh_interval(Some("1")), MIN_REFRESH);
        assert_eq!(parse_refresh_interval(Some("50")), MIN_REFRESH);
        assert_eq!(
            parse_refresh_interval(Some("100")),
            Duration::from_millis(100)
        );
        assert_eq!(
            parse_refresh_interval(Some("30000")),
            Duration::from_millis(30_000)
        );
    }
}
