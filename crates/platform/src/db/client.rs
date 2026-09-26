use std::time::Duration;

use oxy_shared::errors::OxyError;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, SqlxPostgresConnector};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use tokio::sync::OnceCell;

use super::auth_mode::{DatabaseAuthMode, IamConfig, SslMode};
use super::iam;
use super::pool_probe;

static DB_POOL: OnceCell<DatabaseConnection> = OnceCell::const_new();

// Connection pool sizing — applied identically across auth modes so prod/dev
// behave the same under load.
//
// The MIN default is deliberately small. min_connections is a floor that
// idle_timeout never reaps below, so a large min just pins idle backends and
// consumes Postgres max_connections — multiplied across every pod. The
// control-plane pool is almost always idle (a fleet is typically ~1 active
// query at a time), yet a 6-pod oxy-dev fleet at min=20 held ~120 idle backends
// and starved a deploy-time `oxy migrate` pre-upgrade hook of connections
// (2026-07-09). Connections still grow toward max under real load and shrink
// back to min via idle_timeout.
//
// Both bounds are env-overridable (OXY_DATABASE_{MAX,MIN}_CONNECTIONS) so infra
// can size them per environment / per component without a rebuild.
const DEFAULT_MAX_CONNECTIONS: u32 = 80;
const DEFAULT_MIN_CONNECTIONS: u32 = 2;
const MAX_CONNECTIONS_ENV: &str = "OXY_DATABASE_MAX_CONNECTIONS";
const MIN_CONNECTIONS_ENV: &str = "OXY_DATABASE_MIN_CONNECTIONS";
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_LIFETIME: Duration = Duration::from_secs(1800);

/// Parse a pool-size bound, falling back to `default` when the value is absent
/// or unparseable. Split from the env lookup so it can be unit-tested without
/// touching process-global environment.
fn resolve_pool_bound(raw: Option<&str>, default: u32) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

/// Max connections per pool. Override with `OXY_DATABASE_MAX_CONNECTIONS`.
/// Clamped to >= 1 (sqlx requires a positive ceiling).
fn max_connections() -> u32 {
    resolve_pool_bound(
        std::env::var(MAX_CONNECTIONS_ENV).ok().as_deref(),
        DEFAULT_MAX_CONNECTIONS,
    )
    .max(1)
}

/// Min (idle-floor) connections per pool. Override with
/// `OXY_DATABASE_MIN_CONNECTIONS`. Clamped to <= max so an env misconfig can't
/// make the floor exceed the ceiling.
fn min_connections() -> u32 {
    resolve_pool_bound(
        std::env::var(MIN_CONNECTIONS_ENV).ok().as_deref(),
        DEFAULT_MIN_CONNECTIONS,
    )
    .min(max_connections())
}

// Refresh IAM tokens every 10 min, leaving a 5-min headroom before the
// 15-min RDS token TTL expires. New physical connections always use a
// token at most ~10 min old.
const IAM_TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(600);

// On refresh failure, retry aggressively until success. Without this, a
// single transient AWS credential-provider blip at t=10m would wait the
// full 10-min cadence before retrying at t=20m — but the baked token
// expires at t=15m, so new connections would start failing for 5 minutes
// before the next attempt. 60s retry closes that window.
const IAM_TOKEN_REFRESH_RETRY: Duration = Duration::from_secs(60);

pub async fn establish_connection() -> Result<DatabaseConnection, OxyError> {
    DB_POOL
        .get_or_try_init(|| async {
            let db = match DatabaseAuthMode::from_env()? {
                DatabaseAuthMode::Password => connect_password().await,
                DatabaseAuthMode::Iam => connect_iam().await,
            }?;
            // Inside the `OnceCell` init, so exactly one monitor per process,
            // watching the one pool this process will ever use.
            spawn_pool_health_monitor(db.get_postgres_connection_pool().clone());
            Ok(db)
        })
        .await
        .cloned()
}

// ---- pool health ----------------------------------------------------------

/// How often the pool-health monitor samples the pool.
const POOL_HEALTH_INTERVAL: Duration = Duration::from_secs(30);

/// How long a healthy pool is allowed to take to hand out a connection before
/// the monitor calls it starved. A warm pool answers in microseconds, so this
/// is three orders of magnitude of headroom, not a tight bound.
const POOL_HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Watch the pool and, when it stops handing out connections, say **why** —
/// naming the server-side error rather than letting every caller discover it as
/// a mute 30-second hang.
///
/// This exists because of a real outage. `sqlx` retries a refused connect
/// internally and discards the error; with `sqlx_logging(false)` the only thing
/// that ever reached our logs was `"Connection pool timed out"` after the full
/// [`ACQUIRE_TIMEOUT`]. Postgres had been rejecting us for hours with
/// `FATAL: sorry, too many clients already` and *none of it was visible on this
/// side* — the incident (2026-09-01) presented purely as "the homepage feels
/// slow", and the actual cause was only found by reading the RDS error log.
/// The probe below recovers exactly that message, in-process, within 30s.
/// Below this many affordable processes, the fleet arithmetic is called out at
/// startup. `internal-docs/adr-postgres-as-worker-queue.md` §3 sizes the server
/// at `max_connections >= 4 × (HTTP fleet + worker fleet)`; a real deployment is
/// an ide + a serve fleet + a worker fleet, so a pool ceiling that lets fewer
/// than this many processes coexist cannot satisfy that rule for any fleet worth
/// running.
const MIN_AFFORDABLE_PROCESSES: u32 = 8;

/// State the connection budget out loud, once, at startup.
///
/// The 2026-09-01 outage was a division nobody performed: a pool ceiling of 80
/// per process against a server ceiling of ~190 affords **two** processes, and
/// the fleet had seven. Nothing in the system had an opinion about that until
/// Postgres started refusing clients. This makes the arithmetic a startup log
/// line, so the next time a fleet grows the mismatch is visible on the first
/// boot rather than after a day of accumulation.
async fn log_connection_budget(pool: &sqlx::PgPool) {
    let pool_max = max_connections();
    let server_max: Option<u32> = sqlx::query_scalar::<_, String>("SHOW max_connections")
        .fetch_one(pool)
        .await
        .ok()
        .and_then(|v| v.trim().parse().ok());
    let Some(server_max) = server_max else {
        // Not fatal, and not worth a warning: some managed proxies hide this.
        tracing::debug!(pool_max, "could not read the server's max_connections");
        return;
    };
    let affordable = server_max / pool_max.max(1);
    // Only a FLEET can be oversubscribed. A single process is entitled to the
    // whole server, and a dev box — embedded Postgres at `max_connections = 100`
    // against the default pool ceiling of 80 — affords exactly one by
    // construction. Erroring there would fire on every local boot and teach
    // people to ignore the line, which costs more than it could ever save.
    // `OXY_ROLE` is set per-role in a deployed fleet and unset locally, so it is
    // the fact itself rather than a flag invented for this check.
    let in_fleet = std::env::var("OXY_ROLE").is_ok_and(|v| !v.trim().is_empty());
    if in_fleet && affordable < MIN_AFFORDABLE_PROCESSES {
        tracing::error!(
            pool_max,
            server_max,
            affordable_processes = affordable,
            "connection budget is oversubscribed: at this pool ceiling the server \
             can afford only this many processes fleet-wide. Lower \
             OXY_DATABASE_MAX_CONNECTIONS or raise the server's max_connections \
             (see internal-docs/adr-postgres-as-worker-queue.md §3)"
        );
    } else {
        tracing::info!(
            pool_max,
            server_max,
            affordable_processes = affordable,
            "connection budget"
        );
    }
}

fn spawn_pool_health_monitor(pool: sqlx::PgPool) {
    tokio::spawn(async move {
        let max = max_connections();
        log_connection_budget(&pool).await;
        let mut starved = false;
        loop {
            tokio::time::sleep(POOL_HEALTH_INTERVAL).await;
            let (size, idle) = (pool.size(), pool.num_idle());
            // Publish occupancy every pass, whether or not the probe succeeds:
            // a saturated pool is exactly the case where the probe fails, and
            // losing the numbers there would blind the metric precisely when it
            // matters.
            oxy_telemetry::metrics::sources::set_db_pool(
                u64::from(size),
                idle as u64,
                u64::from(max),
            );
            let probe_started = std::time::Instant::now();
            let probe = tokio::time::timeout(POOL_HEALTH_PROBE_TIMEOUT, pool.acquire()).await;
            // `timeout` (pool exhausted), `server_unavailable` (the pool had
            // room, so the wait was on Postgres) and `error` call for opposite
            // fixes, and sqlx hides which one this is: it retries a server
            // that is down or out of slots inside `acquire()`, so a server
            // outage times out exactly like a full pool. The pool's size once
            // the timeout has fired tells them apart — see `pool_probe`. The
            // values come from `record`, where the same set is seeded at zero.
            let failure_reason = pool_probe::failure_reason(&probe, pool.size(), max);
            // `probe` still holds the connection on success; it goes back to
            // the pool when the loop body ends.
            match failure_reason {
                None => {
                    oxy_telemetry::metrics::record::db_pool_probe(
                        probe_started.elapsed().as_secs_f64(),
                    );
                    oxy_telemetry::metrics::sources::set_db_pool_starved(false);
                    if starved {
                        tracing::info!(
                            pool_size = size,
                            pool_idle = idle,
                            pool_max = max,
                            "database connection pool recovered"
                        );
                        starved = false;
                    }
                }
                Some(reason) => {
                    // No duration sample on any failure path. A timeout was
                    // cancelled and never finished; an error finished without
                    // acquiring, so its elapsed time measures how fast the
                    // server said no, not how long a checkout takes. Recording
                    // either would put a number in the histogram that answers a
                    // different question from every other sample in it.
                    oxy_telemetry::metrics::record::db_pool_probe_failure(reason);
                    oxy_telemetry::metrics::sources::set_db_pool_starved(true);
                    // Resolve the cause BEFORE the macro: awaiting inside a
                    // `tracing` argument holds the macro's non-`Send` internals
                    // across the await and makes the whole task unspawnable.
                    let cause = diagnose_pool_starvation(&pool, reason).await;
                    tracing::error!(
                        pool_size = size,
                        pool_idle = idle,
                        pool_max = max,
                        acquire_timeout_secs = ACQUIRE_TIMEOUT.as_secs(),
                        // The metric's label, so a log line and a counter
                        // increment can be matched without re-deriving it.
                        oxy_reason = reason,
                        %cause,
                        "database connection pool is starved — requests will block up to \
                         acquire_timeout and then fail"
                    );
                    starved = true;
                }
            }
        }
    });
}

/// Recover the error the pool swallows, by opening ONE connection outside it.
///
/// A server that refuses it hands back the verbatim `FATAL`, which is what
/// tells you the ceiling is on the *other* side. A server that accepts it means
/// something different per `reason` — only an exhausted pool (`timeout`) makes
/// the pool the constraint — so that wording comes from
/// [`pool_probe::cause_when_server_accepts`] and stays consistent with the
/// label recorded beside it.
async fn diagnose_pool_starvation(pool: &sqlx::PgPool, reason: &str) -> String {
    use sqlx::Connection;
    let options = (*pool.connect_options()).clone();
    match tokio::time::timeout(
        POOL_HEALTH_PROBE_TIMEOUT,
        sqlx::PgConnection::connect_with(&options),
    )
    .await
    {
        Ok(Ok(conn)) => {
            let _ = conn.close().await;
            pool_probe::cause_when_server_accepts(reason).to_string()
        }
        Ok(Err(e)) => format!("the server refused a new connection: {e}"),
        Err(_) => format!(
            "the server did not answer a new connection within {}s",
            POOL_HEALTH_PROBE_TIMEOUT.as_secs()
        ),
    }
}

async fn connect_password() -> Result<DatabaseConnection, OxyError> {
    let url = std::env::var("OXY_DATABASE_URL").map_err(|_| {
        OxyError::Database(
            "OXY_DATABASE_URL environment variable is required. \
             Use 'oxy start' to automatically start PostgreSQL with Docker, \
             or set OXY_DATABASE_URL to your PostgreSQL connection string."
                .to_string(),
        )
    })?;

    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        tracing::error!(
            "OXY_DATABASE_URL must be a PostgreSQL connection string (starting with \
             'postgres://' or 'postgresql://'). Got: {}",
            url
        );
        return Err(OxyError::Database(
            "OXY_DATABASE_URL must be a PostgreSQL connection string (starting with \
             'postgres://' or 'postgresql://')"
                .to_string(),
        ));
    }

    tracing::debug!("Connecting to PostgreSQL from OXY_DATABASE_URL");

    let mut opt = ConnectOptions::new(url);
    opt.max_connections(max_connections())
        .min_connections(min_connections())
        .connect_timeout(CONNECT_TIMEOUT)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .idle_timeout(IDLE_TIMEOUT)
        .max_lifetime(MAX_LIFETIME)
        .sqlx_logging(false);

    connect_sea_orm_with_retry(opt).await
}

async fn connect_iam() -> Result<DatabaseConnection, OxyError> {
    let config = IamConfig::from_env()?;
    tracing::info!(
        host = %config.host,
        port = config.port,
        database = %config.database,
        user = %config.user,
        region = %config.region,
        "Connecting to PostgreSQL via RDS IAM auth"
    );

    let initial_token = iam::generate_auth_token(&config).await?;
    let connect_options = build_pg_connect_options(&config, &initial_token);
    let pool = connect_sqlx_with_retry(connect_options).await?;
    let db = SqlxPostgresConnector::from_sqlx_postgres_pool(pool);

    // Spawn the token-refresh loop. It holds a clone of the underlying
    // sqlx::PgPool (Arc-backed) and swaps fresh options onto it every
    // IAM_TOKEN_REFRESH_INTERVAL. Existing connections are unaffected by
    // set_connect_options; only new physical connections pick up the
    // refreshed token.
    let pool_clone = db.get_postgres_connection_pool().clone();
    tokio::spawn(refresh_iam_token_loop(pool_clone, config));

    Ok(db)
}

fn build_pg_connect_options(config: &IamConfig, token: &str) -> PgConnectOptions {
    let ssl_mode = match config.ssl_mode {
        SslMode::Require => PgSslMode::Require,
        SslMode::VerifyFull => PgSslMode::VerifyFull,
    };
    PgConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.user)
        .database(&config.database)
        .password(token)
        .ssl_mode(ssl_mode)
}

async fn refresh_iam_token_loop(pool: sqlx::PgPool, config: IamConfig) {
    let mut next_delay = IAM_TOKEN_REFRESH_INTERVAL;
    loop {
        tokio::time::sleep(next_delay).await;
        match iam::generate_auth_token(&config).await {
            Ok(token) => {
                pool.set_connect_options(build_pg_connect_options(&config, &token));
                tracing::info!("Refreshed RDS IAM auth token");
                next_delay = IAM_TOKEN_REFRESH_INTERVAL;
            }
            Err(e) => {
                // Existing pooled connections keep working; only *new*
                // physical connections will start failing once the currently
                // baked token ages past 15 min. Alert on this log line.
                tracing::error!(
                    error = %e,
                    retry_seconds = IAM_TOKEN_REFRESH_RETRY.as_secs(),
                    "Failed to refresh RDS IAM auth token; will retry shortly"
                );
                next_delay = IAM_TOKEN_REFRESH_RETRY;
            }
        }
    }
}

// ---- retry helpers ---------------------------------------------------------

// Postgres can accept TCP but reject the startup packet with "Connection reset
// by peer" for a short window after the container reports ready (Docker
// port-publisher + Postgres backend init race). Retry a handful of times with
// short backoff to absorb this.
const CONNECT_MAX_ATTEMPTS: u32 = 8;
const CONNECT_INITIAL_BACKOFF_MS: u64 = 250;
const CONNECT_MAX_BACKOFF_MS: u64 = 2000;

async fn connect_sea_orm_with_retry(opt: ConnectOptions) -> Result<DatabaseConnection, OxyError> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match Database::connect(opt.clone()).await {
            Ok(db) => return Ok(db),
            Err(e) if attempt < CONNECT_MAX_ATTEMPTS && is_transient_sea_orm_error(&e) => {
                sleep_backoff(attempt, &e.to_string()).await;
            }
            Err(e) => {
                tracing::error!(
                    "Failed to connect to PostgreSQL database after {} attempt(s): {}",
                    attempt,
                    e
                );
                return Err(OxyError::Database(e.to_string()));
            }
        }
    }
}

async fn connect_sqlx_with_retry(opt: PgConnectOptions) -> Result<sqlx::PgPool, OxyError> {
    let (max_conns, min_conns) = (max_connections(), min_connections());
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let pool_result = PgPoolOptions::new()
            .max_connections(max_conns)
            .min_connections(min_conns)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .idle_timeout(Some(IDLE_TIMEOUT))
            .max_lifetime(Some(MAX_LIFETIME))
            .connect_with(opt.clone())
            .await;
        match pool_result {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < CONNECT_MAX_ATTEMPTS && is_transient_sqlx_error(&e) => {
                sleep_backoff(attempt, &e.to_string()).await;
            }
            Err(e) => {
                tracing::error!(
                    "Failed to establish IAM-authenticated PostgreSQL pool after {} attempt(s): {}",
                    attempt,
                    e
                );
                return Err(OxyError::Database(e.to_string()));
            }
        }
    }
}

async fn sleep_backoff(attempt: u32, msg: &str) {
    let backoff_ms = std::cmp::min(
        CONNECT_INITIAL_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt - 1)),
        CONNECT_MAX_BACKOFF_MS,
    );
    tracing::warn!(
        "Transient PostgreSQL connect error (attempt {}/{}): {}. Retrying in {}ms",
        attempt,
        CONNECT_MAX_ATTEMPTS,
        msg,
        backoff_ms
    );
    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
}

// Classify errors that deserve a retry at startup. Prefer structural matching
// on `sea_orm::DbErr` / `sqlx::Error` variants — the string-based fallback is
// inherently version-sensitive (sea_orm, sqlx, or the OS may change error
// formatting) and is only there to catch the long tail.
fn is_transient_sea_orm_error(err: &sea_orm::DbErr) -> bool {
    use sea_orm::{DbErr, RuntimeErr};

    if let DbErr::Conn(RuntimeErr::SqlxError(sqlx_err)) = err
        && is_transient_sqlx_error(sqlx_err)
    {
        return true;
    }
    if matches!(err, DbErr::ConnectionAcquire(_)) {
        return true;
    }

    // Fallback: non-sqlx `Internal` errors and anything the structural path
    // missed. Substrings target stable English wording.
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("connection reset by peer")
        || msg.contains("connection refused")
        || msg.contains("connection closed")
        || msg.contains("broken pipe")
        || msg.contains("unexpected eof")
        || msg.contains("no connection could be made")
        // OS error codes are a last-resort fallback for non-English locales
        // where the substrings above don't match. Unix-specific; on Windows
        // the WSA codes (10054/10061) are rendered with the English substrings
        // above by the Rust standard library.
        || msg.contains("os error 54")    // macOS ECONNRESET
        || msg.contains("os error 104")   // Linux ECONNRESET
        || msg.contains("os error 111") // Linux ECONNREFUSED
}

fn is_transient_sqlx_error(err: &sqlx::Error) -> bool {
    use std::io::ErrorKind;
    match err {
        sqlx::Error::Io(io_err) => matches!(
            io_err.kind(),
            ErrorKind::ConnectionReset
                | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
                | ErrorKind::TimedOut
                | ErrorKind::NotConnected
        ),
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::WorkerCrashed => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn iam_config() -> IamConfig {
        IamConfig {
            host: "db.example.com".to_string(),
            port: 5432,
            database: "oxydb".to_string(),
            user: "oxy_app".to_string(),
            region: "us-west-2".to_string(),
            ssl_mode: SslMode::Require,
        }
    }

    #[test]
    fn build_pg_connect_options_sets_all_fields() {
        let cfg = iam_config();
        let opts = build_pg_connect_options(&cfg, "fake-iam-token");
        assert_eq!(opts.get_host(), "db.example.com");
        assert_eq!(opts.get_port(), 5432);
        assert_eq!(opts.get_username(), "oxy_app");
        assert_eq!(opts.get_database(), Some("oxydb"));
        assert!(matches!(opts.get_ssl_mode(), PgSslMode::Require));
    }

    #[test]
    fn build_pg_connect_options_propagates_verify_full() {
        let mut cfg = iam_config();
        cfg.ssl_mode = SslMode::VerifyFull;
        let opts = build_pg_connect_options(&cfg, "fake-iam-token");
        assert!(matches!(opts.get_ssl_mode(), PgSslMode::VerifyFull));
    }

    #[test]
    fn transient_sqlx_error_classifies_io_kinds() {
        for kind in [
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::TimedOut,
            io::ErrorKind::NotConnected,
        ] {
            let err = sqlx::Error::Io(io::Error::new(kind, "x"));
            assert!(
                is_transient_sqlx_error(&err),
                "expected {kind:?} to be transient"
            );
        }
    }

    #[test]
    fn transient_sqlx_error_rejects_permanent_io_kinds() {
        let err = sqlx::Error::Io(io::Error::new(io::ErrorKind::InvalidData, "x"));
        assert!(!is_transient_sqlx_error(&err));
    }

    #[test]
    fn transient_sqlx_error_classifies_pool_states() {
        assert!(is_transient_sqlx_error(&sqlx::Error::PoolTimedOut));
        assert!(is_transient_sqlx_error(&sqlx::Error::PoolClosed));
        assert!(is_transient_sqlx_error(&sqlx::Error::WorkerCrashed));
    }

    #[test]
    fn transient_sea_orm_error_classifies_structural_sqlx_wrap() {
        use sea_orm::{DbErr, RuntimeErr};
        let inner = sqlx::Error::Io(io::Error::new(io::ErrorKind::ConnectionReset, "x"));
        // SeaORM 2.0 holds the sqlx error in an `Arc` so `DbErr` can be cloned.
        let err = DbErr::Conn(RuntimeErr::SqlxError(std::sync::Arc::new(inner)));
        assert!(is_transient_sea_orm_error(&err));
    }

    #[test]
    fn transient_sea_orm_error_falls_back_to_string_match() {
        use sea_orm::DbErr;
        let err = DbErr::Custom("Connection reset by peer".to_string());
        assert!(is_transient_sea_orm_error(&err));

        let err = DbErr::Custom("Syntax error at position 1".to_string());
        assert!(!is_transient_sea_orm_error(&err));
    }

    #[test]
    fn resolve_pool_bound_uses_default_when_absent_or_invalid() {
        assert_eq!(resolve_pool_bound(None, 2), 2);
        assert_eq!(resolve_pool_bound(Some(""), 2), 2);
        assert_eq!(resolve_pool_bound(Some("garbage"), 2), 2);
        assert_eq!(resolve_pool_bound(Some("-1"), 2), 2);
    }

    #[test]
    fn resolve_pool_bound_parses_override() {
        assert_eq!(resolve_pool_bound(Some("10"), 2), 10);
        assert_eq!(resolve_pool_bound(Some("  40 "), 2), 40);
        assert_eq!(resolve_pool_bound(Some("0"), 2), 0);
    }

    #[test]
    fn min_is_clamped_to_max() {
        // A misconfigured floor above the ceiling must not exceed it — this is
        // the clamp `min_connections()` applies via `.min(max_connections())`.
        assert_eq!(
            resolve_pool_bound(Some("500"), DEFAULT_MIN_CONNECTIONS).min(40),
            40
        );
        // Within range, the override passes through.
        assert_eq!(
            resolve_pool_bound(Some("5"), DEFAULT_MIN_CONNECTIONS).min(40),
            5
        );
    }

    #[test]
    fn defaults_are_sane() {
        // Floor must be small (the whole point of the fix) and never exceed the
        // ceiling default.
        assert!(DEFAULT_MIN_CONNECTIONS <= DEFAULT_MAX_CONNECTIONS);
        assert!(DEFAULT_MIN_CONNECTIONS <= 5, "idle floor should stay small");
    }

    #[test]
    fn backoff_caps_at_ceiling() {
        // Replicates the backoff computation used in `sleep_backoff` so any
        // future change to exponent/ceiling is flagged by this test.
        fn compute(attempt: u32) -> u64 {
            std::cmp::min(
                CONNECT_INITIAL_BACKOFF_MS.saturating_mul(2u64.saturating_pow(attempt - 1)),
                CONNECT_MAX_BACKOFF_MS,
            )
        }
        assert_eq!(compute(1), 250);
        assert_eq!(compute(2), 500);
        assert_eq!(compute(3), 1000);
        assert_eq!(compute(4), 2000);
        assert_eq!(compute(5), 2000);
        assert_eq!(compute(10), 2000);
        // Far-out attempts must not overflow.
        assert_eq!(compute(100), 2000);
    }
}
