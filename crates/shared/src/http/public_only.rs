//! Requests to an address someone outside Oxygen chose, which must reach
//! only the public internet.
//!
//! Two callers fetch such a URL from inside our network: a custom app's
//! `ctx.fetch`, and the registration and key refresh of an org's Kubernetes
//! token issuer (`oxy_api_identity::org_api_access::oidc_issuers`). Either
//! could otherwise be pointed at the cloud metadata service, a pod in the
//! cluster, or a loopback port. Two layers stop it:
//!
//!  1. [`is_safe_outbound`] rejects the URL up front: not HTTPS, a literal
//!     private IP, a name that is plainly internal.
//!  2. [`PublicOnlyDnsResolver`] checks every *resolved* address at connect
//!     time, which closes DNS rebinding — a public name whose A record points
//!     at `169.254.169.254`.
//!
//! [`builder`] puts the resolver on a client and turns redirects off, so a
//! `302` to an internal address cannot launder past either layer on a later
//! hop (reqwest gives no per-hop validation hook). The caller sets its own
//! timeout.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// [`super::builder`] with redirects off and the [`PublicOnlyDnsResolver`].
/// No overall timeout: set one.
pub fn builder() -> reqwest::ClientBuilder {
    super::builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(PublicOnlyDnsResolver))
}

/// First-layer SSRF check — rejects non-HTTPS, loopback,
/// private, link-local, and internal-suffix hosts *by inspecting the URL
/// string only*. A non-literal hostname that resolves to a private/internal
/// address (DNS rebinding) passes this check; that case is caught at connect
/// time by [`PublicOnlyDnsResolver`].
pub fn is_safe_outbound(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host_for_ip = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host_for_ip.parse::<IpAddr>() {
        return is_public_ip(&ip);
    }
    let lower = host.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "localhost" | "ip6-localhost" | "ip6-loopback"
    ) {
        return false;
    }
    const INTERNAL_SUFFIXES: &[&str] = &[
        ".internal",
        ".local",
        ".localdomain",
        ".svc",
        ".cluster.local",
    ];
    !INTERNAL_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

pub fn is_public_ip(ip: &IpAddr) -> bool {
    // Fold an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) back to IPv4 before
    // classifying. Otherwise `::ffff:169.254.169.254` (cloud metadata),
    // `::ffff:10.x`, `::ffff:127.0.0.1` have `segments()[0] == 0` and slip
    // through the v6 arm below as "public" — and Linux routes a v4-mapped
    // address on a v6 socket straight to the underlying internal IPv4.
    // `to_canonical()` (stable since 1.75) makes the v4 checks below apply.
    let ip = ip.to_canonical();
    if ip.is_loopback() || ip.is_unspecified() {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => {
            !v4.is_private() && !v4.is_link_local() && !v4.is_broadcast() && !v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            // NAT64 (`64:ff9b::/96`) and IPv4-compatible (`::/96`) addresses
            // embed a v4 address in the low 32 bits. `to_canonical()` folds
            // v4-*mapped* (`::ffff:a.b.c.d`) but NOT these, so an id like
            // `64:ff9b::169.254.169.254` (or `::10.0.0.5`) would otherwise pass
            // the v6 arm below as "public" while pointing at metadata / RFC1918
            // — reachable on any host with a NAT64 path. Re-run the v4 checks on
            // the embedded address instead of trusting the v6 prefix.
            if let Some(v4) = embedded_ipv4(&v6) {
                return is_public_ip(&IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            let link_local = (seg & 0xffc0) == 0xfe80;
            let unique_local = (seg & 0xfe00) == 0xfc00;
            !link_local && !unique_local
        }
    }
}

/// Extract the IPv4 address embedded in an IPv4-compatible (`::/96`) or NAT64
/// (`64:ff9b::/96`) IPv6 address. Returns `None` for every other v6 address.
///
/// v4-*mapped* (`::ffff:0:0/96`) is deliberately excluded: [`is_public_ip`]
/// calls `to_canonical()` first, which already folds it to a real `IpAddr::V4`,
/// so it never reaches here as a v6 value. Both prefixes handled here are
/// non-global by design (IPv4-compatible is deprecated; NAT64 is a translation
/// prefix), so treating anything in them as an embedded v4 cannot mis-block a
/// legitimately-routable global v6 address.
fn embedded_ipv4(v6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let seg = v6.segments();
    let is_v4_compatible = seg[0..6] == [0, 0, 0, 0, 0, 0];
    let is_nat64 = seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0];
    if is_v4_compatible || is_nat64 {
        Some(std::net::Ipv4Addr::from(
            ((seg[6] as u32) << 16) | seg[7] as u32,
        ))
    } else {
        None
    }
}

/// Partition resolved socket addresses into the ones safe to connect to,
/// dropping any that point at a non-public IP. Factored out of the resolver so
/// the rebinding decision is unit-testable without real DNS. Returns `Err` with
/// a diagnostic when *every* resolved address is filtered out (the rebinding
/// case), so the caller surfaces a clear message instead of an opaque DNS miss.
fn keep_public_addrs(host: &str, resolved: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, String> {
    let safe: Vec<SocketAddr> = resolved
        .into_iter()
        .filter(|addr| is_public_ip(&addr.ip()))
        .collect();
    if safe.is_empty() {
        return Err(format!("'{host}' resolves only to non-public addresses"));
    }
    Ok(safe)
}

/// Second-layer SSRF defense: a custom reqwest DNS resolver
/// that drops every resolved address pointing at a non-public IP. This closes
/// the DNS-rebinding vector [`is_safe_outbound`] can't see — a public hostname
/// whose A record resolves to `169.254.169.254` / `10.x` / `127.x`. Validation
/// happens at resolution (i.e. connect) time, so there is no resolve-then-
/// reconnect TOCTOU window, and reqwest connects only to an address we vetted.
#[derive(Debug)]
pub struct PublicOnlyDnsResolver;

impl reqwest::dns::Resolve for PublicOnlyDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            // Port 0: reqwest overrides it with the URL's port; we only care
            // about the resolved IPs here.
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0u16))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();
            let safe = keep_public_addrs(&host, resolved)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            let addrs: reqwest::dns::Addrs = Box::new(safe.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).unwrap()
    }

    #[test]
    fn rejects_non_https() {
        assert!(!is_safe_outbound(&url("http://example.com")));
    }

    #[test]
    fn allows_public_https_host() {
        assert!(is_safe_outbound(&url("https://api.example.com/v1")));
    }

    #[test]
    fn rejects_loopback_and_private_hosts() {
        assert!(!is_safe_outbound(&url("https://localhost/")));
        assert!(!is_safe_outbound(&url("https://127.0.0.1/")));
        assert!(!is_safe_outbound(&url("https://10.0.0.5/")));
        assert!(!is_safe_outbound(&url("https://192.168.1.1/")));
        assert!(!is_safe_outbound(&url("https://[::1]/")));
    }

    #[test]
    fn rejects_internal_suffix_hosts() {
        assert!(!is_safe_outbound(&url("https://service.internal/")));
        assert!(!is_safe_outbound(&url("https://db.svc/")));
        assert!(!is_safe_outbound(&url("https://app.cluster.local/")));
        assert!(!is_safe_outbound(&url("https://box.localdomain/")));
    }

    #[test]
    fn allows_public_ip_literal() {
        assert!(is_safe_outbound(&url("https://93.184.216.34/")));
    }

    #[test]
    fn rejects_link_local_ip() {
        assert!(!is_safe_outbound(&url("https://169.254.1.1/")));
    }

    #[test]
    fn rejects_ipv4_mapped_ipv6_internal_literals() {
        // ::ffff:a.b.c.d must be canonicalized to IPv4 before classification —
        // otherwise the metadata endpoint (and RFC1918 / loopback) reach the
        // internal target via a v6-socket-routed v4-mapped address.
        assert!(!is_safe_outbound(&url(
            "https://[::ffff:169.254.169.254]/latest/meta-data/"
        )));
        assert!(!is_safe_outbound(&url("https://[::ffff:10.0.0.5]/")));
        assert!(!is_safe_outbound(&url("https://[::ffff:127.0.0.1]/")));
        // The primitive directly: mapped-internal is rejected, mapped-public
        // still allowed.
        assert!(!is_public_ip(
            &"::ffff:169.254.169.254".parse::<IpAddr>().unwrap()
        ));
        assert!(is_public_ip(
            &"::ffff:93.184.216.34".parse::<IpAddr>().unwrap()
        ));
    }

    #[test]
    fn nat64_and_v4_compatible_embedded_internal_ips_are_blocked() {
        // NAT64 (`64:ff9b::/96`) and IPv4-compatible (`::/96`) addresses embed a
        // v4 in the low 32 bits and are NOT folded by `to_canonical()`, so the
        // embedded v4 must be re-checked. An embedded metadata / RFC1918 / loopback
        // address must be rejected; an embedded public one still allowed.
        for internal in [
            "64:ff9b::169.254.169.254", // NAT64 → cloud metadata
            "64:ff9b::10.0.0.5",        // NAT64 → RFC1918
            "64:ff9b::127.0.0.1",       // NAT64 → loopback
            "::169.254.169.254",        // IPv4-compatible → cloud metadata
            "::10.0.0.5",               // IPv4-compatible → RFC1918
        ] {
            let ip = internal.parse::<IpAddr>().unwrap();
            assert!(!is_public_ip(&ip), "{internal} must be classed non-public");
            assert!(
                !is_safe_outbound(&url(&format!("https://[{internal}]/latest/meta-data/"))),
                "{internal} must be blocked by is_safe_outbound"
            );
        }
        // An embedded PUBLIC v4 behind NAT64 is still reachable.
        assert!(is_public_ip(
            &"64:ff9b::93.184.216.34".parse::<IpAddr>().unwrap()
        ));
    }

    // ── keep_public_addrs (DNS-rebinding defense) ───────────────────────────

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn keep_public_addrs_filters_private_and_metadata() {
        // A "public" hostname that resolves to a mix keeps only the public IP.
        let resolved = vec![sa("169.254.169.254:0"), sa("93.184.216.34:0")];
        let kept = keep_public_addrs("evil.example.com", resolved).unwrap();
        assert_eq!(kept, vec![sa("93.184.216.34:0")]);
    }

    #[test]
    fn keep_public_addrs_rejects_when_all_private() {
        // Pure rebinding: every resolved address is internal → hard error.
        let resolved = vec![sa("169.254.169.254:0"), sa("10.0.0.5:0"), sa("127.0.0.1:0")];
        let err = keep_public_addrs("evil.example.com", resolved).unwrap_err();
        assert!(err.contains("non-public"), "unexpected error: {err}");
    }

    #[test]
    fn keep_public_addrs_passes_all_public() {
        let resolved = vec![sa("93.184.216.34:0"), sa("1.1.1.1:0")];
        let kept = keep_public_addrs("api.example.com", resolved.clone()).unwrap();
        assert_eq!(kept, resolved);
    }
}
