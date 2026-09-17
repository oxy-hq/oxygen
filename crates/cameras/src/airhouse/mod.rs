//! Airhouse-side writer + schema management for the camera fleet.
//!
//! Three concerns live here:
//!
//! 1. **Schema DDL** (`schema`) — `CREATE TABLE IF NOT EXISTS` for the
//!    per-tenant tables (`oxy_cam_events`, `oxy_cam_camera_health`,
//!    `oxy_cam_box_health`, `oxy_cam_compliance_reports`, `oxy_cam_device_logs`).
//! 2. **Connections** (`client`) — **persistent, per-tenant** pgwire clients.
//!    Cameras are tenant-based (each workspace is its own Airhouse tenant with
//!    its own minted credentials), so the [`client`] registry keeps one
//!    long-lived `tokio_postgres::Client` per `(workspace_id, role, purpose)`
//!    and reuses it across writes. This is the fix for the edge-ingest session
//!    churn that OOM'd Airhouse: a fresh connection per write spawned a fresh
//!    server-side DuckDB session each time. The ephemeral credential is still
//!    minted per-tenant via the SA broker (`SystemPurpose::EdgeIngest`).
//! 3. **Ensure cache** — a lazy "we've already created the tables in this
//!    tenant" set so the DDL only runs on the first ingest per
//!    (workspace_id, process lifetime).
//!
//! Service-layer entry points (`service::ingest`, `service::compliance`)
//! call `connect_and_ensure(workspace_id)` to get a ready-to-use, reused
//! [`TenantClient`], then build a multi-row INSERT via simple-query (DuckLake
//! doesn't speak prepared statements / `$N` placeholders).

pub mod client;
pub mod escape;
pub mod schema;

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use thiserror::Error;
use tokio::sync::RwLock;
use tokio_postgres::Client;
use uuid::Uuid;

use airhouse::{AirhouseConfig, SystemPurpose, UserRole};

pub use client::TenantClient;

/// Errors from the airhouse write path. Service-layer code maps these
/// into `ServiceError` for the route layer.
#[derive(Debug, Error)]
pub enum AirhouseError {
    #[error("airhouse is not configured (env vars unset)")]
    Disabled,
    #[error("broker mint failed: {0}")]
    Mint(String),
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("schema DDL failed: {0}")]
    Ddl(String),
    #[error("INSERT failed: {0}")]
    Insert(String),
}

/// Longest error text this hands back, in bytes.
///
/// Not a formatting nicety: the string it bounds ends up in a client-facing
/// JSON body (`routes::errors`), in a log line, and in a Sentry event message.
/// DuckLake can quote a large fragment of the failing statement, and none of
/// those three places want it unbounded. `db.query.text` is capped at 8 KB
/// elsewhere in the platform; an error message needs far less to be actionable.
const PG_ERROR_TEXT_MAX_BYTES: usize = 2048;

/// Render a `tokio_postgres::Error` with the server's reason attached.
///
/// Its `Display` is only the error KIND — `"db error"` — and DuckLake's message,
/// code, detail and hint sit behind `as_db_error()`. Formatting with `{e}` /
/// `to_string()` threw all of that away, with two effects:
///
/// * every airhouse failure on the edge write path read `INSERT failed: db
///   error` — 1,270 of them in prod in the 24h to 2026-09-15, all on
///   `POST /api/control/compliance-reports`, none saying why; and
/// * every "schema not provisioned yet → empty result" branch sniffs this string
///   for `does not exist`, which `"db error"` never contains, so those branches
///   could never fire and a fresh workspace got an error instead of the empty
///   result they exist to return.
///
/// **`DETAIL` is deliberately left out.** This string reaches a client: it
/// becomes `AirhouseError::{Insert,Connect}`, which `routes::errors` puts in the
/// response body verbatim (and logs, and therefore sends to Sentry as a message).
/// Postgres puts the offending row in `DETAIL` — `Key (…)=(…)` — so including it
/// would carry compliance-report and device-log VALUES out through a 502 body,
/// after `send_default_pii(false)` has had its say. The SQLSTATE plus the
/// server's message is what makes the failure actionable; `HINT` is the server's
/// own advice and carries no row data, so it stays.
///
/// A transport failure carries no `DbError`; for those the source chain is
/// appended so the io error survives. Same format as the DDL path used first.
///
/// (`crates/airhouse/src/connector/mod.rs` has a sibling of this for the
/// connector's own errors. They are deliberately not one function: that one is
/// an internal diagnostic and keeps the error KIND prefix to say whether the
/// failure was a connect or a query, which is noise in a client body, and it has
/// no reason to cap or drop `DETAIL`.)
pub(crate) fn pg_error_text(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        return truncate_on_char_boundary(format!(
            "[{code}] {msg}{hint}",
            code = db.code().code(),
            msg = db.message(),
            hint = db
                .hint()
                .map(|s| format!(" (hint: {s})"))
                .unwrap_or_default(),
        ));
    }
    let mut out = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(next) = source {
        out.push_str(": ");
        out.push_str(&next.to_string());
        source = next.source();
    }
    truncate_on_char_boundary(out)
}

/// Cut to [`PG_ERROR_TEXT_MAX_BYTES`] without splitting a UTF-8 character.
fn truncate_on_char_boundary(mut text: String) -> String {
    if text.len() <= PG_ERROR_TEXT_MAX_BYTES {
        return text;
    }
    // `*i + c.len_utf8() <= MAX`, not `*i < MAX`: the latter keeps a character
    // that merely STARTS below the cap, so a 3-byte character could push the
    // result two bytes past it.
    let cut = text
        .char_indices()
        .take_while(|(i, c)| *i + c.len_utf8() <= PG_ERROR_TEXT_MAX_BYTES)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    text.truncate(cut);
    text.push('…');
    text
}

/// Whether `err` says THIS table has not been created yet.
///
/// Every caller is a read whose contract is "a workspace that has never written
/// gets an empty result, not an error", and DuckLake ships no SQLSTATE precise
/// enough to match on — hence a substring test.
///
/// The table name is part of the match on purpose. The predicate this replaced
/// was `contains("Table") && contains("does not exist")`, which also matches
/// things that are real bugs and must not be swallowed: `Catalog Error: Table
/// Function with name … does not exist`, or a mistyped identifier in one of
/// these hand-built statements. Those used to surface as an airhouse error; with
/// [`pg_error_text`] now putting the server's real message in front of the
/// predicate, they would have started returning an empty list instead — which
/// reads as "no data" in the UI, and is the worst possible way to report a bug.
/// One case is deliberately left in: an error that quotes statement context
/// (`LINE 1: … FROM oxy_cam_compliance_reports …`) and whose own text says
/// "does not exist" — a catalog error on a COLUMN, say — still matches.
/// Closing it would mean pinning DuckDB's exact phrasing
/// (`Table with name {table} does not exist`), and the two failure directions
/// are not symmetric: a false positive here costs one reader an empty list,
/// while a false negative puts every fresh workspace back to an error instead
/// of the empty result — the bug this whole path exists to prevent. The looser
/// form fails in the cheaper direction.
pub(crate) fn is_missing_table(err: &str, table: &str) -> bool {
    err.contains("does not exist") && err.contains(table)
}

// ── Tunables (env-configurable) ─────────────────────────────────────────────
//
// NOTE: per-connection tunables (`ingest_ttl`, `read_ttl`,
// `max_reconnect_attempts`, the TLS opt-out) are read when a tenant's
// persistent connection is FIRST opened and captured for that connection's
// lifetime (including its background reconnects). Changing the env later only
// affects tenants connected afterward, not already-open ones. `insert_chunk_rows`
// is read per write, so it takes effect immediately, and `connect_timeout` is
// read per connect attempt, so a change applies to the next reconnect without a
// restart.

/// Default credential TTL for the ingest (Writer) + DDL (Admin) paths.
/// Override with `OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS`.
const DEFAULT_INGEST_TTL_SECS: u64 = 15 * 60;

/// Default credential TTL for the read (Reader) path.
/// Override with `OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS`.
const DEFAULT_READ_TTL_SECS: u64 = 5 * 60;

/// Default max rows per INSERT statement; large ingest batches are split into
/// chunks of this size to keep SQL strings — and the server-side materialised
/// row group — bounded. Override with `OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS`.
const DEFAULT_INSERT_CHUNK_ROWS: usize = 500;

/// Default consecutive unhealthy reconnect cycles a persistent tenant
/// connection tolerates before its background driver gives up, evicts the
/// tenant from the registry, and exits — so a deprovisioned / long-dead tenant
/// doesn't keep a reconnect task and a held server-side DuckDB session alive
/// forever. Counts both connect/auth failures (can't connect at all) and rapid
/// flaps (connect succeeds but the session drops almost immediately); a
/// connection that stays up resets the count. The next request re-establishes
/// lazily. Each failed cycle is the backoff sleep PLUS the attempt itself, and
/// an attempt that hangs runs to `connect_timeout()` (30s): at the 30s backoff
/// cap, 20 is ~10 minutes of failures that return at once, and ~20 minutes when
/// every attempt hangs.
/// Override with `OXY_CAMERAS_AIRHOUSE_MAX_RECONNECT_ATTEMPTS`; `0` = forever.
const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 20;

/// Default bound on ONE connect attempt, end to end: TCP, TLS and the pgwire
/// startup/auth exchange. `tokio_postgres::Config::connect_timeout` does not
/// do this — it bounds only the TCP connect, and through HAProxy that always
/// succeeds at once. Unbounded, a handshake HAProxy holds open (a backend that
/// went away mid-rollout) stalls until its `timeout server`, 1h, and a stalled
/// reconnect driver still reads as live, so every write fails with
/// `connection closed` meanwhile (prod 2026-09-17, airhouse 0.1.50 rollout).
/// Generous against a cold DuckDB session, and 120x shorter than that stall.
/// Override with `OXY_CAMERAS_AIRHOUSE_CONNECT_TIMEOUT_SECS`.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Credential lifetime for the ingest / DDL path. Plenty of headroom for an
/// ingest burst; well under the airhouse max (`SYSTEM_MAX_TTL_SECS = 86400`).
/// Note this bounds the *credential*, not the connection — a persistent
/// connection is reused across TTLs and only re-mints on reconnect.
pub fn ingest_ttl() -> Duration {
    env_duration_secs(
        "OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS",
        DEFAULT_INGEST_TTL_SECS,
    )
}

/// Credential lifetime for the read path.
pub fn read_ttl() -> Duration {
    env_duration_secs("OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS", DEFAULT_READ_TTL_SECS)
}

/// Max rows per INSERT statement for the ingest writers.
pub fn insert_chunk_rows() -> usize {
    env_usize(
        "OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS",
        DEFAULT_INSERT_CHUNK_ROWS,
    )
}

/// Consecutive reconnect failures a persistent tenant connection tolerates
/// before giving up + evicting (see [`DEFAULT_MAX_RECONNECT_ATTEMPTS`]). `0`
/// (explicitly set) means retry forever; garbage / unset → default.
pub fn max_reconnect_attempts() -> u32 {
    std::env::var("OXY_CAMERAS_AIRHOUSE_MAX_RECONNECT_ATTEMPTS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_RECONNECT_ATTEMPTS)
}

/// Bound on one whole connect attempt (see [`DEFAULT_CONNECT_TIMEOUT_SECS`]).
/// Read per attempt, so a change applies to the next reconnect.
pub fn connect_timeout() -> Duration {
    env_duration_secs(
        "OXY_CAMERAS_AIRHOUSE_CONNECT_TIMEOUT_SECS",
        DEFAULT_CONNECT_TIMEOUT_SECS,
    )
}

/// Parse a positive-integer seconds env var into a `Duration`, falling back
/// to `default_secs` when unset, empty, unparseable, or zero.
fn env_duration_secs(var: &str, default_secs: u64) -> Duration {
    Duration::from_secs(env_u64(var, default_secs))
}

fn env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

fn env_usize(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// In-process record of which workspace tenants have had the
/// camera-fleet DDL applied this process lifetime. Keyed by
/// `workspace_id`. Eviction happens only on process restart; that's
/// fine because the DDL is idempotent (`IF NOT EXISTS`).
fn ensured() -> &'static RwLock<HashSet<Uuid>> {
    static C: OnceLock<RwLock<HashSet<Uuid>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Ensure the camera-fleet DDL has been applied to this workspace's tenant at
/// least once this process, then return the **reused** Writer ingest client.
///
/// **Where the DDL actually runs:** in production, the
/// `airhouse::register_post_provision_hook` registered at app startup
/// (see `crates/app/src/cli/commands/serve.rs`) fires this same
/// `ensure_schema` from `TenantProvisioner::provision`, so by the time
/// any edge box hits us the tables already exist. This function's
/// `ensured()` short-circuit + idempotent `CREATE TABLE IF NOT EXISTS`
/// then makes the ingest path's contribution effectively zero on the
/// hot path.
///
/// **The fallback case:** kept for safety in two scenarios:
///   1. Tenants provisioned before the hook was registered (one-time —
///      solved automatically the first time ingest runs against any such
///      tenant).
///   2. A post-provision hook failure (logged but non-fatal — see
///      `airhouse::post_provision::invoke_all`).
///
/// **Role split**: DDL runs with **Admin** (Writer cannot `CREATE TABLE` in
/// DuckLake — fails with `42501 Permission denied`). INSERTs run with
/// **Writer** (least privilege for ingest). The two roles are separate
/// persistent connections (and separate broker cache keys), so the Writer
/// reuse doesn't churn the Admin path.
pub async fn connect_and_ensure(workspace_id: Uuid) -> Result<Arc<TenantClient>, AirhouseError> {
    // Fast path: schema already ensured for this workspace this process.
    let already_ensured = ensured().read().await.contains(&workspace_id);
    if !already_ensured {
        ensure_schema(workspace_id).await?;
        ensured().write().await.insert(workspace_id);
    }
    client::tenant_client(
        workspace_id,
        UserRole::Writer,
        SystemPurpose::EdgeIngest,
        ingest_ttl(),
        client::Pool::Serving,
    )
    .await
}

/// Read-side companion to [`connect_and_ensure`]. Used by service-layer
/// SELECTs (e.g. the Compliance tab pulling reports for a camera).
///
/// Differences from the write path:
///   - Audited as [`SystemPurpose::ComplianceReportsRead`] so the audit log
///     can separate UI traffic from bulk edge ingest.
///   - Mints with [`UserRole::Reader`] — read-only, can SELECT but not
///     INSERT / DDL.
///   - Shorter credential TTL ([`read_ttl`]).
///   - Does NOT run schema DDL. If the table doesn't exist (no edge box ever
///     wrote to it), the SELECT just returns an empty set or errors as
///     `undefined_table`, both of which are accurate.
///   - Routed to the **analytics** DP pool ([`client::Pool::Analytics`]) when
///     one is configured. Every camera dashboard read — health-summary,
///     compliance, cost, logs, rollup — funnels through here, so this is the
///     one place that decides it.
///
/// Why the analytics pool: these are UI SELECTs that scan DuckLake, and they
/// were sharing the serving DP with edge ingest. That coupled them in both
/// directions — dashboard scans competing with hot writes, and, worse, a
/// write-path fault taking the dashboard down with it. Each DP has its own
/// circuit breaker: on 2026-07-15 rejected `oxy_cam_*` writes tripped the
/// serving DP's breaker, and because reads lived on that same pool the
/// dashboard went down with the ingest. On the analytics pool a write-path
/// fault costs writes only.
///
/// Falls back to the serving endpoint when `AIRHOUSE_ANALYTICS_WIRE_HOST` is
/// unset, so deployments without a separate analytics pool are unchanged.
///
/// Like the write path, the underlying connection is **persistent and reused**
/// per tenant rather than opened per request.
pub async fn connect_for_reads(workspace_id: Uuid) -> Result<Arc<TenantClient>, AirhouseError> {
    client::tenant_client(
        workspace_id,
        UserRole::Reader,
        SystemPurpose::ComplianceReportsRead,
        read_ttl(),
        client::Pool::Analytics,
    )
    .await
}

/// One-shot connection for the log-retention sweep.
///
/// Retention runs infrequently (hourly+, `OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS`)
/// and issues large `DELETE`s. Deliberately **not** routed through the
/// persistent `(Writer, EdgeIngest)` ingest connection: the server-side DuckDB
/// session executes serially, so a slow retention `DELETE` sharing that
/// connection could head-of-line-block live edge ingest for the tenant. A
/// dedicated one-shot connection (audited under [`SystemPurpose::Scheduler`])
/// isolates it; being one-shot is also cheaper than holding a second persistent
/// session per tenant for an operation that runs a couple times a day. No DDL —
/// a missing table is handled by the caller as "nothing to retain".
pub async fn connect_for_retention(workspace_id: Uuid) -> Result<Client, AirhouseError> {
    connect(
        workspace_id,
        UserRole::Writer,
        SystemPurpose::Scheduler,
        ingest_ttl(),
    )
    .await
}

/// One-shot Admin-credentialled DDL run. The Admin client is created, used
/// for the `CREATE TABLE IF NOT EXISTS` statements, and dropped immediately —
/// no long-lived Admin handle in process memory. DDL is rare (gated by
/// `ensured()`), so this path intentionally does **not** join the persistent
/// registry.
pub async fn ensure_schema(workspace_id: Uuid) -> Result<(), AirhouseError> {
    let admin_client = connect(
        workspace_id,
        UserRole::Admin,
        SystemPurpose::EdgeIngest,
        ingest_ttl(),
    )
    .await?;
    schema::ensure(&admin_client).await?;
    drop(admin_client);
    Ok(())
}

/// Open a **fresh, one-shot** tokio-postgres client to the workspace's
/// Airhouse tenant at the requested role. The caller drops the client when
/// done; the detached connection task exits with it.
///
/// This is only for the short-lived Admin DDL path. High-frequency ingest /
/// read paths must use [`connect_and_ensure`] / [`connect_for_reads`], which
/// return a reused persistent connection instead.
pub async fn connect(
    workspace_id: Uuid,
    role: UserRole,
    purpose: SystemPurpose,
    ttl: Duration,
) -> Result<Client, AirhouseError> {
    let cfg = match AirhouseConfig::from_env() {
        AirhouseConfig::Enabled(c) => c,
        _ => return Err(AirhouseError::Disabled),
    };

    let broker = airhouse::token_broker().ok_or(AirhouseError::Disabled)?;
    let cred = broker
        .mint_for_system(workspace_id, purpose, role, ttl)
        .await
        .map_err(|e| AirhouseError::Mint(e.to_string()))?;

    let pg = client::make_pg_config(
        &cfg.wire_host,
        cfg.wire_port,
        &cred.username,
        &cred.password,
        &cred.tenant,
    );
    let (client, conn_fut) = client::try_connect(&pg, client::insecure_from_env())
        .await
        .map_err(|e| AirhouseError::Connect(e.text()))?;

    // Drive the pgwire connection on a detached task. When the Client drops,
    // this future completes and the task exits.
    tokio::spawn(async move {
        if let Err(e) = conn_fut.await {
            tracing::warn!("cameras airhouse one-shot connection ended: {e}");
        }
    });

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Env-var parsing must serialize: these touch the process-wide env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[serial]
    #[test]
    fn ttls_and_chunk_fall_back_to_defaults_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS");
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS");
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS");
        }
        assert_eq!(ingest_ttl(), Duration::from_secs(DEFAULT_INGEST_TTL_SECS));
        assert_eq!(read_ttl(), Duration::from_secs(DEFAULT_READ_TTL_SECS));
        assert_eq!(insert_chunk_rows(), DEFAULT_INSERT_CHUNK_ROWS);
    }

    #[serial]
    #[test]
    fn env_overrides_are_honored() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::set_var("OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS", "120");
            std::env::set_var("OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS", "30");
            std::env::set_var("OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS", "1000");
        }
        assert_eq!(ingest_ttl(), Duration::from_secs(120));
        assert_eq!(read_ttl(), Duration::from_secs(30));
        assert_eq!(insert_chunk_rows(), 1000);
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS");
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS");
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS");
        }
    }

    #[serial]
    #[test]
    fn max_reconnect_attempts_parsing() {
        let _g = ENV_LOCK.lock().unwrap();
        let var = "OXY_CAMERAS_AIRHOUSE_MAX_RECONNECT_ATTEMPTS";
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::remove_var(var) };
        assert_eq!(max_reconnect_attempts(), DEFAULT_MAX_RECONNECT_ATTEMPTS);
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::set_var(var, "5") };
        assert_eq!(max_reconnect_attempts(), 5);
        // Unlike the other knobs, 0 is meaningful here ("retry forever") and
        // must NOT fall back to the default.
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::set_var(var, "0") };
        assert_eq!(max_reconnect_attempts(), 0);
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::set_var(var, "garbage") };
        assert_eq!(max_reconnect_attempts(), DEFAULT_MAX_RECONNECT_ATTEMPTS);
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::remove_var(var) };
    }

    #[serial]
    #[test]
    fn zero_and_garbage_values_fall_back_to_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::set_var("OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS", "0");
            std::env::set_var("OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS", "not-a-number");
        }
        assert_eq!(ingest_ttl(), Duration::from_secs(DEFAULT_INGEST_TTL_SECS));
        assert_eq!(insert_chunk_rows(), DEFAULT_INSERT_CHUNK_ROWS);
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS");
            std::env::remove_var("OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS");
        }
    }

    /// The cap exists because this text reaches a client body and a Sentry
    /// message; a DuckLake error can quote a large slice of the statement.
    #[test]
    fn long_error_text_is_capped() {
        let long = "x".repeat(PG_ERROR_TEXT_MAX_BYTES * 3);
        let out = truncate_on_char_boundary(long);
        assert!(out.len() <= PG_ERROR_TEXT_MAX_BYTES + '…'.len_utf8());
        assert!(out.ends_with('…'));
    }

    #[test]
    fn short_error_text_is_untouched() {
        let short = "[XX000] Catalog Error: Table with name oxy_cam_events does not exist!";
        assert_eq!(truncate_on_char_boundary(short.to_string()), short);
    }

    /// Multi-byte characters must not be split — a truncated UTF-8 sequence
    /// would make the whole JSON body unserialisable.
    #[test]
    fn truncation_lands_on_a_char_boundary() {
        // 2-byte and 3-byte characters: neither may be split, and neither may
        // push the result past the cap.
        for filler in ["é", "€"] {
            let out = truncate_on_char_boundary(filler.repeat(PG_ERROR_TEXT_MAX_BYTES));
            assert!(
                out.len() <= PG_ERROR_TEXT_MAX_BYTES + '…'.len_utf8(),
                "{filler}: {} bytes",
                out.len()
            );
            assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        }
    }

    #[test]
    fn missing_table_matches_only_that_table() {
        let err = "[XX000] Catalog Error: Table with name oxy_cam_device_logs does not exist!";
        assert!(is_missing_table(err, "oxy_cam_device_logs"));
        assert!(
            !is_missing_table(err, "oxy_cam_compliance_reports"),
            "a different table's absence is not this reader's empty-result case"
        );
    }

    /// The failures the old `contains(\"Table\") && contains(\"does not exist\")`
    /// predicate would have swallowed as "no data" once `pg_error_text` started
    /// putting the server's real message in front of it.
    #[test]
    fn missing_table_rejects_real_errors() {
        assert!(!is_missing_table(
            "[XX000] Catalog Error: Table Function with name read_parquet does not exist!",
            "oxy_cam_compliance_reports"
        ));
        assert!(!is_missing_table(
            "[XX000] Binder Error: Referenced column \"tokens_uesd\" not found in FROM clause",
            "oxy_cam_compliance_reports"
        ));
        assert!(!is_missing_table(
            "[53300] too many connections for tenant",
            "oxy_cam_compliance_reports"
        ));
    }
}
