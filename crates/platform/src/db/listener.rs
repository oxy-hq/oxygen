//! Connection factory for the agentic task router's dedicated LISTEN
//! connection.
//!
//! Two implementations, selected by the same `OXY_DATABASE_AUTH_MODE`
//! that drives the connection pool in [`super::client`]:
//!
//! - **Password**: parses `OXY_DATABASE_URL` once at startup and the
//!   factory clones the resulting [`tokio_postgres::Config`] on each
//!   call. Same URL the pool uses; no surprises.
//! - **IAM**: each factory call awaits a fresh SigV4 token from
//!   [`super::iam::generate_auth_token`] and builds a Config with the
//!   token as the password. The reconnect loop in `PostgresTaskRouter`
//!   calls the factory exactly when it needs a fresh credential, which
//!   is the only time RDS IAM auth checks tokens.
//!
//! Why this lives in the platform crate, not the runtime: the platform
//! crate already owns [`super::auth_mode::IamConfig`] and the token
//! mint. Putting these factories here keeps `agentic-runtime` free of
//! AWS deps.
//!
//! And why it names no `agentic-runtime` type: platform must never import
//! agentic (`internal-docs/domain-boundaries.md` L2). [`ListenerConnectFactory`]
//! is structurally the router's `ListenerConfigFactory` — the same `Arc<dyn Fn>`
//! — so it passes straight in; the TLS posture is returned as this crate's
//! [`SslMode`] and mapped onto the router's enum by `oxy-app`, the layer that
//! wires the two together.
//!
//! ## What this file does NOT do
//!
//! - **No background token refresh.** Unlike the pool (which keeps
//!   long-lived physical connections that need fresh tokens
//!   continuously), the listener has one connection at a time, and
//!   Postgres doesn't re-auth mid-stream. The token only matters at
//!   *connect* time; the reconnect loop is the only refresh hook we
//!   need.
//! - **No retry inside the factory.** AWS credential outages bubble up
//!   to the router's reconnect-with-backoff path, which already retries
//!   200ms → 5s. Re-implementing that here would just busy-loop faster.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use oxy_shared::errors::OxyError;
use tokio_postgres::config::SslMode as PgSslMode;

use super::auth_mode::{DatabaseAuthMode, IamConfig, SslMode};
use super::iam::generate_auth_token;

/// Mints a fresh `tokio_postgres::Config` per (re)connect. Spelled out rather
/// than imported so this crate stays agentic-free; it is the same type as
/// `agentic_runtime::router::ListenerConfigFactory`.
pub type ListenerConnectFactory = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<tokio_postgres::Config, String>> + Send>>
        + Send
        + Sync
        + 'static,
>;

/// Build a [`ListenerConnectFactory`] using the same `OXY_DATABASE_AUTH_MODE`
/// selection that [`super::establish_connection`] honours.
///
/// Returned closure is `Arc`-shared; cheap to clone into the router.
pub fn listener_factory_from_env() -> Result<ListenerConnectFactory, OxyError> {
    match DatabaseAuthMode::from_env()? {
        DatabaseAuthMode::Password => password_factory_from_env(),
        DatabaseAuthMode::Iam => Ok(iam_factory(IamConfig::from_env()?)),
    }
}

/// Resolve the listener's TLS posture from `OXY_DATABASE_SSL_MODE`.
///
/// This is the companion to [`listener_factory_from_env`]: it reads the
/// *same* env var (and default, `require`) the connection pool uses, so
/// the router's dedicated LISTEN connection is exactly as strict as the
/// pool. `oxy-app` maps it onto the router's `TlsVerification`:
/// `require` → encrypt without validating the cert (our RDS and
/// CloudNativePG CAs aren't in the Mozilla bundle); `verify-full` → full
/// chain + hostname verification.
pub fn listener_ssl_mode_from_env() -> Result<SslMode, OxyError> {
    SslMode::from_env()
}

fn password_factory_from_env() -> Result<ListenerConnectFactory, OxyError> {
    let url = std::env::var("OXY_DATABASE_URL").map_err(|_| {
        OxyError::Database(
            "OXY_DATABASE_URL is required for the task router's listener \
             when OXY_DATABASE_AUTH_MODE is password (or unset)."
                .to_string(),
        )
    })?;
    let config: tokio_postgres::Config = url.parse().map_err(|e: tokio_postgres::Error| {
        OxyError::Database(format!("listener factory: invalid database url: {e}"))
    })?;
    Ok(Arc::new(move || {
        let config = config.clone();
        Box::pin(async move { Ok(config) })
    }))
}

/// IAM-mode factory.
///
/// Each call awaits `generate_auth_token` — that does
/// `aws_config::load_defaults().await` internally, which can hit IMDS.
/// For the listener this fires only on reconnect (typically rare); the
/// router's reconnect backoff (200ms → 5s cap) bounds the rate during
/// failover events.
///
/// `SslMode::VerifyFull` and `SslMode::Require` both map to
/// `tokio_postgres::SslMode::Require` at the tokio-postgres layer
/// (tokio-postgres only decides *whether* to do TLS, not how strictly to
/// verify). The verification *strictness* is decided separately by the
/// router's rustls connector via [`listener_ssl_mode_from_env`]:
/// `require` skips certificate validation (matching the pool), and
/// `verify-full` enforces full chain + SAN/hostname verification.
fn iam_factory(config: IamConfig) -> ListenerConnectFactory {
    Arc::new(move || {
        let config = config.clone();
        Box::pin(async move {
            let token = generate_auth_token(&config)
                .await
                .map_err(|e| format!("IAM token mint: {e}"))?;
            Ok(build_iam_config(&config, &token))
        })
    })
}

fn build_iam_config(config: &IamConfig, token: &str) -> tokio_postgres::Config {
    let mut pg = tokio_postgres::Config::new();
    pg.host(&config.host)
        .port(config.port)
        .user(&config.user)
        .password(token)
        .dbname(&config.database)
        .ssl_mode(match config.ssl_mode {
            SslMode::Require | SslMode::VerifyFull => PgSslMode::Require,
        });
    pg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_iam_config() -> IamConfig {
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
    fn iam_config_propagates_all_fields() {
        let cfg = fake_iam_config();
        let pg = build_iam_config(&cfg, "fake-token");
        assert_eq!(pg.get_hosts().len(), 1, "expected exactly one host entry");
        assert_eq!(pg.get_ports(), &[5432]);
        assert_eq!(pg.get_user(), Some("oxy_app"));
        assert_eq!(pg.get_dbname(), Some("oxydb"));
        // get_ssl_mode is exposed by tokio-postgres.
        assert_eq!(pg.get_ssl_mode(), PgSslMode::Require);
    }

    #[test]
    fn verify_full_maps_to_require_at_tokio_postgres_layer() {
        // Both SSL modes map to tokio_postgres `Require` (TLS on/off
        // only). The verify-full vs require *strictness* is decided by
        // the router's rustls connector via
        // `listener_tls_verification_from_env`, not by this field.
        let mut cfg = fake_iam_config();
        cfg.ssl_mode = SslMode::VerifyFull;
        let pg = build_iam_config(&cfg, "fake-token");
        assert_eq!(pg.get_ssl_mode(), PgSslMode::Require);
    }

    #[test]
    #[serial]
    fn ssl_mode_defaults_to_require() {
        clear_env();
        // Unset OXY_DATABASE_SSL_MODE defaults to `require`, which oxy-app
        // maps to the no-verify posture so the listener matches the pool.
        match listener_ssl_mode_from_env() {
            Ok(v) => assert_eq!(v, SslMode::Require),
            Err(e) => panic!("expected Require, got error: {e}"),
        }
        clear_env();
    }

    #[test]
    #[serial]
    fn ssl_mode_reads_verify_full() {
        clear_env();
        unsafe { std::env::set_var("OXY_DATABASE_SSL_MODE", "verify-full") };
        match listener_ssl_mode_from_env() {
            Ok(v) => assert_eq!(v, SslMode::VerifyFull),
            Err(e) => panic!("expected VerifyFull, got error: {e}"),
        }
        clear_env();
    }

    // The env-var dispatch tests are `#[serial]` because they read
    // process-global env vars and would race against each other and
    // against the auth_mode::tests suite. `serial_test` is already a
    // dev-dep for the same reason.
    use serial_test::serial;

    fn clear_env() {
        for var in [
            "OXY_DATABASE_AUTH_MODE",
            "OXY_DATABASE_URL",
            "OXY_DATABASE_HOST",
            "OXY_DATABASE_PORT",
            "OXY_DATABASE_NAME",
            "OXY_DATABASE_USER",
            "OXY_DATABASE_REGION",
            "OXY_DATABASE_SSL_MODE",
        ] {
            unsafe { std::env::remove_var(var) };
        }
    }

    #[tokio::test]
    #[serial]
    async fn dispatch_picks_password_factory_by_default() {
        clear_env();
        unsafe {
            std::env::set_var(
                "OXY_DATABASE_URL",
                "postgresql://u:p@db.example.com:5432/oxy",
            );
        }
        // No `.expect(...)` — ListenerConnectFactory has no Debug impl.
        let factory = match listener_factory_from_env() {
            Ok(f) => f,
            Err(e) => panic!("expected password factory, got error: {e}"),
        };
        // The closure should be infallible for a parsed URL — calling
        // it twice mimics the reconnect loop and asserts the closure
        // is cheap to re-invoke without re-parsing on every call.
        let cfg_a = factory().await.expect("first call");
        let cfg_b = factory().await.expect("second call");
        assert_eq!(cfg_a.get_user(), Some("u"));
        assert_eq!(cfg_b.get_dbname(), Some("oxy"));
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn dispatch_errors_when_password_url_missing() {
        clear_env();
        unsafe { std::env::set_var("OXY_DATABASE_AUTH_MODE", "password") };
        // ListenerConnectFactory is `Arc<dyn Fn(...) -> _>` which has
        // no Debug impl, so `expect_err` won't work — match on the
        // result manually.
        match listener_factory_from_env() {
            Ok(_) => panic!("expected missing-url error, got Ok"),
            Err(err) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("OXY_DATABASE_URL"),
                    "error should name the missing env var; got: {msg}"
                );
            }
        }
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn dispatch_picks_iam_factory_when_mode_iam() {
        clear_env();
        unsafe {
            std::env::set_var("OXY_DATABASE_AUTH_MODE", "iam");
            std::env::set_var("OXY_DATABASE_HOST", "db.example.com");
            std::env::set_var("OXY_DATABASE_NAME", "oxydb");
            std::env::set_var("OXY_DATABASE_USER", "oxy_app");
            std::env::set_var("OXY_DATABASE_REGION", "us-west-2");
        }
        // We can construct the factory without AWS — IAM env-var
        // validation runs synchronously. Don't *call* the factory:
        // generate_auth_token would try to mint a real SigV4 token
        // and either hit IMDS or fail in opaque ways depending on
        // local AWS env. Construction alone validates dispatch.
        let _factory = match listener_factory_from_env() {
            Ok(f) => f,
            Err(e) => panic!("expected iam factory, got error: {e}"),
        };
        clear_env();
    }
}
