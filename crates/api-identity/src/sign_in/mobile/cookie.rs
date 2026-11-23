//! The `oxy_session` cookie, described for a native app to set itself.
//!
//! A WebView's cookie jar starts empty, so a custom app opened in one would
//! bounce to the login page. The apps put the session there themselves
//! (`WKHTTPCookieStore` on iOS, `CookieManager` on Android) before loading
//! one; no web endpoint turns a bearer into a cookie, which would be a
//! login-CSRF surface. What they set must be the cookie a browser login sets,
//! so the description is read off the same attributes the server's own
//! `Set-Cookie` is rendered from (`ops::session_cookie_attributes`).

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::Serialize;

use oxy_app_core::session::{
    SessionCookieAttributes, is_request_secure, session_cookie_attributes,
};

/// Every attribute of the session cookie; its value is the session token.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SessionCookie {
    pub name: &'static str,
    /// The cookie's `Domain` (`.oxygen-hq.com` in production), or `null` for
    /// a host-only cookie on the API's host.
    pub domain: Option<String>,
    pub path: &'static str,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: &'static str,
    /// The token's own expiry: the cookie lapses with it, never after.
    pub expires_at: DateTime<Utc>,
}

impl SessionCookie {
    fn of(attrs: SessionCookieAttributes, expires_at: DateTime<Utc>) -> Self {
        Self {
            name: attrs.name,
            domain: attrs.domain,
            path: attrs.path,
            secure: attrs.secure,
            http_only: attrs.http_only,
            same_site: attrs.same_site,
            expires_at,
        }
    }

    /// The cookie this request's login would have been handed, expiring at
    /// `expires_at`. `Secure` is decided as every login decides it.
    pub(super) fn for_request(headers: &HeaderMap, expires_at: DateTime<Utc>) -> Self {
        Self::of(
            session_cookie_attributes(is_request_secure(headers)),
            expires_at,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use oxy_app_core::session::render_session_cookie;

    fn attrs(secure: bool, domain: Option<&str>) -> SessionCookieAttributes {
        SessionCookieAttributes {
            domain: domain.map(str::to_string),
            ..session_cookie_attributes(secure)
        }
    }

    /// The attributes of a rendered `Set-Cookie`, keyed lowercase.
    fn parsed(set_cookie: &str) -> Vec<(String, Option<String>)> {
        set_cookie
            .split("; ")
            .skip(1)
            .map(|part| match part.split_once('=') {
                Some((k, v)) => (k.to_ascii_lowercase(), Some(v.to_string())),
                None => (part.to_ascii_lowercase(), None),
            })
            .collect()
    }

    fn attr<'a>(parts: &'a [(String, Option<String>)], key: &str) -> Option<Option<&'a str>> {
        parts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_deref())
    }

    #[test]
    fn the_description_matches_the_cookie_the_server_sets() {
        let expires_at = Utc.with_ymd_and_hms(2026, 11, 15, 12, 0, 0).unwrap();
        for (secure, domain) in [
            (true, Some(".oxygen-hq.com")),
            (false, Some(".oxygen-hq.com")),
            (true, None),
            (false, None),
        ] {
            let set_cookie = render_session_cookie(&attrs(secure, domain), "a.b.c", 3600);
            let described = SessionCookie::of(attrs(secure, domain), expires_at);
            let parts = parsed(&set_cookie);

            assert!(set_cookie.starts_with(&format!("{}=a.b.c; ", described.name)));
            assert_eq!(described.name, "oxy_session");
            assert_eq!(attr(&parts, "path"), Some(Some(described.path)));
            assert_eq!(
                attr(&parts, "domain").flatten(),
                described.domain.as_deref()
            );
            assert_eq!(attr(&parts, "secure").is_some(), described.secure);
            assert_eq!(attr(&parts, "httponly").is_some(), described.http_only);
            assert_eq!(attr(&parts, "samesite"), Some(Some(described.same_site)));
            assert_eq!(described.expires_at, expires_at);
        }
    }

    #[test]
    fn the_server_cookie_is_rendered_from_the_same_attributes() {
        for secure in [true, false] {
            assert_eq!(
                oxy_app_core::session::build_session_cookie_with_max_age("a.b.c", secure, 60),
                render_session_cookie(&session_cookie_attributes(secure), "a.b.c", 60)
            );
        }
    }

    #[test]
    fn it_serializes_as_the_mobile_contract_names_it() {
        let expires_at = Utc.with_ymd_and_hms(2026, 11, 15, 12, 0, 0).unwrap();
        let described = SessionCookie::of(attrs(true, Some(".oxygen-hq.com")), expires_at);
        let json = serde_json::to_value(&described).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "name": "oxy_session",
                "domain": ".oxygen-hq.com",
                "path": "/",
                "secure": true,
                "http_only": true,
                "same_site": "Lax",
                "expires_at": "2026-11-15T12:00:00Z",
            })
        );
        let host_only = SessionCookie::of(attrs(false, None), expires_at);
        let json = serde_json::to_value(&host_only).expect("serialize");
        assert_eq!(json["domain"], serde_json::Value::Null);
        assert_eq!(json["secure"], false);
    }
}
