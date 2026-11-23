//! Promote `?api_key=…` from the URL into the `X-API-Key` request header.
//!
//! `EventSource` (browser SSE) cannot attach custom headers, so the only
//! reliable way to authenticate an SSE stream is via a query parameter.
//! This middleware sits ahead of the auth gate and copies a query-string
//! `api_key` value into a header before the standard auth path runs —
//! existing handlers, middlewares, and the `X-API-Key` authentication
//! flow stay untouched.
//!
//! Header wins on conflict: an explicit `X-API-Key` header is not
//! overwritten. After promotion the `api_key` query parameter is stripped
//! from the request URI so the secret never reaches request logging, the
//! `TraceLayer` span, or any downstream that echoes the URL.

use axum::body::Body;
use axum::extract::Request;
use axum::http::uri::PathAndQuery;
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use oxy_auth::constants::DEFAULT_API_KEY_HEADER;

pub async fn api_key_query_middleware(mut request: Request<Body>, next: Next) -> Response {
    let header_name = HeaderName::from_static("x-api-key");
    debug_assert_eq!(header_name.as_str(), DEFAULT_API_KEY_HEADER.to_lowercase());

    let Some(query) = request.uri().query().map(str::to_string) else {
        return next.run(request).await;
    };
    let Some(api_key) = extract_api_key(&query) else {
        return next.run(request).await;
    };
    // A sandbox agent token never travels in a URL: it is not promoted, and
    // the request is refused here whatever else it carries (sandbox agent
    // credential design §3.1). The value is left out of the log.
    if api_key.starts_with(oxy_auth::token::format::SBX_PREFIX) {
        tracing::warn!("sandbox agent token presented as ?api_key= — 401");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Promote the value into the X-API-Key header (an explicit header wins).
    if !request.headers().contains_key(&header_name)
        && let Ok(value) = HeaderValue::from_str(&api_key)
    {
        request.headers_mut().insert(header_name, value);
    }

    // Strip `api_key` from the URI so the secret never reaches request
    // logging, the TraceLayer span, or anything else that echoes the URL.
    if let Some(stripped) = strip_query_param(request.uri(), "api_key") {
        *request.uri_mut() = stripped;
    }

    next.run(request).await
}

/// Returns a copy of `uri` with every `name=…` pair removed from the query
/// string. `None` when there's no query or the rebuilt URI can't be
/// constructed — the caller then leaves the original URI unchanged.
fn strip_query_param(uri: &Uri, name: &str) -> Option<Uri> {
    let query = uri.query()?;
    let filtered = query
        .split('&')
        .filter(|pair| pair.split('=').next() != Some(name))
        .collect::<Vec<_>>()
        .join("&");
    let path = uri.path();
    let path_and_query = if filtered.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{filtered}")
    };
    let pq: PathAndQuery = path_and_query.parse().ok()?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(pq);
    Uri::from_parts(parts).ok()
}

fn extract_api_key(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        if key != "api_key" {
            continue;
        }
        let raw = parts.next().unwrap_or("");
        return Some(percent_decode(raw));
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            match u8::from_str_radix(hex, 16) {
                Ok(decoded) => {
                    out.push(decoded);
                    i += 3;
                }
                Err(_) => {
                    out.push(b);
                    i += 1;
                }
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_basic_api_key() {
        assert_eq!(extract_api_key("api_key=abc"), Some("abc".to_string()));
    }

    #[test]
    fn extract_with_other_params() {
        assert_eq!(
            extract_api_key("branch=main&api_key=xyz&other=1"),
            Some("xyz".to_string()),
        );
    }

    #[test]
    fn extract_url_encoded() {
        assert_eq!(
            extract_api_key("api_key=hello%2Bworld%3D"),
            Some("hello+world=".to_string()),
        );
    }

    #[test]
    fn extract_missing_returns_none() {
        assert_eq!(extract_api_key("branch=main"), None);
    }

    #[test]
    fn extract_empty_value() {
        assert_eq!(extract_api_key("api_key="), Some("".to_string()));
    }

    #[test]
    fn strip_removes_api_key_and_keeps_others() {
        let uri: Uri = "/p/sql/query?branch=main&api_key=secret&x=1"
            .parse()
            .unwrap();
        let stripped = strip_query_param(&uri, "api_key").unwrap();
        assert_eq!(stripped.query(), Some("branch=main&x=1"));
        assert!(!stripped.to_string().contains("secret"));
    }

    #[test]
    fn strip_drops_query_when_only_api_key() {
        let uri: Uri = "/world-model/events?api_key=secret".parse().unwrap();
        let stripped = strip_query_param(&uri, "api_key").unwrap();
        assert_eq!(stripped.query(), None);
        assert_eq!(stripped.path(), "/world-model/events");
    }

    #[test]
    fn strip_noop_without_param() {
        let uri: Uri = "/p/sql?branch=main".parse().unwrap();
        let stripped = strip_query_param(&uri, "api_key").unwrap();
        assert_eq!(stripped.query(), Some("branch=main"));
    }
}
