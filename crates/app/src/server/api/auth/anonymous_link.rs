//! The origin of a link that an ANONYMOUS request makes Oxygen mail: the
//! magic-link sign-in (`POST /api/auth/magic-link/request`).
//!
//! The mail is genuinely Oxygen's, so its recipient trusts the host in it, and
//! a click hands that host a one-time sign-in token. So the host must be this
//! deployment's, never one the request named: `Origin` and `Referer` are as
//! forgeable as `Host` by any client that is not a browser, and CORS only
//! decides whether a browser may read the response. A link a signed-in caller
//! sends goes through `ops::extract_link_base_for_authenticated_request`.

use axum::http::HeaderMap;
use url::Url;

use super::ops::{LOCAL_DEFAULT_BASE, origin_named_by_request, pin_org_subdomain_to_app_host};

/// The sign-in link a magic-link mail carries. Only [`magic_link_for`] makes
/// one, and the mailer takes nothing else, so the handler cannot mail a link
/// built some other way, and the tests here check the link it mails.
pub(super) struct SignInLink(String);

impl SignInLink {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The link the magic-link handler mails: the callback on
/// [`link_base_for_anonymous_request`], carrying the token and the
/// already-validated `return_to` when there is one.
pub(super) fn magic_link_for(
    headers: &HeaderMap,
    token: &str,
    return_to: Option<&str>,
) -> SignInLink {
    let base = link_base_for_anonymous_request(headers);
    SignInLink(match return_to {
        Some(target) => format!(
            "{base}/auth/magic-link/callback?token={token}&return_to={}",
            urlencoding::encode(target)
        ),
        None => format!("{base}/auth/magic-link/callback?token={token}"),
    })
}

/// The base of the link: the deployment's configured public URL
/// (`OXY_API_URL`, as `admin_base_url` reads it), unless the deployment is a
/// local stack.
///
/// - **Configured and not loopback** (every cloud deployment): that URL,
///   whatever the request says. A sign-in started on an org subdomain lands on
///   it too, as `pin_org_subdomain_to_app_host` always made it.
/// - **Configured loopback, or unset** (a laptop): a loopback `Origin` or
///   `Referer` is believed, because Vite serves the SPA on another port
///   (`:5173`) than the API (`:3000`) and the link must open the SPA. Any other
///   origin is ignored for the configured URL, or with none for
///   `http://localhost:3000`. Either way the link reaches only the recipient's
///   own machine.
fn link_base_for_anonymous_request(headers: &HeaderMap) -> String {
    let configured = oxy_app_core::custom_apps_host_dispatch::admin_base_url();
    choose_base(origin_named_by_request(headers).as_deref(), configured)
}

fn choose_base(named: Option<&str>, configured: Option<String>) -> String {
    let local_stack = configured
        .as_deref()
        .is_none_or(|c| loopback_base(c).is_some());
    if local_stack && let Some(local) = named.and_then(loopback_base) {
        return pin_org_subdomain_to_app_host(local);
    }
    let base = configured
        .clone()
        .unwrap_or_else(|| LOCAL_DEFAULT_BASE.to_string());
    if let Some(named) = named.filter(|n| !is_this_deployment(n, configured.as_deref())) {
        tracing::warn!(
            requested = named,
            link_base = %base,
            "magic-link request named a host that is not this deployment's (OXY_API_URL); \
             the emailed link ignores it"
        );
    }
    base
}

/// `scheme://host[:port]` of `base` when it names this machine, rebuilt from
/// the parsed URL so the link says exactly what was checked:
/// `http://localhost@evil.example` parses to the host `evil.example`, and is
/// refused.
fn loopback_base(base: &str) -> Option<String> {
    let url = Url::parse(base).ok()?;
    let host = url.host_str()?;
    let web = matches!(url.scheme(), "http" | "https");
    if !web || !crate::server::router::is_loopback_host(host) {
        return None;
    }
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{}://{host}{port}", url.scheme()))
}

/// Whether `named` is a host this deployment answers on: the configured one,
/// or one of its org subdomains. Only decides whether to log.
fn is_this_deployment(named: &str, configured: Option<&str>) -> bool {
    let host = |base: &str| Some(Url::parse(base).ok()?.host_str()?.to_ascii_lowercase());
    let Some(named) = host(named) else {
        return false;
    };
    configured.and_then(host).as_deref() == Some(named.as_str())
        || oxy_app_core::org_host_dispatch::parse_org_subdomain(&named).is_some()
}

#[cfg(test)]
mod tests {
    use super::magic_link_for;
    use axum::http::HeaderMap;
    use std::collections::HashMap;
    use url::Url;

    const PROD_API_URL: &str = "https://app.oxygen-hq.com/api";
    const APP: &str = "https://app.oxygen-hq.com";
    const FORGED: &str = "https://oxygen-login.evil.example";

    /// Foreign hosts dressed as this deployment's or as loopback. A check that
    /// read the string, not the host a browser would use, would believe them.
    const LOOK_ALIKES: &[&str] = &[
        "https://app.oxygen-hq.com.evil.example",
        "https://app.oxygen-hq.com@evil.example",
        "https://evil.example#.app.oxygen-hq.com",
        "http://localhost.evil.example",
        "http://127.0.0.1.evil.example",
        "http://localhost@evil.example",
        "http://localhost:5173@evil.example",
        // A trailing dot.
        "http://localhost.evil.example.",
        // An uppercase scheme.
        "HTTP://localhost.evil.example",
        "null",
        // Scheme-less, as some proxies forward `Origin`.
        "app.oxygen-hq.com.evil.example",
        "localhost.evil.example",
        "localhost:5173@evil.example",
    ];

    /// The link a sign-in mail carries on a deployment whose `OXY_API_URL` is
    /// `api_url` (`None`: unset), for a request carrying `request`, built by
    /// the function the handler mails through. nextest runs each test in its
    /// own process, so these env writes don't leak.
    fn emailed_link(
        api_url: Option<&str>,
        request: &[(&'static str, &str)],
        return_to: Option<&str>,
    ) -> Url {
        unsafe {
            for var in [
                "OXY_API_URL",
                "OXY_ORG_SUBDOMAIN_ZONE",
                "OXY_CUSTOM_APPS_ZONE",
                "OXY_RESERVED_SUBDOMAINS",
            ] {
                std::env::remove_var(var);
            }
            if let Some(url) = api_url {
                std::env::set_var("OXY_API_URL", url);
            }
        }
        let mut headers = HeaderMap::new();
        for (name, value) in request {
            headers.insert(*name, value.parse().expect("a valid header value"));
        }
        let link = magic_link_for(&headers, "t0k3n", return_to);
        Url::parse(link.as_str()).expect("the link parses")
    }

    /// Where a click on `link` goes: what a browser would send the token to.
    fn lands_on(link: &Url) -> String {
        link.origin().ascii_serialization()
    }

    #[test]
    fn a_forged_origin_never_reaches_the_link() {
        let link = emailed_link(Some(PROD_API_URL), &[("origin", FORGED)], None);
        assert_eq!(lands_on(&link), APP);
    }

    #[test]
    fn a_forged_referer_never_reaches_the_link() {
        let referer = format!("{FORGED}/login");
        let link = emailed_link(Some(PROD_API_URL), &[("referer", &referer)], None);
        assert_eq!(lands_on(&link), APP);
    }

    #[test]
    fn an_origin_that_only_looks_like_ours_is_refused() {
        for forged in LOOK_ALIKES {
            let link = emailed_link(Some(PROD_API_URL), &[("origin", *forged)], None);
            assert_eq!(lands_on(&link), APP, "{forged}");
        }
    }

    /// A local stack is the one place the request's origin is weighed at all,
    /// so this is where a look-alike reaches the code that judges it.
    #[test]
    fn a_local_stack_judges_a_look_alike_by_its_real_host() {
        // Every leak is collected, so a failure names each look-alike let through.
        let mut leaked = Vec::new();
        for (api_url, expected) in [
            (None, "http://localhost:3000"),
            (Some("http://localhost:3100/api"), "http://localhost:3100"),
        ] {
            for header in ["origin", "referer"] {
                for forged in LOOK_ALIKES {
                    let landed = lands_on(&emailed_link(api_url, &[(header, *forged)], None));
                    if landed != expected {
                        leaked.push(format!(
                            "OXY_API_URL={api_url:?} {header}: {forged} -> {landed}"
                        ));
                    }
                }
            }
        }
        assert!(leaked.is_empty(), "{leaked:#?}");
    }

    #[test]
    fn a_loopback_origin_is_not_this_deployment_in_production() {
        let link = emailed_link(
            Some(PROD_API_URL),
            &[("origin", "http://localhost:5173")],
            None,
        );
        assert_eq!(lands_on(&link), APP);
    }

    #[test]
    fn a_request_naming_no_origin_gets_the_configured_host() {
        let link = emailed_link(Some(PROD_API_URL), &[], None);
        assert_eq!(lands_on(&link), APP);
    }

    #[test]
    fn a_sign_in_from_an_org_subdomain_links_to_the_app_host() {
        let link = emailed_link(
            Some(PROD_API_URL),
            &[("origin", "https://pokehouse.oxygen-hq.com")],
            None,
        );
        assert_eq!(lands_on(&link), APP);
    }

    #[test]
    fn return_to_rides_along_to_the_callback() {
        let target = "https://pokehouse.oxygen-hq.com/a/store-ops/?tab=today";
        let link = emailed_link(Some(PROD_API_URL), &[("origin", APP)], Some(target));
        assert_eq!(lands_on(&link), APP);
        assert_eq!(link.path(), "/auth/magic-link/callback");
        let query: HashMap<String, String> = link.query_pairs().into_owned().collect();
        assert_eq!(query.get("token").map(String::as_str), Some("t0k3n"));
        assert_eq!(query.get("return_to").map(String::as_str), Some(target));
    }

    #[test]
    fn local_dev_keeps_the_vite_origin() {
        // Vite serves the SPA on another port than the API, and the link must
        // open the SPA.
        for api_url in [Some("http://localhost:3000/api"), None] {
            for vite in [
                "http://localhost:5173",
                "http://127.0.0.1:5273",
                "http://[::1]:5173",
            ] {
                let link = emailed_link(api_url, &[("origin", vite)], None);
                assert_eq!(lands_on(&link), vite, "OXY_API_URL={api_url:?}");
            }
        }
    }

    #[test]
    fn a_local_stack_never_links_off_the_machine() {
        for (api_url, expected) in [
            (Some("http://localhost:3100/api"), "http://localhost:3100"),
            (None, "http://localhost:3000"),
        ] {
            let link = emailed_link(api_url, &[("origin", FORGED)], None);
            assert_eq!(lands_on(&link), expected, "OXY_API_URL={api_url:?}");
        }
    }

    #[test]
    fn a_local_stack_naming_no_origin_gets_its_configured_port() {
        let link = emailed_link(Some("http://localhost:3100/api"), &[], None);
        assert_eq!(lands_on(&link), "http://localhost:3100");
    }
}
