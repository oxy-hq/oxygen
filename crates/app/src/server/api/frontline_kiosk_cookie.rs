//! The two cookies an enrolled kiosk carries, and the one list of attributes
//! they share.
//!
//! - **`oxy_kiosk`** — `<device id>.<secret>`, HttpOnly. The credential a PIN
//!   is only ever usable beside ([`super::frontline_devices`]).
//! - **`oxy_kiosk_hint`** — `1`, readable by the page. It tells the web app, on
//!   every host the kiosk cookie covers, that this browser may be a kiosk, so
//!   its sign-out check (`useKioskSessionGuard`) knows to ask
//!   `GET /api/frontline/device`. It is a reason to ask, never the answer: the
//!   probe still decides, and it carries nothing the page could not already
//!   learn by asking.
//!
//! Why the hint exists: the web app keeps an admin's bearer token in
//! `localStorage`, and a custom app's idle sign-out clears only the session
//! cookie, so on a kiosk the web app must notice a sign-in the cookie no longer
//! backs. It only checks on a browser it has reason to think is a kiosk — a
//! browser that was never one makes no extra call — and its first reason, a
//! flag in `localStorage`, is per origin and set only by pages an org
//! subdomain never shows. The hint is set with the kiosk cookie's own
//! `Domain`, so it reaches `<org>.oxygen-hq.com` too.
//!
//! The two are set together (binding), cleared together (leaving, or revoking
//! this browser's own kiosk), and share every attribute but `HttpOnly` — a
//! clearing header that drifted from the setting one by one attribute would
//! leave the cookie exactly where it was.

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use tracing::warn;
use uuid::Uuid;

/// The cookie an enrolled kiosk carries: `<device id>.<secret>`.
pub const KIOSK_COOKIE_NAME: &str = "oxy_kiosk";
/// Page-readable: "this browser may be a kiosk — ask".
pub const KIOSK_HINT_COOKIE_NAME: &str = "oxy_kiosk_hint";
/// A bound device stays bound for a year of calendar time; a lost tablet is
/// handled by revocation, not by expiry. The hint lives exactly as long.
const DEVICE_COOKIE_MAX_AGE_SECS: i64 = 365 * 24 * 60 * 60;

/// A cookie's value, if the request carries a non-empty one. Same RFC 6265
/// split as `oxy_auth::built_in::extract_session_cookie`, for the same reason
/// it exists there: callers drifted on the empty-value guard.
fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    for value in headers.get_all(header::COOKIE).iter() {
        let Ok(raw) = value.to_str() else { continue };
        for part in raw.split(';') {
            if let Some(v) = part.trim().strip_prefix(prefix.as_str())
                && !v.is_empty()
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// The `oxy_kiosk` value, if the request carries one.
pub fn extract_kiosk_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, KIOSK_COOKIE_NAME)
}

/// The device id and secret the kiosk cookie names, when it is well formed.
/// Says nothing about whether they are real — only `bound_device` can.
pub(crate) fn kiosk_cookie_parts(headers: &HeaderMap) -> Option<(Uuid, String)> {
    let raw = extract_kiosk_cookie(headers)?;
    let (id, secret) = raw.split_once('.')?;
    Some((Uuid::parse_str(id).ok()?, secret.to_string()))
}

/// Whether the request already carries the hint, so a probe need not set it.
pub(crate) fn carries_kiosk_hint(headers: &HeaderMap) -> bool {
    extract_cookie(headers, KIOSK_HINT_COOKIE_NAME).as_deref() == Some("1")
}

/// Which of the two cookies a `Set-Cookie` is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KioskCookie {
    Device,
    Hint,
}

impl KioskCookie {
    fn name(self) -> &'static str {
        match self {
            KioskCookie::Device => KIOSK_COOKIE_NAME,
            KioskCookie::Hint => KIOSK_HINT_COOKIE_NAME,
        }
    }
}

/// The attributes every kiosk `Set-Cookie` carries — setting and clearing,
/// device and hint alike, except that only the device secret is HttpOnly.
fn kiosk_cookie_attributes(kind: KioskCookie, secure: bool, domain: Option<&str>) -> Vec<String> {
    let mut parts = vec!["Path=/".to_string()];
    if kind == KioskCookie::Device {
        parts.push("HttpOnly".to_string());
    }
    parts.push("SameSite=Lax".to_string());
    if secure {
        parts.push("Secure".to_string());
    }
    if let Some(domain) = domain {
        parts.push(format!("Domain={domain}"));
    }
    parts
}

/// Same `Domain` rule as the session cookie, or the two would disagree on
/// which hosts a kiosk is a kiosk for.
fn kiosk_cookie_domain() -> Option<String> {
    let domain = std::env::var("OXY_SESSION_COOKIE_DOMAIN").ok()?;
    let domain = domain.trim();
    (!domain.is_empty()).then(|| domain.to_string())
}

fn kiosk_set_cookie(
    kind: KioskCookie,
    value: &str,
    max_age_secs: i64,
    secure: bool,
    domain: Option<&str>,
) -> String {
    let mut parts = vec![
        format!("{}={value}", kind.name()),
        format!("Max-Age={max_age_secs}"),
    ];
    parts.extend(kiosk_cookie_attributes(kind, secure, domain));
    parts.join("; ")
}

fn live(kind: KioskCookie, value: &str, secure: bool) -> String {
    let domain = kiosk_cookie_domain();
    kiosk_set_cookie(
        kind,
        value,
        DEVICE_COOKIE_MAX_AGE_SECS,
        secure,
        domain.as_deref(),
    )
}

/// What binding a tablet sets: the kiosk cookie holding `value`, and the hint.
pub(crate) fn kiosk_cookies(value: &str, secure: bool) -> [String; 2] {
    [
        live(KioskCookie::Device, value, secure),
        kiosk_hint_cookie(secure),
    ]
}

/// The hint alone — what a probe hands a kiosk bound before the hint existed.
pub(crate) fn kiosk_hint_cookie(secure: bool) -> String {
    live(KioskCookie::Hint, "1", secure)
}

/// The pair of `Set-Cookie`s that take both off a browser: empty values that
/// expire now, under exactly the attributes they were set with.
pub(crate) fn clear_kiosk_cookies(secure: bool) -> [String; 2] {
    let domain = kiosk_cookie_domain();
    [KioskCookie::Device, KioskCookie::Hint]
        .map(|kind| kiosk_set_cookie(kind, "", 0, secure, domain.as_deref()))
}

/// Every value as a header, or `None` when the header layer rejects any —
/// which only a malformed `OXY_SESSION_COOKIE_DOMAIN` can cause. All or none,
/// so a response never sets one cookie of the pair without the other.
pub(crate) fn set_cookie_values(cookies: &[String]) -> Option<Vec<HeaderValue>> {
    cookies
        .iter()
        .map(|c| HeaderValue::from_str(c).ok())
        .collect()
}

pub(crate) fn append_set_cookies(resp: &mut Response, values: Vec<HeaderValue>) {
    for value in values {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// Expire both cookies on `resp`. A rejected header is logged rather than
/// failed: every caller has revoked the row by now, and `bound_device` ignores
/// a revoked row, so a kiosk cookie left behind is inert — a hint left behind
/// only keeps the web app's sign-out check asking.
pub(crate) fn clear_kiosk_cookies_on(resp: &mut Response, secure: bool) {
    match set_cookie_values(&clear_kiosk_cookies(secure)) {
        Some(values) => append_set_cookies(resp, values),
        None => warn!(
            "kiosk revoked but its clearing cookies were rejected — check \
             OXY_SESSION_COOKIE_DOMAIN; the kiosk cookie left behind is inert"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cookie: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        h
    }

    #[test]
    fn the_kiosk_cookie_is_read_beside_the_session_cookie() {
        assert_eq!(
            extract_kiosk_cookie(&headers("oxy_session=jwt; oxy_kiosk=abc.def; x=y")).as_deref(),
            Some("abc.def")
        );
        assert!(extract_kiosk_cookie(&headers("oxy_session=jwt")).is_none());
        // An empty value is no value — the guard callers drifted on before.
        assert!(extract_kiosk_cookie(&headers("oxy_kiosk=; oxy_session=jwt")).is_none());
        // The hint is a different cookie, never mistaken for the kiosk one.
        assert!(extract_kiosk_cookie(&headers("oxy_kiosk_hint=1")).is_none());
    }

    #[test]
    fn the_cookie_parts_are_an_id_and_a_secret_or_nothing() {
        let id = Uuid::new_v4();
        assert_eq!(
            kiosk_cookie_parts(&headers(&format!("oxy_kiosk={id}.s3cret"))),
            Some((id, "s3cret".to_string()))
        );
        assert!(kiosk_cookie_parts(&headers("oxy_kiosk=garbage")).is_none());
        assert!(kiosk_cookie_parts(&headers("oxy_kiosk=not-a-uuid.s")).is_none());
        assert!(kiosk_cookie_parts(&HeaderMap::new()).is_none());
    }

    #[test]
    fn the_hint_is_read_only_when_it_says_one() {
        assert!(carries_kiosk_hint(&headers(
            "oxy_kiosk=a.b; oxy_kiosk_hint=1"
        )));
        assert!(!carries_kiosk_hint(&headers("oxy_kiosk=a.b")));
        assert!(!carries_kiosk_hint(&headers("oxy_kiosk_hint=")));
    }

    #[test]
    fn the_cookie_carries_the_attributes_the_session_cookie_does() {
        let [c, _] = kiosk_cookies("id.secret", true);
        for part in [
            "oxy_kiosk=id.secret",
            "Path=/",
            "HttpOnly",
            "SameSite=Lax",
            "Secure",
        ] {
            assert!(c.contains(part), "{c} lacks {part}");
        }
        assert!(!kiosk_cookies("id.secret", false)[0].contains("Secure"));
    }

    /// Everything but the name, value and lifetime, as a set.
    fn attributes(set_cookie: &str) -> std::collections::BTreeSet<String> {
        set_cookie
            .split("; ")
            .skip(1)
            .filter(|p| !p.starts_with("Max-Age="))
            .map(str::to_string)
            .collect()
    }

    /// The page reads the hint, so it may not be HttpOnly — and that is the one
    /// attribute it may not share, or the hint would stop reaching a host the
    /// kiosk cookie reaches (or outlive it).
    #[test]
    fn the_hint_is_the_kiosk_cookie_readable_by_the_page() {
        for secure in [true, false] {
            for domain in [None, Some(".oxygen-hq.com")] {
                let device = kiosk_set_cookie(
                    KioskCookie::Device,
                    "id.secret",
                    DEVICE_COOKIE_MAX_AGE_SECS,
                    secure,
                    domain,
                );
                let hint = kiosk_set_cookie(
                    KioskCookie::Hint,
                    "1",
                    DEVICE_COOKIE_MAX_AGE_SECS,
                    secure,
                    domain,
                );
                assert!(hint.starts_with("oxy_kiosk_hint=1; "), "{hint}");
                assert!(!hint.contains("HttpOnly"), "{hint}");
                let mut expected = attributes(&device);
                expected.remove("HttpOnly");
                assert_eq!(attributes(&hint), expected, "{hint}");
                assert!(
                    hint.contains(&format!("Max-Age={DEVICE_COOKIE_MAX_AGE_SECS}")),
                    "{hint}"
                );
            }
        }
        assert_eq!(kiosk_cookies("id.secret", true)[1], kiosk_hint_cookie(true));
    }

    /// A browser only overwrites a cookie whose `Domain` and `Path` match the
    /// one it holds, so each clearing header must carry exactly what its
    /// setting header did — or "Leave kiosk mode" answers 204 and the tablet
    /// stays one.
    #[test]
    fn each_clearing_cookie_carries_exactly_the_attributes_that_set_it() {
        for kind in [KioskCookie::Device, KioskCookie::Hint] {
            for secure in [true, false] {
                for domain in [None, Some(".oxygen-hq.com")] {
                    let set =
                        kiosk_set_cookie(kind, "v", DEVICE_COOKIE_MAX_AGE_SECS, secure, domain);
                    let clear = kiosk_set_cookie(kind, "", 0, secure, domain);
                    assert_eq!(attributes(&set), attributes(&clear), "{kind:?} {domain:?}");
                    assert!(clear.starts_with(&format!("{}=; ", kind.name())), "{clear}");
                    assert!(clear.contains("Max-Age=0"), "{clear}");
                }
            }
        }
        // And the pairs the handlers actually send, under whatever this
        // process's `OXY_SESSION_COOKIE_DOMAIN` is.
        for secure in [true, false] {
            let set = kiosk_cookies("id.secret", secure);
            let clear = clear_kiosk_cookies(secure);
            for (s, c) in set.iter().zip(clear.iter()) {
                assert_eq!(attributes(s), attributes(c));
                assert_eq!(s.split('=').next(), c.split('=').next());
            }
        }
    }

    #[test]
    fn clearing_puts_both_cookies_on_the_response() {
        let mut resp = Response::new(axum::body::Body::empty());
        clear_kiosk_cookies_on(&mut resp, true);
        let set: Vec<_> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(set, clear_kiosk_cookies(true).to_vec());
    }
}
