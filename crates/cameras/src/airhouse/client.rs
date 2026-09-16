//! Persistent, **per-tenant** Airhouse clients for the camera fleet.
//!
//! # Why this exists
//!
//! Airhouse speaks the Postgres wire protocol but backs **every** accepted
//! connection with its own server-side DuckDB session — a fresh in-memory
//! DuckDB instance, the DuckLake / h3 / spatial extensions, and an `ATTACH`
//! of the tenant's catalog (see `airhouse-server/src/session.rs`). That
//! per-connection session is not cheap and is not reused server-side.
//!
//! The previous edge-ingest path opened a brand-new `tokio_postgres::Client`
//! on every `write_*` call and dropped it immediately. A fleet of edge boxes
//! POSTing health every few seconds therefore churned ~hundreds of fresh
//! DuckDB sessions per minute against a single tenant, which OOM'd Airhouse.
//!
//! # What this does
//!
//! Keep **one** long-lived `tokio_postgres::Client` per
//! `(workspace_id, role, purpose)` — i.e. per tenant + access pattern —
//! behind a background reconnect driver (a pattern originally shared with the
//! since-removed airhouse observability backend). The first write per tenant opens the
//! connection; every subsequent write reuses it, so session churn collapses
//! from N-per-minute to ~1-per-tenant. A dropped connection is re-established
//! in the background with exponential backoff, **re-minting** the ephemeral
//! credential through the SA broker on each attempt (pgwire auth happens once
//! at connect, so a live connection outlives its minting credential's TTL).
//!
//! # Liveness
//!
//! Reuse is only safe while the handle's reconnect driver is running — it is
//! the driver, not the cached `Client`, that heals a dropped connection. Each
//! handle therefore carries an `alive` flag cleared by the driver task's
//! [`AliveGuard`] on *any* exit, and [`tenant_client`] evicts a handle whose
//! driver is gone instead of serving it. Without that check a driverless
//! handle stayed cached for the process's lifetime and failed every query with
//! `connection closed` while never attempting to reconnect.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OnceCell, RwLock};
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};
use tokio_postgres_rustls::MakeRustlsConnect;
use uuid::Uuid;

use airhouse::{AirhouseConfig, SystemPurpose, UserRole};

use super::AirhouseError;

/// A boxed connection-driver future (the TLS stream type is erased so the
/// same `loop` can drive a `NoTls` or rustls connection across reconnects).
type BoxConn = Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static>>;

/// Async callback returning a fresh `(user, password, database)` triple.
/// Invoked once at construction and again on every reconnect so the
/// SA-broker-minted ephemeral credential is transparently refreshed.
type CredFn = Arc<
    dyn Fn()
            -> Pin<Box<dyn Future<Output = Result<(String, String, String), AirhouseError>> + Send>>
        + Send
        + Sync,
>;

/// A persistent pgwire client to one Airhouse tenant, transparently
/// reconnected on drop. Cloneable handle (`Arc`) shared by all callers for
/// the same `(workspace, role, purpose)`.
pub struct TenantClient {
    /// Swapped by the reconnect driver; read-locked only to clone the inner
    /// `Arc<Client>`, so concurrent queries on the same tenant do not
    /// serialize on this lock.
    client: Arc<RwLock<Arc<Client>>>,
    /// Cleared when the reconnect driver task ends, for **any** reason —
    /// give-up bound, panic, or the runtime dropping the task. It is the
    /// driver, not the inner `Client`, that makes this handle usable: while
    /// the driver lives it swaps a fresh `Client` in after every drop, so a
    /// momentarily-closed inner client is NOT dead. Once the driver is gone
    /// nothing will ever reconnect, and every query on this handle fails with
    /// `connection closed` forever — see [`is_live`](Self::is_live).
    alive: Arc<AtomicBool>,
}

impl std::fmt::Debug for TenantClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantClient").finish_non_exhaustive()
    }
}

impl TenantClient {
    /// Run a simple-query against the live connection.
    ///
    /// Signature matches [`tokio_postgres::Client::simple_query`] so existing
    /// call sites (`client.simple_query(&sql).await`) compile unchanged. If
    /// the connection is mid-reconnect the call surfaces the driver error;
    /// the caller retries on its next tick and the background driver will
    /// have re-established the connection by then.
    pub async fn simple_query(
        &self,
        sql: &str,
    ) -> Result<Vec<SimpleQueryMessage>, tokio_postgres::Error> {
        let client = Arc::clone(&*self.client.read().await);
        client.simple_query(sql).await
    }

    /// True while this handle's reconnect driver is still running. Cheap,
    /// non-blocking, no round-trip — safe to call on every checkout.
    ///
    /// Deliberately tracks the *driver*, not `Client::is_closed()`: a driver
    /// that is mid-reconnect holds a closed inner client for a moment, and
    /// treating that as dead would evict a perfectly good entry and spawn a
    /// second driver for the same tenant (which the registry exists to
    /// prevent).
    pub fn is_live(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

/// Which airhouse DP pool a client connects to.
///
/// The DP fleet is split: a serving/ingest pool on `AIRHOUSE_WIRE_HOST` and an
/// optional analytics pool on `AIRHOUSE_ANALYTICS_WIRE_HOST`, fronted by
/// separate HAProxy backends (`dp` / `dp-analytics`) over separate pods. The
/// split exists so heavy OLAP can't contend with latency-sensitive ingest —
/// and, just as importantly, so one side failing doesn't take the other with
/// it: each DP has its own circuit breaker, and a write-path fault trips only
/// the pool it happens on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pool {
    /// The serving/ingest DP. Edge writes belong here — they're small, hot, and
    /// what that pool is sized for.
    Serving,
    /// The analytics DP, when `AIRHOUSE_ANALYTICS_WIRE_HOST` is set; otherwise
    /// falls back to serving, so deployments without a separate pool are
    /// unaffected.
    Analytics,
}

impl Pool {
    /// Resolve to a concrete `(host, port)`, falling back to the serving
    /// endpoint when no analytics pool is configured.
    fn endpoint(self, cfg: &airhouse::AirhouseRuntimeConfig) -> (String, u16) {
        self.resolve(cfg, airhouse::analytics_wire_endpoint())
    }

    /// The routing decision, with the analytics endpoint injected rather than
    /// read from the environment — `set_var` is not thread-safe, so a test that
    /// reached for the env would race every other test in the binary that reads
    /// it.
    fn resolve(
        self,
        cfg: &airhouse::AirhouseRuntimeConfig,
        analytics: Option<airhouse::WireEndpoint>,
    ) -> (String, u16) {
        let serving = || (cfg.wire_host.clone(), cfg.wire_port);
        match self {
            Pool::Serving => serving(),
            Pool::Analytics => analytics.map_or_else(serving, |e| (e.host, e.port)),
        }
    }
}

// ── Per-tenant registry ─────────────────────────────────────────────────────

/// Key: one persistent connection per tenant **and** access pattern. Reader
/// vs Writer vs Admin connect with different minted credentials, and the
/// `purpose` segments the airhouse audit log, so they must not share a
/// connection.
///
/// `purpose` also keeps the pools apart: reads and writes already carry
/// distinct purposes, so a client on the analytics pool can never be handed to
/// a caller that wanted the serving pool.
type Key = (Uuid, UserRole, &'static str);

/// Per-key lazily-initialised client cell. The `OnceCell` single-flights the
/// first connect per key without holding the outer lock across the network
/// round-trip.
type Cell = Arc<OnceCell<Arc<TenantClient>>>;

/// Map of key → client cell. Guarantees we never spawn two perpetual
/// reconnect drivers for the same tenant (a duplicate driver would leak a
/// connection for the whole process lifetime).
fn registry() -> &'static Mutex<HashMap<Key, Cell>> {
    static R: OnceLock<Mutex<HashMap<Key, Cell>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get (or lazily open) the persistent client for a tenant + access pattern.
///
/// `ttl` is the lifetime of each minted credential; reuse of the *connection*
/// is independent of it (auth is checked once at connect), so a short TTL no
/// longer forces a reconnect.
pub async fn tenant_client(
    workspace_id: Uuid,
    role: UserRole,
    purpose: SystemPurpose,
    ttl: Duration,
    pool: Pool,
) -> Result<Arc<TenantClient>, AirhouseError> {
    let key = (workspace_id, role, purpose.as_str());
    let cell = {
        let mut guard = registry().lock().await;
        // Drop a cached entry whose driver is gone before handing it out.
        // `OnceCell` caches the first successful connect forever, so without
        // this a handle orphaned by a dead driver is returned to every future
        // caller and fails every query with `connection closed` until the
        // process restarts — no reconnect is ever attempted, because the only
        // thing that reconnects is the task that died. (Prod 2026-07-16: the
        // cameras ingest and `/cameras/health-summary` wedged this way for
        // hours against a healthy DP.) Safe against the mid-reconnect race:
        // `is_live` tracks the driver, not the inner client.
        if guard
            .get(&key)
            .and_then(|cell| cell.get())
            .is_some_and(|client| !client.is_live())
        {
            tracing::warn!(
                "cameras airhouse: cached tenant client has no live driver; \
                 evicting so the next connect rebuilds it"
            );
            guard.remove(&key);
        }
        guard
            .entry(key)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };
    // On error the cell stays uninitialised, so the next caller retries the
    // connect rather than caching a permanent failure.
    cell.get_or_try_init(|| async {
        TenantClient::connect(workspace_id, role, purpose, ttl, pool)
            .await
            .map(Arc::new)
    })
    .await
    .cloned()
}

impl TenantClient {
    async fn connect(
        workspace_id: Uuid,
        role: UserRole,
        purpose: SystemPurpose,
        ttl: Duration,
        pool: Pool,
    ) -> Result<Self, AirhouseError> {
        let cfg = match AirhouseConfig::from_env() {
            AirhouseConfig::Enabled(c) => c,
            _ => return Err(AirhouseError::Disabled),
        };
        // Captured for the driver's lifetime, so reconnects stay on the pool
        // this client was opened against.
        let (host, port) = pool.endpoint(&cfg);
        let insecure = insecure_from_env();
        let max_reconnect_attempts = super::max_reconnect_attempts();
        let get_credentials = make_cred_fn(workspace_id, role, purpose, ttl);

        let (user, password, database) = get_credentials().await?;
        let config = make_pg_config(&host, port, &user, &password, &database);
        let (client, conn) = try_connect(&config, insecure)
            .await
            .map_err(|e| AirhouseError::Connect(super::pg_error_text(&e)))?;

        let client_ref = Arc::new(RwLock::new(Arc::new(client)));
        let alive = Arc::new(AtomicBool::new(true));
        spawn_driver(DriverCtx {
            conn,
            client_ref: Arc::clone(&client_ref),
            alive: Arc::clone(&alive),
            host,
            port,
            insecure,
            get_credentials,
            max_reconnect_attempts,
        });
        Ok(Self {
            client: client_ref,
            alive,
        })
    }
}

// ── Credential refresh ──────────────────────────────────────────────────────

/// Build a [`CredFn`] that mints a fresh ephemeral credential via the SA
/// broker for this `(workspace, role, purpose)`. The broker caches mints by
/// the same tuple, so steady-state calls are cheap; a reconnect after a long
/// idle period re-mints transparently.
fn make_cred_fn(
    workspace_id: Uuid,
    role: UserRole,
    purpose: SystemPurpose,
    ttl: Duration,
) -> CredFn {
    Arc::new(move || {
        Box::pin(async move {
            let broker = airhouse::token_broker().ok_or(AirhouseError::Disabled)?;
            let cred = broker
                .mint_for_system(workspace_id, purpose, role, ttl)
                .await
                .map_err(|e| AirhouseError::Mint(e.to_string()))?;
            Ok((cred.username, cred.password, cred.tenant))
        })
    })
}

// ── Reconnect driver ────────────────────────────────────────────────────────

/// A connection that stays up at least this long is considered healthy and
/// resets the give-up counter. One that drops sooner counts as a "flap" — the
/// connect succeeded but the server tore the session down almost immediately —
/// which is bounded just like a connect failure. An ingest connection is reused
/// and stays open for minutes/hours (idle TCP doesn't drop), so a sub-30s drop
/// is abnormal.
const MIN_STABLE_UPTIME: Duration = Duration::from_secs(30);

/// Everything the background driver needs. Grouped into a struct so the spawn
/// call stays under clippy's argument-count limit.
struct DriverCtx {
    conn: BoxConn,
    client_ref: Arc<RwLock<Arc<Client>>>,
    /// Cleared via [`AliveGuard`] when this driver task ends.
    alive: Arc<AtomicBool>,
    host: String,
    port: u16,
    insecure: bool,
    get_credentials: CredFn,
    /// Consecutive unhealthy reconnect cycles (connect/auth failures **or**
    /// rapid flaps) tolerated before the driver gives up and evicts the tenant;
    /// `0` means retry forever. Reset whenever a connection stays up past
    /// [`MIN_STABLE_UPTIME`].
    max_reconnect_attempts: u32,
}

/// Give up if `failures` has reached the bound. Returns `true` when the caller
/// should stop the driver. Shared by the flap path and the connect-failure path
/// so the give-up policy lives in one place.
///
/// Eviction is *not* done here. Returning `true` drops the driver's
/// [`AliveGuard`], which clears the handle's `alive` flag; the next
/// [`tenant_client`] checkout sees the dead handle and rebuilds it. Removing
/// the entry by key from here would race that rebuild and could evict a
/// freshly-connected replacement.
fn give_up_if_exhausted(max: u32, failures: u32, reason: &str) -> bool {
    if max == 0 || failures < max {
        return false;
    }
    tracing::warn!(
        "cameras airhouse giving up after {failures} {reason}; \
         tenant connection will be re-established on next use"
    );
    true
}

/// Clears a [`TenantClient`]'s `alive` flag when the driver task ends for
/// **any** reason — the give-up bound, a panic, or the runtime dropping the
/// task during shutdown. This is what makes a driverless handle detectable:
/// the panic/abort paths never run the normal give-up code, and previously
/// left the registry caching a handle nothing would ever reconnect.
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Drive the pgwire connection future and reconnect with exponential backoff
/// when it ends. A single spawned task owns the whole lifecycle; the inner loop
/// handles every reconnect attempt so no task is spawned per reconnect. Adds a give-up bound so a
/// deprovisioned / long-dead tenant doesn't keep a reconnect task (and its log
/// spam) and a server-side DuckDB session alive forever. The bound trips on
/// **both** repeated connect/auth failures (can't connect at all) **and**
/// repeated flaps (connect succeeds but the session drops within
/// [`MIN_STABLE_UPTIME`]); either way the handle is marked dead and the next
/// request re-establishes lazily. A connection that stays up resets the count.
fn spawn_driver(ctx: DriverCtx) {
    let DriverCtx {
        mut conn,
        client_ref,
        alive,
        host,
        port,
        insecure,
        get_credentials,
        max_reconnect_attempts,
    } = ctx;
    tokio::spawn(async move {
        // Marks the handle dead however this task ends — including a panic or
        // an abort, which skip every `return` below.
        let _alive_guard = AliveGuard(alive);
        let mut connected_at = Instant::now();
        let mut failures: u32 = 0;
        'session: loop {
            match conn.await {
                Err(e) => tracing::warn!("cameras airhouse connection dropped: {e}"),
                Ok(()) => {
                    tracing::info!("cameras airhouse connection closed cleanly; reconnecting")
                }
            }

            // A connection that stayed up is healthy → reset. One that dropped
            // almost immediately is flapping → count it toward the give-up bound
            // (otherwise a server tearing sessions down on connect would churn
            // forever, reintroducing the very session churn this module fixes).
            if connected_at.elapsed() >= MIN_STABLE_UPTIME {
                failures = 0;
            } else {
                failures += 1;
                if give_up_if_exhausted(max_reconnect_attempts, failures, "rapid reconnect flaps") {
                    return;
                }
            }

            let mut delay = Duration::from_millis(200);
            loop {
                tokio::time::sleep(delay).await;

                // On success, reassign `conn` + `connected_at` and
                // `continue 'session` adjacently so the borrow checker can prove
                // `conn` is live at the outer loop's next `conn.await`.
                match get_credentials().await {
                    Err(e) => {
                        tracing::warn!(
                            "cameras airhouse credential refresh failed: {e}, retrying in {delay:?}"
                        );
                    }
                    Ok((user, password, database)) => {
                        let config = make_pg_config(&host, port, &user, &password, &database);
                        match try_connect(&config, insecure).await {
                            Ok((new_client, new_conn)) => {
                                *client_ref.write().await = Arc::new(new_client);
                                tracing::info!("cameras airhouse reconnected");
                                conn = new_conn;
                                connected_at = Instant::now();
                                continue 'session; // drive the new connection
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "cameras airhouse reconnect failed: {e}, retrying in {delay:?}"
                                );
                            }
                        }
                    }
                }

                // Reached only on a failed attempt (success `continue`s above).
                failures += 1;
                if give_up_if_exhausted(max_reconnect_attempts, failures, "reconnect failures") {
                    return;
                }
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    });
}

// ── Low-level connection helpers ────────────────────────────────────────────

pub(super) fn make_pg_config(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    database: &str,
) -> tokio_postgres::Config {
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(host);
    cfg.port(port);
    cfg.user(user);
    cfg.password(password);
    cfg.dbname(database);
    cfg
}

pub(super) async fn try_connect(
    config: &tokio_postgres::Config,
    insecure: bool,
) -> Result<(Client, BoxConn), tokio_postgres::Error> {
    if insecure {
        let (c, conn) = config.connect(NoTls).await?;
        Ok((c, Box::pin(conn)))
    } else {
        let (c, conn) = config.connect(tls_connector()).await?;
        Ok((c, Box::pin(conn)))
    }
}

/// TLS on by default; opt out with `OXY_AIRHOUSE_OBS_INSECURE` for
/// localhost / trusted-network deployments. Shares the env var with the
/// observability backend so operators flip one switch.
pub(super) fn insecure_from_env() -> bool {
    std::env::var("OXY_AIRHOUSE_OBS_INSECURE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

pub(super) fn tls_connector() -> MakeRustlsConnect {
    static TLS: OnceLock<MakeRustlsConnect> = OnceLock::new();
    TLS.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        MakeRustlsConnect::new(cfg)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn give_up_bound_of_zero_retries_forever() {
        assert!(!give_up_if_exhausted(0, u32::MAX, "test"));
    }

    #[test]
    fn give_up_trips_only_once_failures_reach_the_bound() {
        assert!(!give_up_if_exhausted(3, 2, "test"));
        assert!(give_up_if_exhausted(3, 3, "test"));
    }

    /// The give-up bound is not the only way a driver dies — a panic unwinds
    /// past every `return`. The guard must still mark the handle dead, or
    /// `tenant_client` keeps serving a handle that nothing will ever
    /// reconnect, failing every query with `connection closed` until the
    /// process restarts (prod wedge, 2026-07-16).
    #[tokio::test]
    async fn driver_panic_marks_the_handle_dead() {
        let alive = Arc::new(AtomicBool::new(true));
        let held = Arc::clone(&alive);
        let driver = tokio::spawn(async move {
            let _guard = AliveGuard(held);
            panic!("driver blew up");
        });

        assert!(driver.await.is_err(), "task should have panicked");
        assert!(
            !alive.load(Ordering::Acquire),
            "a panicking driver must leave the handle marked dead"
        );
    }

    /// The handle stays usable for exactly as long as its driver holds the
    /// guard — a driver that is merely mid-reconnect must NOT read as dead, or
    /// checkout would evict it and spawn a second driver for the tenant, which
    /// the registry exists to prevent. Any exit then clears the flag, however
    /// the task unwound.
    #[test]
    fn handle_is_live_while_the_driver_holds_the_guard_and_dead_once_it_drops() {
        let alive = Arc::new(AtomicBool::new(true));
        {
            let _guard = AliveGuard(Arc::clone(&alive));
            assert!(
                alive.load(Ordering::Acquire),
                "handle must stay live while its driver runs"
            );
        }
        assert!(
            !alive.load(Ordering::Acquire),
            "handle must read dead once its driver's guard drops"
        );
    }

    fn cfg() -> airhouse::AirhouseRuntimeConfig {
        airhouse::AirhouseRuntimeConfig {
            base_url: "http://cp:8080".into(),
            admin_token: "t".into(),
            wire_host: "serving-host".into(),
            wire_port: 5445,
        }
    }

    fn analytics() -> Option<airhouse::WireEndpoint> {
        Some(airhouse::WireEndpoint {
            host: "analytics-host".into(),
            port: 5446,
        })
    }

    #[test]
    fn serving_pool_ignores_the_analytics_endpoint() {
        assert_eq!(
            Pool::Serving.resolve(&cfg(), analytics()),
            ("serving-host".to_string(), 5445),
            "edge writes must stay on the serving DP even when an analytics pool exists"
        );
    }

    #[test]
    fn analytics_pool_routes_to_the_analytics_endpoint() {
        assert_eq!(
            Pool::Analytics.resolve(&cfg(), analytics()),
            ("analytics-host".to_string(), 5446),
            "dashboard reads must route to the analytics DP"
        );
    }

    /// Deployments without a separate analytics pool must be unaffected — the
    /// read path keeps working against the serving DP exactly as before.
    #[test]
    fn analytics_pool_falls_back_to_serving_when_unconfigured() {
        assert_eq!(
            Pool::Analytics.resolve(&cfg(), None),
            ("serving-host".to_string(), 5445),
            "no analytics pool configured ⇒ fall back to serving, not fail"
        );
    }
}
