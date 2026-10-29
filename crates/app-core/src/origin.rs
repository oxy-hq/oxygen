//! Which request origins a server-side gate admits, and the predicate the CORS
//! layer shares with it (`oxy-app`'s `build_cors_layer`).

use axum::http::header::HeaderValue;
use axum::http::{HeaderMap, header};

/// Shared predicate body for both `build_cors_layer` (browser CORS preflight)
/// and [`is_allowed_origin`] (server-side gate). Both auto-allow the request's
/// own host, or a request whose two ends are both on this machine.
pub fn cors_allow(origin: &HeaderValue, headers: &HeaderMap) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    is_self_origin(origin, headers) || is_local_dev_pair(origin, headers)
}

/// The host the request arrived at: `X-Forwarded-Host` when a TLS-terminating
/// reverse proxy set one, else `Host`. Empty when neither is present, which is
/// how a non-browser client shows up. Our own hop to the Factory
/// (`ide_proxy::preserve_public_host`) sets it too, so on the Factory this is
/// the host the caller used, not the in-cluster Service.
pub(crate) fn request_host(headers: &HeaderMap) -> &str {
    headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// Return `true` when `origin` (e.g. `https://app.oxygen-hq.com`) targets the
/// same host as the incoming request. This covers both direct-bind and
/// behind-a-load-balancer deployments without an env var.
fn is_self_origin(origin: &str, headers: &HeaderMap) -> bool {
    let origin_host = match origin.split_once("://") {
        Some((_, rest)) => rest.trim_end_matches('/'),
        None => return false,
    };
    if origin_host.is_empty() {
        return false;
    }
    let host = request_host(headers);
    !host.is_empty() && origin_host == host
}

/// Both ends of the request are on this machine — the local-development case.
///
/// A dev setup routinely mismatches the browser's origin against the host the
/// backend sees, so [`is_self_origin`] cannot pass it: Vite proxies `/api` with
/// `changeOrigin: true`, so a page loaded from `http://127.0.0.1:5173` reaches
/// the backend as `Host: localhost:3000`.
///
/// This replaced a fixed allowlist of literal `http://localhost:<port>` strings,
/// which failed that pair twice over. `127.0.0.1` and `localhost` are the same
/// machine, but only one of the two spellings was listed — so a developer who
/// typed the IP got 403 "origin not allowed" on every custom-app query, with
/// nothing in the URL to suggest why. And the ports were hard-coded while
/// `OXY_DEV_PORT` / `OXY_DEV_PROXY_TARGET` exist precisely so several dev
/// servers can coexist, so moving off `:5173` failed the same way.
///
/// The rule is a **pair**, not "loopback origins are fine": the origin must be
/// loopback AND the request must have arrived at a loopback host. That keeps it
/// strictly narrower than a bare origin check where it matters — a page on
/// `http://localhost:1234` posting to `app.oxygen-hq.com` carries the viewer's
/// cookie and stays rejected — while being wider everywhere it was costing
/// time: any port, either IPv4 or IPv6 loopback, and the `*.localhost` names
/// org-subdomain testing uses.
///
/// ## What the pair leans on
///
/// The safety half is an **ops** property, not a code property: it holds while
/// `request_host()` is never loopback in production. An edge that sets
/// `X-Forwarded-Host` satisfies that. One that does not may not — nginx
/// defaults to `proxy_set_header Host $proxy_host`, so `proxy_pass
/// http://127.0.0.1:3000` with no explicit `Host` / `X-Forwarded-Host` presents
/// `Host: localhost:3000` to this code, and every loopback origin is then
/// accepted with `allow_credentials(true)`. Still far narrower than the fixed
/// allowlist this replaced — which accepted eight literal `localhost` origins
/// with no reference to the host at all — and the attack needs a server already
/// running on the victim's own machine. But it is conditional, and anyone
/// widening this further should know which condition they are leaning on.
fn is_local_dev_pair(origin: &str, headers: &HeaderMap) -> bool {
    let Some((_, origin_host)) = origin.split_once("://") else {
        return false;
    };
    is_loopback_host(origin_host.trim_end_matches('/')) && is_loopback_host(request_host(headers))
}

/// `true` for a `host[:port]` naming this machine: `localhost`, an RFC 6761
/// `*.localhost` subdomain, or any loopback IP literal (`127.0.0.0/8`, `::1`).
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() {
        return false;
    }
    // Strip the port. An IPv6 literal is bracketed and its address contains
    // colons, so the closing bracket — not the first colon — is the boundary.
    let hostname = match host.rfind(']') {
        Some(close) => host[..=close].trim_start_matches('[').trim_end_matches(']'),
        // An unbracketed IPv6 literal is all colons, so the port split yields
        // nothing. Rare — a spec-compliant proxy brackets it — but silently
        // reading `::1` as "not loopback" is the wrong way to be wrong.
        None => match host.split(':').next() {
            Some("") | None => host,
            Some(before_port) => before_port,
        },
    };
    if hostname.eq_ignore_ascii_case("localhost")
        || hostname
            .rsplit_once('.')
            .is_some_and(|(_, tld)| tld.eq_ignore_ascii_case("localhost"))
    {
        return true;
    }
    hostname
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Check whether the `Origin` (or `Referer`) header on an incoming request
/// targets a permitted origin. Mirrors the CORS layer's predicate as a
/// belt-and-braces server-side check used by sensitive endpoints (the
/// custom-app data gate, etc.).
///
/// Rules:
///   - No `Origin` and no `Referer` → allowed (programmatic clients
///     legitimately omit both).
///   - Origin matches the request's `Host` / `X-Forwarded-Host` → allowed
///     (the standard same-domain deployment).
///   - Origin AND the request's host are both loopback → allowed (local dev
///     across a proxy that rewrites the host; see [`is_local_dev_pair`]).
///   - Otherwise → rejected.
///
/// If only `Referer` is present, its scheme+host is extracted and used as
/// the candidate origin.
///
/// The extracted `oxy-api-frontline` surface gates "leave kiosk mode" on it.
pub fn is_allowed_origin(headers: &HeaderMap) -> bool {
    let candidate = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok())
                .and_then(|r| {
                    // Extract scheme://host[:port] from the Referer URL.
                    let without_scheme = r.find("://").map(|i| &r[i + 3..])?;
                    let host_end = without_scheme.find('/').unwrap_or(without_scheme.len());
                    let host = &without_scheme[..host_end];
                    let scheme = &r[..r.find("://").unwrap()];
                    Some(format!("{scheme}://{host}"))
                })
        });
    match candidate {
        // No Origin or Referer → allow non-browser clients.
        None => true,
        Some(origin) => is_self_origin(&origin, headers) || is_local_dev_pair(&origin, headers),
    }
}
