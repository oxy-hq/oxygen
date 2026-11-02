//! One way to open a tenant Postgres connection.
//!
//! Every tenant-side connection — DDL, migrations, `ctx.tx`, the ERD reader —
//! goes through here so that TLS is not a per-call-site decision. It was, and
//! all four sites independently chose `NoTls`, which is invisible against the
//! local demo cluster and fatal against any managed provider: Neon, Supabase and
//! RDS with `rds.force_ssl` all refuse a plaintext session outright.
//!
//! **The DSN decides, not a flag.** `sslmode` is already part of every DSN this
//! crate builds ([`crate::provisioner::sslmode_for`]), so a single rustls
//! connector covers every case:
//!
//! | `sslmode` | Behaviour |
//! | --- | --- |
//! | `disable` | plaintext; what a local Docker Postgres uses |
//! | absent / `prefer` | try TLS, fall back to plaintext |
//! | `require` and stricter | TLS or the connection fails |
//!
//! So the same helper serves a container with no certificates and a managed
//! provider that mandates them, and adding a provider cannot reintroduce the
//! bug by forgetting a connector argument.

use std::net::IpAddr;
use std::sync::OnceLock;

use tokio_postgres::Client;
use tokio_postgres_rustls::MakeRustlsConnect;

/// Process-wide TLS config. Built once: assembling the root store per
/// connection would show up on `ctx.tx`, which opens one per invocation.
fn tls_connector() -> MakeRustlsConnect {
    static TLS: OnceLock<MakeRustlsConnect> = OnceLock::new();
    TLS.get_or_init(|| {
        // Idempotent, and `let _` because another crate in this process may
        // legitimately have installed it first.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        MakeRustlsConnect::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
    .clone()
}

/// Connect and spawn the connection driver.
///
/// `label` names the caller in the log line emitted when the driver ends, so a
/// dropped connection is attributable to `ctx.tx` or the migrator rather than
/// appearing as an anonymous disconnect.
///
/// **On failure the error is the most informative one, not the last one.**
/// `tokio_postgres::connect` tries every address the host resolves to and keeps
/// only the final error, so a server that refuses the session (Neon:
/// `Your account or project has exceeded the quota`) is overwritten by a later
/// IPv6 `Network is unreachable` and reads as a networking fault. Here each
/// address is tried separately and the first error that is not a failure to
/// reach the address wins; see [`connect_reporting_server_errors`].
pub async fn connect(dsn: &str, label: &'static str) -> Result<Client, tokio_postgres::Error> {
    let (client, connection) = connect_reporting_server_errors(dsn).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!("{label} connection closed: {e}");
        }
    });
    Ok(client)
}

type Connected = (
    Client,
    tokio_postgres::Connection<
        tokio_postgres::Socket,
        <MakeRustlsConnect as tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket>>::Stream,
    >,
);

/// `Display` of `Error::connect`, i.e. the TCP connection never opened.
/// `tokio_postgres::Error` has no kind accessor, so this is a text match; if a
/// release rewords it the error is treated as informative, which only means the
/// first error is reported instead of a later one.
fn is_unreachable(e: &tokio_postgres::Error) -> bool {
    e.as_db_error().is_none() && e.to_string() == "error connecting to server"
}

/// [`tokio_postgres::connect`], except the error returned is the first one that
/// is not a failure to reach an address (a server `ErrorResponse`, a TLS or
/// authentication failure), or the first error if every address was
/// unreachable.
///
/// The same rule as `agentic-connector`'s `pg_connect`, which also reports how
/// many addresses were tried. This crate cannot import it (the platform never
/// depends on agentic crates) and its callers need a `tokio_postgres::Error`
/// for [`pg_detail`], so it returns one of the real attempts' errors.
///
/// Only the single-host DSN shape every tenant DSN has is split per address; a
/// multi-host, `hostaddr` or unresolvable DSN goes straight to
/// `tokio_postgres`, which reports its own configuration or lookup error.
async fn connect_reporting_server_errors(dsn: &str) -> Result<Connected, tokio_postgres::Error> {
    let config: tokio_postgres::Config = dsn.parse()?;
    let plain = || config.connect(tls_connector());
    let (hosts, hostaddrs, ports) = (
        config.get_hosts(),
        config.get_hostaddrs(),
        config.get_ports(),
    );
    let [tokio_postgres::config::Host::Tcp(host)] = hosts else {
        return plain().await;
    };
    if !hostaddrs.is_empty() {
        return plain().await;
    }
    let port = ports.first().copied().unwrap_or(5432);
    let Ok(resolved) = tokio::net::lookup_host((host.as_str(), port)).await else {
        return plain().await;
    };
    let addrs: Vec<IpAddr> = resolved.map(|a| a.ip()).collect();
    if addrs.is_empty() {
        return plain().await;
    }
    connect_to_each(&config, addrs).await
}

/// Try `addrs` in order against a single-host `config` and return the first
/// success. Every address is tried, as `tokio_postgres` does: a transient
/// server refusal on one address must not stop a later one from accepting.
/// Only the error reported is chosen: the first informative failure, else the
/// first one.
async fn connect_to_each(
    config: &tokio_postgres::Config,
    addrs: Vec<IpAddr>,
) -> Result<Connected, tokio_postgres::Error> {
    let mut informative: Option<tokio_postgres::Error> = None;
    let mut first_unreachable: Option<tokio_postgres::Error> = None;
    for ip in addrs {
        // One host plus one `hostaddr`: dial the address, keep the host name
        // for TLS server-name validation.
        let mut one = config.clone();
        one.hostaddr(ip);
        match one.connect(tls_connector()).await {
            Ok(connected) => return Ok(connected),
            Err(e) if is_unreachable(&e) => {
                first_unreachable.get_or_insert(e);
            }
            Err(e) => {
                informative.get_or_insert(e);
            }
        }
    }
    // `addrs` is never empty, so one of the two slots is filled.
    Err(informative
        .or(first_unreachable)
        .expect("connect_to_each is given at least one address"))
}

/// Render a Postgres failure the way a human needs it.
///
/// `tokio_postgres::Error`'s Display is just "db error" — the constraint name,
/// the column, the reason and any `RAISE ... USING HINT` all live in the
/// `DbError`. Lives here because every caller in this crate reaches Postgres
/// through this module, and "statement failed: db error" has now cost two
/// separate debugging sessions.
pub fn pg_detail(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            // SQLSTATE first: it is the only stable, machine-matchable part of
            // a Postgres error, and callers that classify failures (see
            // `provisioner::is_unconfined`) must not have to match on prose.
            let mut msg = format!("[{}] {}", db.code().code(), db.message());
            if let Some(detail) = db.detail() {
                msg.push_str(&format!(" — {detail}"));
            }
            if let Some(hint) = db.hint() {
                msg.push_str(&format!(" (hint: {hint})"));
            }
            msg
        }
        // Not a server error. `Display` is the fixed phase name
        // (`error connecting to server`); the io cause behind it
        // (`Network is unreachable (os error 101)`) is the `source()`.
        None => {
            let mut msg = e.to_string();
            let mut source = std::error::Error::source(e);
            while let Some(cause) = source {
                let text = cause.to_string();
                if !msg.contains(&text) {
                    msg.push_str(": ");
                    msg.push_str(&text);
                }
                source = cause.source();
            }
            msg
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use tokio_postgres::config::SslMode;

    /// The whole design rests on `sslmode` surviving into the parsed config, so
    /// that one connector can serve both a plaintext container and a provider
    /// that mandates TLS. If this ever stopped holding, every tenant connection
    /// would silently pick the wrong mode.
    #[test]
    fn sslmode_in_the_dsn_is_what_decides() {
        let parse = |dsn: &str| {
            dsn.parse::<tokio_postgres::Config>()
                .unwrap()
                .get_ssl_mode()
        };

        assert_eq!(
            parse("postgres://u:p@localhost:55432/db?sslmode=disable"),
            SslMode::Disable,
            "the local demo cluster has no certificates"
        );
        assert_eq!(
            parse("postgres://u:p@ep-x.neon.tech/neondb?sslmode=require"),
            SslMode::Require,
            "a managed provider must not be reachable in plaintext"
        );
        assert_eq!(
            parse("postgres://u:p@localhost/db"),
            SslMode::Prefer,
            "an unannotated DSN tries TLS and falls back, so both work"
        );
    }

    // ── Which address's error is reported ────────────────────────────────────

    use super::{connect_to_each, pg_detail};
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub(crate) const QUOTA: &str = "Your account or project has exceeded the quota";

    /// A Postgres server that answers the first `refusals` startup packets with
    /// an `ErrorResponse` carrying [`QUOTA`], then accepts. Returns its port.
    /// `api::probe`'s tests stand on it too.
    pub(crate) async fn fake_server(refusals: usize) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let refuse = seen.fetch_add(1, Ordering::SeqCst) < refusals;
                tokio::spawn(async move {
                    let mut len = [0u8; 4];
                    if sock.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let mut body = vec![0u8; u32::from_be_bytes(len) as usize - 4];
                    if sock.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let mut out = Vec::new();
                    if refuse {
                        let mut fields = Vec::new();
                        for (tag, text) in [(b'S', "FATAL"), (b'C', "XX000"), (b'M', QUOTA)] {
                            fields.push(tag);
                            fields.extend_from_slice(text.as_bytes());
                            fields.push(0);
                        }
                        fields.push(0);
                        out.push(b'E');
                        out.extend_from_slice(&(fields.len() as u32 + 4).to_be_bytes());
                        out.extend_from_slice(&fields);
                    } else {
                        out.extend_from_slice(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0]);
                        out.extend_from_slice(&[b'K', 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0, 2]);
                        out.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
                    }
                    let _ = sock.write_all(&out).await;
                    let _ = sock.read(&mut [0u8; 1]).await;
                });
            }
        });
        port
    }

    fn dsn_for(port: u16) -> tokio_postgres::Config {
        format!("postgres://u@db.example.test:{port}/d?sslmode=disable")
            .parse()
            .unwrap()
    }

    const LIVE: &str = "127.0.0.1";
    /// Nothing listens on the IPv6 loopback at the fake server's port, so it is
    /// refused (or unroutable) at the TCP level: an io error, as with the
    /// pods that have no IPv6 route.
    const UNREACHABLE: &str = "::1";

    async fn failure(port: u16, order: [&str; 2]) -> String {
        let addrs: Vec<IpAddr> = order.iter().map(|a| a.parse().unwrap()).collect();
        match connect_to_each(&dsn_for(port), addrs).await {
            Ok(_) => panic!("expected the connection to fail"),
            Err(e) => pg_detail(&e),
        }
    }

    /// The incident: the server refuses the first address and the next is
    /// unreachable. The refusal is what must be reported.
    #[tokio::test]
    async fn a_refusal_outranks_a_later_unreachable_address() {
        let port = fake_server(usize::MAX).await;
        let message = failure(port, [LIVE, UNREACHABLE]).await;
        assert!(message.contains(QUOTA), "refusal was lost: {message}");
    }

    #[tokio::test]
    async fn a_refusal_outranks_an_earlier_unreachable_address() {
        let port = fake_server(usize::MAX).await;
        let message = failure(port, [UNREACHABLE, LIVE]).await;
        assert!(message.contains(QUOTA), "refusal was lost: {message}");
    }

    /// With only io errors the message carries the io cause, not just the
    /// fixed `error connecting to server`.
    #[tokio::test]
    async fn unreachable_addresses_report_their_os_error() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let message = failure(port, [LIVE, LIVE]).await;
        assert!(
            message.starts_with("error connecting to server: ") && message.contains("(os error"),
            "io cause missing: {message}"
        );
    }

    /// A transient refusal on one address must not stop a later address from
    /// accepting: every address is still tried.
    #[tokio::test]
    async fn a_refused_address_does_not_stop_a_later_one_accepting() {
        let port = fake_server(1).await;
        let addrs = vec![LIVE.parse().unwrap(), LIVE.parse().unwrap()];
        let Ok((client, _connection)) = connect_to_each(&dsn_for(port), addrs).await else {
            panic!("the second attempt should have been accepted");
        };
        assert!(!client.is_closed());
    }
}
