//! Validator helpers for the two surfaces that serve an HTML shell with
//! `Cache-Control: no-cache` — the admin SPA (`oxy-server`'s `commands::serve`) and
//! custom-app bundles (`oxy_api_custom_apps::serve`).
//!
//! Both must revalidate on every load, so both need a validator, and a
//! validator format that drifts between them is a bug that only shows up as
//! "revalidation stopped working on one surface". One home, one format.

use axum::http::{HeaderMap, header};

/// Weak ETag over the final response bytes.
///
/// Weak (`W/`) because the bytes are produced by a transform — org injection
/// for the admin SPA, base-path rewriting plus `window.__OXY_APP__` injection
/// for a bundle — not read verbatim off a file.
///
/// `DefaultHasher` is deterministic (SipHash with fixed keys, not the
/// randomized `RandomState`), so every replica running the same binary derives
/// the same validator for the same bytes. That is the property a fleet behind
/// a load balancer needs: a revalidation must not turn into a full 200 just
/// because it landed on a different pod.
pub fn weak_etag(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("W/\"{:016x}\"", hasher.finish())
}

/// True when an `If-None-Match` header value already holds `etag`.
///
/// Handles the comma-separated list form and the `*` wildcard (RFC 9110
/// §13.1.2), which a conditional request may legitimately send.
pub fn if_none_match_matches(header_value: &str, etag: &str) -> bool {
    header_value
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate == etag)
}

/// [`if_none_match_matches`] against a request's headers.
pub fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| if_none_match_matches(value, etag))
}

/// Returns true if the client wants HTML — used to scope the SPA fallback
/// (`bundle_dir/index.html`) to top-level navigations only. Browsers send
/// `Accept: text/html,application/xhtml+xml,...` for navigations; `fetch()`
/// and asset XHRs send `Accept: */*` or a specific MIME type. We deliberately
/// do NOT treat `*/*` as HTML, because that's exactly the case the reviewer
/// wanted to 404 cleanly (asset for a missing file shouldn't get the SPA
/// shell back at 200). Missing Accept is rare in practice (manual `curl`
/// without flags); we treat it as HTML so the SPA still renders.
pub fn wants_html(headers: &HeaderMap) -> bool {
    let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let accept = accept.to_ascii_lowercase();
    accept.contains("text/html") || accept.contains("application/xhtml+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_stable_and_content_sensitive() {
        assert_eq!(
            weak_etag(b"<html>hello</html>"),
            weak_etag(b"<html>hello</html>")
        );
        assert_ne!(
            weak_etag(b"<html>hello</html>"),
            weak_etag(b"<html>world</html>")
        );
        assert!(weak_etag(b"x").starts_with("W/\""));
    }

    #[test]
    fn if_none_match_handles_lists_and_wildcard() {
        assert!(if_none_match_matches("W/\"abc\"", "W/\"abc\""));
        assert!(if_none_match_matches("W/\"other\", W/\"abc\"", "W/\"abc\""));
        assert!(if_none_match_matches("*", "W/\"abc\""));
        assert!(!if_none_match_matches("W/\"other\"", "W/\"abc\""));
    }
}
