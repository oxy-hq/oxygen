//! The audience trusted access requires: one per deployment.
//!
//! A GitHub OIDC token names who it is for in `aud`, and the `jti` of a spent
//! one is recorded per database. With one audience for every deployment, a
//! token presented to staging is a token production has never seen: whoever
//! runs — or reads the traffic of — one deployment could replay it at another
//! inside its five-minute life. So each deployment requires an audience of its
//! own, derived from its public address, and a token asked for one deployment
//! is `wrong_audience` at every other.
//!
//! The value is `oxy:<host>` — `oxy:app.oxygen-hq.com` — with `:<port>` only
//! when the address carries a port that is not its scheme's default. A
//! deployment with no public address configured (a dev box) has nothing to tell
//! itself apart by and requires plain [`AUDIENCE_OXY`] — which no client ever
//! asks GitHub for, so GitHub sign-in is simply not available on it.
//!
//! **A client works the audience out from the URL it is talking to, and never
//! asks the deployment for it.** A deployment that could name its audience
//! could name another deployment's, and be handed a token that one accepts;
//! derived from the URL, the token a client gives to a host is only ever good
//! at the deployment that calls itself by that host. So [`deployment_audience`]
//! is a contract with `sdk/cli` (`oidcAudience`) and `sdk/setup-oxyc`
//! (`audienceFor`): the three must agree on every URL, and each repeats the
//! cases in the test below. The clients use the WHATWG URL's `host`, which is
//! what this reproduces — lowercased, default port dropped.
//!
//! Never [`AUDIENCE_PUBLISH`](super::claims::AUDIENCE_PUBLISH): every value
//! here is `oxy` or starts `oxy:`, so a token requested for a publish cannot be
//! traded for trusted access on any deployment, nor the reverse.

use super::claims::AUDIENCE_OXY;

/// The port a scheme implies, which a URL's `host` therefore leaves out.
fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "https" | "wss" => Some(443),
        "http" | "ws" => Some(80),
        _ => None,
    }
}

/// `host` and `port` of an authority with no userinfo: `app.example:8443`,
/// `[::1]:3000`. A bracketed IPv6 literal keeps its brackets.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    let after_host = match authority.strip_prefix('[') {
        Some(rest) => rest.find(']').map(|end| end + 2),
        None => authority.find(':'),
    };
    match after_host {
        Some(at) if at <= authority.len() => {
            let (host, rest) = authority.split_at(at);
            (host, rest.strip_prefix(':').filter(|port| !port.is_empty()))
        }
        _ => (authority, None),
    }
}

/// The `host` of a base URL as a WHATWG URL reports it: lowercased, with its
/// port only when that is not the scheme's default. `None` when there is none.
fn host_of(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    let (scheme, rest) = trimmed
        .split_once("://")
        .map_or(("", trimmed), |(scheme, rest)| (scheme, rest));
    // Up to the path, the query or the fragment; then past any `user@`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_userinfo, host)| host);
    let (host, port) = split_port(authority);
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let implied = default_port(&scheme.to_ascii_lowercase());
    Some(match port.map(str::parse::<u16>) {
        Some(Ok(port)) if Some(port) != implied => format!("{host}:{port}"),
        // No port, the scheme's own, or one that is not a port at all.
        _ => host,
    })
}

/// The audience a deployment whose public base URL is `public_base_url`
/// requires of a trusted-access token — and the audience a client talking to
/// that URL asks GitHub for. `None` — no URL configured — is plain
/// [`AUDIENCE_OXY`].
pub fn deployment_audience(public_base_url: Option<&str>) -> String {
    match public_base_url.and_then(host_of) {
        Some(host) => format!("{AUDIENCE_OXY}:{host}"),
        None => AUDIENCE_OXY.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_oidc::claims::AUDIENCE_PUBLISH;

    /// URL → audience. **The same list is in `sdk/cli/src/auth/oidc.test.ts`
    /// and `sdk/setup-oxyc/test/main.test.mjs`**: the clients derive the
    /// audience from the URL they talk to, the server from the URL it calls
    /// itself by, and the three must never disagree. Change one, change all.
    const CASES: &[(&str, &str)] = &[
        // The three deployments, as `oxyc --env` and `OXY_API_URL` name them.
        ("https://app.oxygen-hq.com", "oxy:app.oxygen-hq.com"),
        ("https://aip.dev.oxy.tech", "oxy:aip.dev.oxy.tech"),
        ("https://aip.staging.oxy.tech", "oxy:aip.staging.oxy.tech"),
        // The scheme's default port is dropped…
        ("https://app.oxygen-hq.com:443", "oxy:app.oxygen-hq.com"),
        ("http://app.oxygen-hq.com:80", "oxy:app.oxygen-hq.com"),
        // …and any other port is kept: it is part of which deployment it is.
        ("http://localhost:3000", "oxy:localhost:3000"),
        (
            "https://app.oxygen-hq.com:8443",
            "oxy:app.oxygen-hq.com:8443",
        ),
        ("http://localhost:443", "oxy:localhost:443"),
        // The host is lowercased.
        ("https://App.Oxygen-HQ.com", "oxy:app.oxygen-hq.com"),
        // Only the address counts: not a path such as `/api`, nor a slash.
        ("https://app.oxygen-hq.com/api", "oxy:app.oxygen-hq.com"),
        ("https://app.oxygen-hq.com/", "oxy:app.oxygen-hq.com"),
        ("https://app.oxygen-hq.com/api/", "oxy:app.oxygen-hq.com"),
        (
            "https://app.oxygen-hq.com:443/api?x=1#y",
            "oxy:app.oxygen-hq.com",
        ),
        // An IPv6 literal keeps its brackets.
        ("http://[::1]:3000", "oxy:[::1]:3000"),
        ("https://[2001:db8::1]", "oxy:[2001:db8::1]"),
        ("https://[2001:db8::1]:443/api", "oxy:[2001:db8::1]"),
        // Credentials in a URL are no part of the address.
        (
            "https://user:secret@app.oxygen-hq.com",
            "oxy:app.oxygen-hq.com",
        ),
    ];

    #[test]
    fn a_deployments_audience_is_its_host_as_a_client_derives_it() {
        for (url, audience) in CASES {
            assert_eq!(deployment_audience(Some(url)), *audience, "{url}");
        }
    }

    #[test]
    fn two_deployments_never_share_one() {
        // The point of the whole thing: a token asked for staging is not one
        // production accepts.
        let production = deployment_audience(Some("https://app.oxygen-hq.com"));
        let staging = deployment_audience(Some("https://aip.staging.oxy.tech"));
        assert_ne!(production, staging);
        assert_ne!(production, AUDIENCE_OXY, "nor the audience of a dev box");
    }

    #[test]
    fn a_deployment_with_no_public_url_uses_the_plain_one() {
        for url in [None, Some(""), Some("   "), Some("https://"), Some("/")] {
            assert_eq!(deployment_audience(url), AUDIENCE_OXY, "{url:?}");
        }
    }

    #[test]
    fn it_is_never_the_publish_exchanges_audience() {
        // Whatever a deployment is called — `publish` included.
        for url in [
            None,
            Some("https://publish"),
            Some("https://-publish"),
            Some("https://oxy-publish"),
        ] {
            let audience = deployment_audience(url);
            assert_ne!(audience, AUDIENCE_PUBLISH, "{url:?}");
            assert!(
                audience == AUDIENCE_OXY || audience.starts_with("oxy:"),
                "{audience}"
            );
        }
    }
}
