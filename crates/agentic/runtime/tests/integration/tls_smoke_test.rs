//! Smoke test for the rustls-based TLS connector that the IAM-aware
//! listener (task #13+) will use.
//!
//! Validates three things before we commit `PostgresTaskRouter` to it:
//!   1. The `tokio-postgres-rustls` + `webpki-roots` combo actually
//!      compiles and links.
//!   2. `MakeRustlsConnect` is the correct type for
//!      `tokio_postgres::Config::connect(tls)`.
//!   3. A `Config::ssl_mode(Prefer) + connect(MakeRustlsConnect)` call
//!      round-trips against a plain-TCP server — i.e. the connector
//!      doesn't reject non-TLS servers when the mode allows it.
//!
//! Run against a real RDS or a TLS-enabled Postgres separately if you
//! want to validate the `Require`/`VerifyFull` path — the testcontainer
//! used here only serves plain TCP.

use std::time::Duration;

use tokio_postgres::Config;
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

async fn test_db_url() -> String {
    oxy_test_utils::containers::shared_db_url::<agentic_runtime::migration::RuntimeMigrator>().await
}

fn build_rustls_connector() -> MakeRustlsConnect {
    // Install the default ring crypto provider once per process. Rustls
    // 0.23 made the provider choice explicit; downstream `ClientConfig`
    // construction panics otherwise.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    MakeRustlsConnect::new(client_config)
}

/// Connect to the testcontainer using a rustls-backed TLS connector
/// with `SslMode::Prefer`. The server is plain TCP, so the connector
/// must fall back to non-TLS — same behaviour the listener will rely
/// on for dev / local deployments.
#[tokio::test]
async fn rustls_connector_falls_back_on_plain_server() {
    let url = test_db_url().await;
    let mut config: Config = url.parse().expect("parse test db url");
    config.ssl_mode(SslMode::Prefer);

    let tls = build_rustls_connector();

    // Retry briefly — the reusable testcontainer may still be starting.
    let mut last_err: Option<tokio_postgres::Error> = None;
    for attempt in 0..5 {
        match config.connect(tls.clone()).await {
            Ok((client, connection)) => {
                // Spawn the driver task so the client can do queries.
                let handle = tokio::spawn(async move {
                    let _ = connection.await;
                });
                let row = client
                    .query_one("SELECT 1::int4 AS one", &[])
                    .await
                    .expect("smoke query failed");
                let val: i32 = row.get("one");
                assert_eq!(val, 1);
                handle.abort();
                return;
            }
            Err(e) if attempt < 4 => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => panic!("rustls Prefer connect failed: {e}"),
        }
    }
    panic!(
        "rustls Prefer connect never succeeded; last error: {:?}",
        last_err
    );
}

/// Disable SSL entirely — sanity check that the connector doesn't
/// somehow force TLS when the mode says no. Same provider construction
/// path as the Prefer test, just configured differently.
#[tokio::test]
async fn rustls_connector_respects_disable_mode() {
    let url = test_db_url().await;
    let mut config: Config = url.parse().expect("parse test db url");
    config.ssl_mode(SslMode::Disable);

    let tls = build_rustls_connector();

    let mut last_err: Option<tokio_postgres::Error> = None;
    for attempt in 0..5 {
        match config.connect(tls.clone()).await {
            Ok((client, connection)) => {
                let handle = tokio::spawn(async move {
                    let _ = connection.await;
                });
                client
                    .simple_query("SELECT 1")
                    .await
                    .expect("simple_query failed");
                handle.abort();
                return;
            }
            Err(e) if attempt < 4 => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => panic!("rustls Disable connect failed: {e}"),
        }
    }
    panic!(
        "rustls Disable connect never succeeded; last error: {:?}",
        last_err
    );
}
