//! Env-driven Airhouse runtime config + factory helpers.
//!
//! Loads the four `AIRHOUSE_*` env vars into a [`AirhouseConfig`] tri-state
//! (Enabled / Disabled / Misconfigured) and provides factory functions for
//! constructing a [`crate::TenantProvisioner`] / the SA-backed
//! [`crate::AirhouseTokenBroker`] when the integration is enabled.

use std::sync::OnceLock;
use uuid::Uuid;

use crate::admin::AirhouseAdminClient;
use crate::broker::AirhouseTokenBroker;
use crate::provisioner::TenantProvisioner;

// ── Env var names ─────────────────────────────────────────────────────────────

pub const AIRHOUSE_BASE_URL_VAR: &str = "AIRHOUSE_BASE_URL";
pub const AIRHOUSE_ADMIN_TOKEN_VAR: &str = "AIRHOUSE_ADMIN_TOKEN";
pub const AIRHOUSE_WIRE_HOST_VAR: &str = "AIRHOUSE_WIRE_HOST";
pub const AIRHOUSE_WIRE_PORT_VAR: &str = "AIRHOUSE_WIRE_PORT";

/// Optional dedicated analytics/read DP endpoint. When set, query workloads
/// (analytics agent, Data-App, SQL-IDE — everything routed through
/// `build_airhouse_connector`) connect here instead of the main serving
/// endpoint, isolating heavy OLAP from the latency-sensitive serving/ingest DP.
/// Ingest (airway) and cameras keep using the main wire endpoint.
pub const AIRHOUSE_ANALYTICS_WIRE_HOST_VAR: &str = "AIRHOUSE_ANALYTICS_WIRE_HOST";
pub const AIRHOUSE_ANALYTICS_WIRE_PORT_VAR: &str = "AIRHOUSE_ANALYTICS_WIRE_PORT";

/// Optional: the name of the tenants' DuckLake catalog on this deployment
/// (Airhouse's default is `lake`). Nothing in the Admin API reports it, so a
/// workspace preview accepts a catalog-qualified name (`lake.S.t`) only when
/// this names the catalog; unset, every such name is refused (fail closed).
pub const AIRHOUSE_CATALOG_VAR: &str = "AIRHOUSE_CATALOG";

/// [`AIRHOUSE_CATALOG_VAR`], trimmed; `None` when unset or empty. Read fresh
/// from the environment, like [`analytics_wire_endpoint`].
pub fn ducklake_catalog() -> Option<String> {
    std::env::var(AIRHOUSE_CATALOG_VAR)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ── Local-mode constants ──────────────────────────────────────────────────────

/// Well-known nil-UUID organization id used in local mode. Mirrors the
/// frontend `LOCAL_ORG_ID` constant. The local-mode startup seeder creates a
/// row at this id so `airhouse_tenants` / `airhouse_users` FKs are satisfied
/// and the provisioner's membership check passes for the local guest user.
pub const LOCAL_ORG_ID: Uuid = Uuid::nil();

const DEFAULT_WIRE_PORT: u16 = 5445;

/// Well-known coordinates of the local Airhouse stack defined in
/// `docker-compose.airhouse.yml`. Used by [`autodetect_local_airhouse`] to
/// wire `--local` mode to a running compose stack without manual env setup.
pub const LOCAL_DEFAULT_BASE_URL: &str = "http://localhost:8080";
pub const LOCAL_DEFAULT_ADMIN_TOKEN: &str = "airhouse-local-token";
pub const LOCAL_DEFAULT_WIRE_HOST: &str = "localhost";
pub const LOCAL_DEFAULT_WIRE_PORT: &str = "5445";

/// Names of all required Airhouse env vars. Used in error messages.
pub const REQUIRED_VARS: &[&str] = &[
    AIRHOUSE_BASE_URL_VAR,
    AIRHOUSE_ADMIN_TOKEN_VAR,
    AIRHOUSE_WIRE_HOST_VAR,
    AIRHOUSE_WIRE_PORT_VAR,
];

// ── Runtime config ────────────────────────────────────────────────────────────

/// Required fields when the Airhouse integration is active.
///
/// S3 bucket + prefix are no longer configured here — Airhouse owns storage
/// internally (see `[storage]` in `airhouse.toml`). The bucket and prefix
/// values returned by the Admin API are persisted on the local
/// `airhouse_tenants` row from the response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirhouseRuntimeConfig {
    pub base_url: String,
    pub admin_token: String,
    pub wire_host: String,
    pub wire_port: u16,
}

/// Three-state config for the Airhouse integration.
///
/// - `Enabled` — all required vars present and non-empty.
/// - `Disabled` — none of the required vars are set; integration is off.
/// - `Misconfigured` — at least one required var is set but the full set is
///   incomplete. Callers should surface this as a startup error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AirhouseConfig {
    Enabled(AirhouseRuntimeConfig),
    Disabled,
    Misconfigured,
}

static CACHED_CONFIG: OnceLock<AirhouseConfig> = OnceLock::new();

impl AirhouseConfig {
    /// Load from environment. Re-reads env vars every call; safe for tests.
    pub fn from_env() -> Self {
        let base_url = std::env::var(AIRHOUSE_BASE_URL_VAR).ok();
        let admin_token = std::env::var(AIRHOUSE_ADMIN_TOKEN_VAR).ok();
        let wire_host = std::env::var(AIRHOUSE_WIRE_HOST_VAR).ok();
        let wire_port_raw = std::env::var(AIRHOUSE_WIRE_PORT_VAR).ok();

        let any_set = [
            base_url.as_deref(),
            admin_token.as_deref(),
            wire_host.as_deref(),
            wire_port_raw.as_deref(),
        ]
        .iter()
        .any(|v| v.is_some_and(|s| !s.is_empty()));

        if !any_set {
            return Self::Disabled;
        }

        let (Some(base_url), Some(admin_token), Some(wire_host)) = (
            base_url.filter(|s| !s.is_empty()),
            admin_token.filter(|s| !s.is_empty()),
            wire_host.filter(|s| !s.is_empty()),
        ) else {
            return Self::Misconfigured;
        };

        let wire_port = wire_port_raw
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse::<u16>().unwrap_or_else(|_| {
                    tracing::warn!(
                        "{AIRHOUSE_WIRE_PORT_VAR} value {s:?} is not a valid port number; \
                         falling back to default {DEFAULT_WIRE_PORT}"
                    );
                    DEFAULT_WIRE_PORT
                })
            })
            .unwrap_or(DEFAULT_WIRE_PORT);

        let base_url = base_url.trim_end_matches('/').to_string();

        Self::Enabled(AirhouseRuntimeConfig {
            base_url,
            admin_token,
            wire_host,
            wire_port,
        })
    }

    /// Cached form of `from_env` — reads env vars once on the first call.
    pub fn cached() -> &'static AirhouseConfig {
        CACHED_CONFIG.get_or_init(Self::from_env)
    }

    pub fn as_runtime(&self) -> Option<&AirhouseRuntimeConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_runtime(self) -> Option<AirhouseRuntimeConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            _ => None,
        }
    }
}

// ── Wire endpoint ─────────────────────────────────────────────────────────────

/// User-facing wire-protocol coordinates exposed by the deployment.
#[derive(Debug, Clone)]
pub struct WireEndpoint {
    pub host: String,
    pub port: u16,
}

/// Resolve the user-facing wire-protocol connection coordinates from config.
pub fn wire_endpoint() -> Option<WireEndpoint> {
    let cfg = AirhouseConfig::cached().as_runtime()?;
    Some(WireEndpoint {
        host: cfg.wire_host.clone(),
        port: cfg.wire_port,
    })
}

/// Resolve the optional dedicated analytics DP endpoint.
///
/// Returns `Some` only when `AIRHOUSE_ANALYTICS_WIRE_HOST` is set to a non-empty
/// value; the port falls back to `AIRHOUSE_ANALYTICS_WIRE_PORT`, then the main
/// `wire_endpoint()` port, then the default. Returns `None` when unset, so
/// callers transparently keep using the main `wire_endpoint()` (no behaviour
/// change in deployments without a separate analytics pool). Read fresh from the
/// environment (not cached) so it composes with the test-time env helpers.
pub fn analytics_wire_endpoint() -> Option<WireEndpoint> {
    let host = std::env::var(AIRHOUSE_ANALYTICS_WIRE_HOST_VAR)
        .ok()
        .filter(|s| !s.is_empty())?;
    let port = std::env::var(AIRHOUSE_ANALYTICS_WIRE_PORT_VAR)
        .ok()
        .filter(|s| !s.is_empty())
        .and_then(|s| match s.parse::<u16>() {
            Ok(p) => Some(p),
            Err(_) => {
                tracing::warn!(
                    "{AIRHOUSE_ANALYTICS_WIRE_PORT_VAR} value {s:?} is not a valid port number; \
                     falling back to the main wire port"
                );
                None
            }
        })
        .or_else(|| wire_endpoint().map(|e| e.port))
        .unwrap_or(DEFAULT_WIRE_PORT);
    Some(WireEndpoint { host, port })
}

// ── Local-mode autodetect ─────────────────────────────────────────────────────

/// In `--local` mode, wire Oxy to a running local Airhouse stack
/// (`docker-compose.airhouse.yml`) without manual env configuration.
///
/// If none of the `AIRHOUSE_*` vars are set, probe the compose stack's
/// control-plane health endpoint. When it responds, inject the well-known
/// compose defaults into the process env so the per-workspace provision flow
/// works out of the box. Returns `true` when defaults were injected.
///
/// No-op (returns `false`) when:
/// - any `AIRHOUSE_*` var is already set — a deliberate (or partial →
///   `Misconfigured`) setup must not be masked by autodetected defaults; or
/// - the local stack is not reachable — the integration then stays
///   `Disabled` with its normal "not configured" message.
///
/// Must be called once at local-mode startup, before the first
/// [`AirhouseConfig::cached`] read.
pub async fn autodetect_local_airhouse() -> bool {
    let any_set = REQUIRED_VARS
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
    if any_set {
        return false;
    }

    let health_url = format!("{LOCAL_DEFAULT_BASE_URL}/healthz");
    let reachable = match reqwest::Client::new()
        .get(&health_url)
        .timeout(std::time::Duration::from_millis(1500))
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    };
    if !reachable {
        tracing::info!(
            "local Airhouse not detected at {LOCAL_DEFAULT_BASE_URL} — integration \
             stays disabled. Run `docker compose -f docker-compose.airhouse.yml up -d` \
             to enable per-workspace provisioning in local mode."
        );
        return false;
    }

    // Safety: called once at local-mode startup before the HTTP server
    // accepts requests and before the first `AirhouseConfig::cached()` read.
    // Every reader runs strictly after this write on the same task, so no
    // data race can occur (mirrors `oxy start`'s OXY_DATABASE_URL injection).
    unsafe {
        std::env::set_var(AIRHOUSE_BASE_URL_VAR, LOCAL_DEFAULT_BASE_URL);
        std::env::set_var(AIRHOUSE_ADMIN_TOKEN_VAR, LOCAL_DEFAULT_ADMIN_TOKEN);
        std::env::set_var(AIRHOUSE_WIRE_HOST_VAR, LOCAL_DEFAULT_WIRE_HOST);
        std::env::set_var(AIRHOUSE_WIRE_PORT_VAR, LOCAL_DEFAULT_WIRE_PORT);
    }
    tracing::info!(
        base_url = LOCAL_DEFAULT_BASE_URL,
        "detected local Airhouse stack — autoconfigured AIRHOUSE_* for local mode"
    );
    true
}

// ── Factory functions ─────────────────────────────────────────────────────────

/// Build a bare [`AirhouseAdminClient`] when the integration is enabled.
/// Unlike [`provisioner_for`] / [`token_broker`] it carries no extra state —
/// it's for stateless one-shot reads (e.g. the server version from
/// `/health`). Returns `None` when the integration is disabled or
/// misconfigured; callers should surface that as 503 "not configured".
pub fn admin_client() -> Option<AirhouseAdminClient> {
    let cfg = AirhouseConfig::cached().as_runtime()?;
    Some(AirhouseAdminClient::new(
        cfg.base_url.clone(),
        cfg.admin_token.clone(),
    ))
}

/// Build a `TenantProvisioner` for the given DB connection if Airhouse is enabled.
/// Returns `None` when the integration is disabled or misconfigured — call sites
/// should treat that as "skip silently".
pub fn provisioner_for(db: sea_orm::DatabaseConnection) -> Option<TenantProvisioner> {
    let cfg = AirhouseConfig::cached().as_runtime()?.clone();
    let client = AirhouseAdminClient::new(cfg.base_url.clone(), cfg.admin_token.clone());
    Some(TenantProvisioner::new(db, client))
}

/// Process-wide [`AirhouseTokenBroker`]. The broker holds an in-memory
/// credential cache keyed by `(workspace_id, subject, role)`; sharing one
/// instance across the app is what makes the cache work — fresh instances
/// would mint per call. Returns `None` when the integration is disabled
/// or misconfigured.
pub fn token_broker() -> Option<&'static AirhouseTokenBroker> {
    static BROKER: OnceLock<AirhouseTokenBroker> = OnceLock::new();
    let cfg = AirhouseConfig::cached().as_runtime()?;
    Some(BROKER.get_or_init(|| {
        AirhouseTokenBroker::new(AirhouseAdminClient::new(
            cfg.base_url.clone(),
            cfg.admin_token.clone(),
        ))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap();
        for k in [
            AIRHOUSE_BASE_URL_VAR,
            AIRHOUSE_ADMIN_TOKEN_VAR,
            AIRHOUSE_WIRE_HOST_VAR,
            AIRHOUSE_WIRE_PORT_VAR,
            AIRHOUSE_ANALYTICS_WIRE_HOST_VAR,
            AIRHOUSE_ANALYTICS_WIRE_PORT_VAR,
        ] {
            unsafe { std::env::remove_var(k) };
        }
        f();
    }

    #[test]
    fn disabled_when_nothing_set() {
        with_clean_env(|| {
            assert_eq!(AirhouseConfig::from_env(), AirhouseConfig::Disabled);
        });
    }

    #[test]
    fn misconfigured_when_partial() {
        with_clean_env(|| {
            unsafe { std::env::set_var(AIRHOUSE_BASE_URL_VAR, "http://airhouse:8080") };
            // admin_token + wire_host missing
            assert_eq!(AirhouseConfig::from_env(), AirhouseConfig::Misconfigured);
        });
    }

    #[test]
    fn enabled_with_defaults_when_all_required_present() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(AIRHOUSE_BASE_URL_VAR, "http://airhouse:8080/");
                std::env::set_var(AIRHOUSE_ADMIN_TOKEN_VAR, "secret");
                std::env::set_var(AIRHOUSE_WIRE_HOST_VAR, "airhouse");
            }
            let cfg = AirhouseConfig::from_env()
                .into_runtime()
                .expect("should be Enabled");
            assert_eq!(cfg.base_url, "http://airhouse:8080"); // trailing slash trimmed
            assert_eq!(cfg.wire_port, DEFAULT_WIRE_PORT);
        });
    }

    #[tokio::test]
    async fn autodetect_noop_when_a_var_already_set() {
        let _g = ENV_LOCK.lock().unwrap();
        for k in REQUIRED_VARS {
            unsafe { std::env::remove_var(k) };
        }
        // A deliberate (or partial) config must not be masked by autodetected
        // defaults — even one var present short-circuits before any probe.
        unsafe { std::env::set_var(AIRHOUSE_BASE_URL_VAR, "http://my-airhouse:9999") };

        assert!(!autodetect_local_airhouse().await);
        assert_eq!(
            std::env::var(AIRHOUSE_BASE_URL_VAR).unwrap(),
            "http://my-airhouse:9999"
        );
        assert!(std::env::var(AIRHOUSE_ADMIN_TOKEN_VAR).is_err());

        unsafe { std::env::remove_var(AIRHOUSE_BASE_URL_VAR) };
    }

    #[test]
    fn analytics_endpoint_opt_in() {
        with_clean_env(|| {
            // Unset → None: callers transparently keep using wire_endpoint(),
            // so deployments without a separate analytics pool are unaffected.
            assert!(analytics_wire_endpoint().is_none());

            // Host (+ explicit port) set → query workloads route here.
            unsafe {
                std::env::set_var(
                    AIRHOUSE_ANALYTICS_WIRE_HOST_VAR,
                    "airhouse-analytics-haproxy",
                );
                std::env::set_var(AIRHOUSE_ANALYTICS_WIRE_PORT_VAR, "5445");
            }
            let ep = analytics_wire_endpoint().expect("set → Some");
            assert_eq!(ep.host, "airhouse-analytics-haproxy");
            assert_eq!(ep.port, 5445);

            // Empty host is treated as unset.
            unsafe { std::env::set_var(AIRHOUSE_ANALYTICS_WIRE_HOST_VAR, "") };
            assert!(analytics_wire_endpoint().is_none());
        });
    }

    #[test]
    fn enabled_with_explicit_port() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(AIRHOUSE_BASE_URL_VAR, "http://airhouse:8080");
                std::env::set_var(AIRHOUSE_ADMIN_TOKEN_VAR, "secret");
                std::env::set_var(AIRHOUSE_WIRE_HOST_VAR, "airhouse");
                std::env::set_var(AIRHOUSE_WIRE_PORT_VAR, "9000");
            }
            let cfg = AirhouseConfig::from_env()
                .into_runtime()
                .expect("should be Enabled");
            assert_eq!(cfg.wire_port, 9000);
        });
    }
}
