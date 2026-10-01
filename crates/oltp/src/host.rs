//! What server a stored host string reaches.
//!
//! Hosts are persisted as the provider or the config spelled them, so two
//! strings that differ — `localhost` and `127.0.0.1:5432`, a Neon endpoint and
//! its `-pooler` door, `DB.Internal` and `db.internal` — can be one server.
//! Shared by the resolver's refusal of a branch row that names production and
//! by `LocalProvider`, which copies and drops branch databases only on a
//! loopback cluster.

use std::net::IpAddr;

use crate::resolver::split_host_port;

/// A host as the server it reaches: lowercase, no port, no trailing dot, every
/// loopback spelling as one, and a Neon endpoint's pooled door as the endpoint.
///
/// The port is dropped on purpose: callers compare servers to refuse, and two
/// spellings that differ only there are refused — the side to err on.
pub(crate) fn server(host: &str) -> String {
    let name = bare_name(host);
    if is_loopback_name(&name) {
        return "loopback".to_string();
    }
    // Neon: `ep-x-123-pooler.<region>.aws.neon.tech` is PgBouncer in front of
    // `ep-x-123.<region>.aws.neon.tech` — the same database.
    match name.split_once('.') {
        Some((label, rest)) => {
            format!("{}.{rest}", label.strip_suffix("-pooler").unwrap_or(label))
        }
        None => name.strip_suffix("-pooler").unwrap_or(&name).to_string(),
    }
}

/// Whether `host` — bare, `host:port`, or IPv6 bracketed or not — is this
/// machine.
pub(crate) fn is_loopback(host: &str) -> bool {
    is_loopback_name(&bare_name(host))
}

fn bare_name(host: &str) -> String {
    let (name, _port) = split_host_port(host.trim());
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn is_loopback_name(name: &str) -> bool {
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    match name.parse::<IpAddr>() {
        // `0.0.0.0` / `::` connect to this host on the platforms Oxy runs on.
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEON: &str = "ep-cool-sky-123.us-east-2.aws.neon.tech";

    const LOOPBACK: [&str; 12] = [
        "localhost",
        "localhost:5432",
        "LocalHost:15432",
        "127.0.0.1",
        "127.0.0.1:5432",
        "127.1.2.3",
        "[::1]:5432",
        "::1",
        "::ffff:127.0.0.1",
        "0.0.0.0",
        "db.localhost",
        " localhost. ",
    ];

    #[test]
    fn every_spelling_of_one_server_is_one_server() {
        for local in LOOPBACK {
            assert_eq!(server(local), server("localhost"), "{local:?}");
        }
        for neon in [
            "EP-Cool-Sky-123.us-east-2.aws.neon.tech",
            "ep-cool-sky-123-pooler.us-east-2.aws.neon.tech",
            "ep-cool-sky-123.us-east-2.aws.neon.tech:5432",
            "ep-cool-sky-123.us-east-2.aws.neon.tech.",
        ] {
            assert_eq!(server(neon), server(NEON), "{neon:?}");
        }
    }

    #[test]
    fn different_servers_stay_different() {
        assert_ne!(
            server("ep-warm-sea-456.us-east-2.aws.neon.tech"),
            server(NEON)
        );
        assert_ne!(server("db.internal"), server("localhost"));
        assert_ne!(server("10.0.0.5"), server("127.0.0.1"));
    }

    #[test]
    fn loopback_is_every_spelling_of_this_machine_and_nothing_else() {
        for local in LOOPBACK {
            assert!(is_loopback(local), "{local:?}");
        }
        for remote in [
            "db.internal",
            "db.internal:5432",
            "10.0.0.5:5432",
            "[2001:db8::1]:5432",
            NEON,
            "localhost.evil.example",
            "",
        ] {
            assert!(!is_loopback(remote), "{remote:?}");
        }
    }
}
