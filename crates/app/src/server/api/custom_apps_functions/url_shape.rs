//! The shape of a URL — scheme, host, port — and never its payload: userinfo,
//! path and query are where credentials and customer data travel.
//!
//! Two readers, one rule. `runtime`'s fetch spans record where a `ctx.fetch`
//! went (through `host_call_attrs`, which hands out this same function), and
//! the failure fingerprint keeps a bare URL's host so two endpoints in one
//! function stay two patterns (`failure_signal::url_host`). They must not
//! disagree about what a host is, so there is one parser.
//!
//! Always compiled, unlike `host_call_attrs`: that module exists only for the
//! V8 runtime's spans and is gated with it, but a failure is fingerprinted in
//! every configuration — the feature-off build still writes an invocation row
//! — and a normalizer that kept hosts under one set of build flags and not
//! another would give one message two fingerprints.

/// Where a URL points — never the path or query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FetchTarget {
    pub scheme: String,
    pub host: String,
    // Read only by the runtime's fetch span; the fingerprint folds the port.
    #[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
    pub port: Option<u16>,
}

pub(super) fn fetch_target(url: &str) -> FetchTarget {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => (String::new(), url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Drop userinfo if a caller embedded credentials in the URL.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), Some(port)),
            Err(_) => (authority.to_string(), None),
        },
        _ => (authority.to_string(), None),
    };
    FetchTarget {
        scheme,
        host: host.to_ascii_lowercase(),
        port,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_target_keeps_scheme_host_port_and_drops_the_rest() {
        let t = fetch_target("https://user:pw@api.stripe.com:8443/v1/charges?key=sk_live_123");
        assert_eq!(t.scheme, "https");
        assert_eq!(t.host, "api.stripe.com");
        assert_eq!(t.port, Some(8443));
        let t = fetch_target("http://example.test/path");
        assert_eq!(
            (t.scheme.as_str(), t.host.as_str(), t.port),
            ("http", "example.test", None)
        );
        // Not a URL at all: no scheme, and whatever came in is the "host" —
        // there is nothing sensitive to strip and nothing to panic on.
        let t = fetch_target("not a url");
        assert_eq!(t.scheme, "");
        assert_eq!(t.port, None);
    }
}
