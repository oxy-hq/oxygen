//! Internal reverse-proxy from a serve replica to the ide StatefulSet.
//!
//! Under the stateless-fleet split, `IdeOnly` routes (git ops, file CRUD,
//! onboarding clones — anything that needs the workspace working copy on local
//! disk) cannot be served by a serve replica. Instead of returning `421` and
//! relying on an EXTERNAL ingress route table (the chart `ideRoutes` → ALB
//! rules, which drifted from `role_manifest` and caused three outages), the
//! serve replica forwards the request to the ide pod itself.
//!
//! Consequence: [`role_manifest::classify`] is the routing AUTHORITY, in the
//! same binary that serves traffic. There is no external route list (chart
//! `ideRoutes` → ALB) to drift from the code, and the edge load balancer just
//! round-robins. (Residual, smaller, in-binary risk: `classify` still defaults
//! an *unlisted* route to FleetOk, so a forgotten new IdeOnly classification
//! would be served locally and fail rather than forward — see the
//! forward-on-doubt + router-introspecting-drift-test fast-follows in
//! `internal-docs/multi-instance-fleet.md`.)
//!
//! ## Trust posture
//!
//! The ide upstream is OUR OWN backend inside the cluster, reached over an
//! in-cluster Service. So — unlike a proxy to a third party, which would have
//! to strip `Cookie` / `Authorization` to avoid leaking the session — we
//! **preserve** auth headers: the forwarded request must stay the same
//! authenticated user. We strip only RFC 7230 hop-by-hop headers and guard
//! against forward loops.

use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::Client;

/// Marks a request as already forwarded by a serve replica. If a forwarded
/// request somehow lands back on a serve replica (e.g. the upstream Service
/// mistakenly selects serve pods too), we refuse to forward it again and fall
/// back to `421` rather than ping-pong forever.
const HEADER_FORWARDED_BY: &str = "x-oxy-forwarded-by";

/// Required-role hint mirrored from `role_middleware` so the FE can tell an
/// "ide backend down" 502 apart from a generic gateway error.
const HEADER_REQUIRED_ROLE: &str = "x-oxy-required-role";

/// Which workspace capability is unavailable, for the one case the status code
/// cannot express on its own. Exactly one value is emitted:
///   - `workspace-materializing`: the ide owns this workspace but its working
///     copy is not on disk yet (pod restart / rolling update, before the volume
///     is populated). It rides a `503` and means "not ready yet, retry" — NOT
///     the ide-down `502` below, where the ide is unreachable rather than
///     un-warmed. Produced by `workspaces::handlers` and `workspace_context`,
///     read by the FE's `isWorkspaceMaterializingError`.
///
/// The ide-down `502` does NOT carry this header. It used to split into
/// `workspace-{runtime,editing}` from a hand-written path table in
/// `role_manifest`, but nothing consumed the distinction — the FE banner keys
/// off [`HEADER_REQUIRED_ROLE`] alone (see `ideHealth.ts`) — so the table was
/// pure drift risk. `forward_to_ide_opt` already WARNs with the failing `url`,
/// which is the signal ops actually had.
pub(crate) const HEADER_UNAVAILABLE: &str = "x-oxy-unavailable";

/// The caller-visible host, carried across the proxy hop because `Host` cannot
/// be. See [`preserve_public_host`].
const HEADER_FORWARDED_HOST: &str = "x-forwarded-host";

/// The ide upstream base URL from `OXY_IDE_UPSTREAM`, e.g.
/// `http://oxy-dev-oxy-app-ide:80`. `None` (unset/empty) → forwarding is off
/// and the caller keeps the legacy `421` behaviour. This is the only knob:
/// local/single-instance and any serve fleet not yet wired to an ide upstream
/// behave exactly as before.
///
/// MUST point at a Service that selects ONLY the ide StatefulSet pods — never
/// the serve fleet — or requests would loop (the [`HEADER_FORWARDED_BY`] guard
/// is the safety net, not the design).
pub fn ide_upstream() -> Option<&'static str> {
    static UPSTREAM: OnceLock<Option<String>> = OnceLock::new();
    UPSTREAM
        .get_or_init(|| {
            std::env::var("OXY_IDE_UPSTREAM")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
        })
        .as_deref()
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // Timeouts (no BLANKET `.timeout(...)`, which would sever long-lived SSE
        // IdeOnly streams — onboarding/modeling — at a fixed budget):
        //   - `connect_timeout(3s)`: an in-cluster Service connect is sub-second;
        //     a short ceiling avoids piling connect attempts onto a
        //     down/recovering ide pod (connection-storm mitigation).
        //   - `read_timeout(120s)`: bounds the gap between successful reads, not
        //     total duration — so a connected-but-hung ide pod (deadlock,
        //     stalled disk IO mid-git-op) is eventually reclaimed instead of
        //     pinning a serve worker slot forever. On elapse reqwest errs → the
        //     caller's 502 path fires.
        //
        //     The 120s budget bounds time-to-first-response-byte too, so it is
        //     sized for the SLOW-BUT-HEALTHY case, not just hangs: the ide pod
        //     must emit its first response byte within 120s. ASSUMPTIONS to hold
        //     for that to be safe — (1) long IdeOnly ops (the ~500 MiB
        //     `onboarding/upload-warehouse-files`, a cold git clone) finish or
        //     start responding under 120s even on slow disk; (2) every IdeOnly
        //     SSE producer emits a frame/keepalive comfortably under 120s of
        //     quiet (each frame resets the clock). If an op legitimately needs
        //     longer-to-first-byte, it should stream progress rather than block.
        // `redirect::none()` — stream any 3xx straight to the browser.
        Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(120))
            .pool_idle_timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("ide_proxy reqwest client init")
    })
}

/// True if this request was already forwarded once (loop guard). The caller
/// must NOT forward such a request again.
pub fn already_forwarded(req: &Request) -> bool {
    req.headers().contains_key(HEADER_FORWARDED_BY)
}

/// Reverse-proxy `req` to the ide upstream. `Ok(resp)` = the ide answered (any
/// status). `Err(req)` = the ide could not be REACHED (connect / transport
/// error); the request is handed BACK, rebuilt from its own parts so the
/// extensions (path params, OriginalUri) survive — the caller can fall through
/// to a local handler. Today [`forward_to_ide`] is the only caller and maps
/// `Err` to the 502 — the hand-back exists for a caller that wants to serve a
/// local fallback, which no route currently does (a read that must survive the
/// ide is classified `FleetOk` and never reaches this proxy).
///
/// Preserves method, path, query, and auth headers; STREAMS both bodies
/// (SSE-safe; the upstream's own per-route body limit is the only ceiling).
pub async fn forward_to_ide_opt(upstream_base: &str, req: Request) -> Result<Response, Request> {
    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| parts.uri.path());
    let url = format!("{upstream_base}{path_and_query}");

    let Ok(method) = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) else {
        return Ok((StatusCode::METHOD_NOT_ALLOWED, "bad method").into_response());
    };

    // STREAM the request body through — do NOT buffer. Buffering with a fixed
    // cap silently broke the one IdeOnly route that takes a large upload
    // (`onboarding/upload-warehouse-files`, ~500 MiB), so the same upload
    // succeeded on the ide pod but 413'd on a serve replica — exactly the
    // round-trip drift this design exists to kill. Streaming makes the proxy
    // transparent: the ide upstream's own per-route `DefaultBodyLimit` is the
    // only ceiling, and we don't hold the upload in serve-replica RAM.
    let out_body = reqwest::Body::wrap_stream(body.into_data_stream());

    let mut out_headers = filter_request_headers(&parts.headers);
    // Loop guard marker — see HEADER_FORWARDED_BY.
    out_headers.insert(HEADER_FORWARDED_BY, HeaderValue::from_static("serve"));
    // Carry the PUBLIC host across the hop — see `preserve_public_host`.
    preserve_public_host(&parts.headers, &mut out_headers);
    // Carry the trace across the hop: the ide pod's request span becomes a
    // child of this replica's, so HyperDX shows one trace for the round trip
    // rather than two that share only `x-oxy-request-id`. Replaces whatever
    // `traceparent` the client sent — that one already parented THIS span.
    // No-op when OTLP export is off (no span context to write).
    oxy_telemetry::propagation::inject_current(&mut out_headers);

    let upstream = client()
        .request(method, &url)
        .headers(out_headers)
        .body(out_body)
        .send()
        .await;

    let upstream = match upstream {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(%url, ?err, "ide_proxy: ide upstream unreachable");
            // Hand the request back rebuilt from its parts — the extensions
            // (path params, OriginalUri) ride along so a caller that falls
            // through to a local handler keeps working. Body is empty (the
            // only callers that use this are read-only GETs).
            return Err(Request::from_parts(parts, Body::empty()));
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_headers = HeaderMap::new();
    for (name, value) in upstream.headers().iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            // `append` (not `insert`) preserves multi-valued headers like
            // Set-Cookie — the ide upstream is our own domain, so its cookies
            // (e.g. an auth refresh) pass through verbatim.
            resp_headers.append(n, v);
        }
    }

    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = resp_headers;
    Ok(response)
}

/// Reverse-proxy `req`, mapping an unreachable ide to the legible `502` with
/// `X-Oxy-Required-Role: ide`. This is what EVERY forwarded IdeOnly route gets:
/// `role_middleware` no longer has a degrade arm, because a read that must
/// survive the ide is fixed by classifying it `FleetOk`, not by serving a
/// half-answer from a replica.
pub async fn forward_to_ide(upstream_base: &str, req: Request) -> Response {
    match forward_to_ide_opt(upstream_base, req).await {
        Ok(resp) => resp,
        Err(_unreachable) => ide_unreachable_response(),
    }
}

/// The legible response for an unreachable ide on a non-degradable IdeOnly
/// route. The ide is RESTARTING, not gone, so this is a retryable degradation,
/// not a hard error:
///   - `X-Oxy-Required-Role: ide` — the FE's existing ide-down banner trigger,
///     and the whole detection contract (see [`HEADER_UNAVAILABLE`] for why
///     there is no per-capability header here).
///   - `Retry-After: 5` — pace client retries instead of hot-looping a
///     recovering pod (reinforces the proxy's `connect_timeout` storm guard).
/// Status stays `502` (not `503`) to preserve the FE detection contract
/// (`status === 502 && x-oxy-required-role === 'ide'`).
fn ide_unreachable_response() -> Response {
    let mut resp = (
        StatusCode::BAD_GATEWAY,
        "ide backend unreachable: workspace editing and runtime temporarily unavailable",
    )
        .into_response();
    let headers = resp.headers_mut();
    headers.insert(HEADER_REQUIRED_ROLE, HeaderValue::from_static("ide"));
    headers.insert(
        axum::http::header::RETRY_AFTER,
        HeaderValue::from_static("5"),
    );
    resp
}

/// Copy request headers, dropping:
///   - RFC 7230 hop-by-hop headers,
///   - `Host` (reqwest sets the upstream Host),
///   - `Content-Length` (the body is re-framed as a chunked stream, so a
///     stale length would conflict with reqwest's framing),
///   - any inbound `X-Oxy-Forwarded-By` (the loop-guard marker is OURS to set;
///     stripping a client-supplied one keeps it from reaching the ide pod).
///
/// UNLIKE the customer-apps proxy, `Cookie` and `Authorization` are KEPT — the
/// ide upstream is our own authenticated backend and the forwarded request must
/// remain the same user.
fn filter_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n)
            || n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case(HEADER_FORWARDED_BY)
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Carry the caller-visible host across the hop as `X-Forwarded-Host`.
///
/// [`filter_request_headers`] must drop `Host` (reqwest sets the upstream's),
/// which leaves the ide pod seeing the in-cluster Service name as its host —
/// while `Origin` / `Referer` ride through untouched, because they are
/// auth-relevant. Any upstream check that compares the two then compares a
/// public origin against an internal host and fails.
///
/// That is not hypothetical: it 403'd every custom-app data call. The
/// `check_custom_app_gates` origin allowlist (`is_self_origin`) reads exactly
/// `x-forwarded-host` → `host`, so on the ide pod both candidates were gone and
/// `POST /api/projects/{id}/query` + `/semantic-query` returned
/// "origin not allowed" for every browser caller regardless of role — while
/// their FleetOk siblings on the SAME gate chain (`/shell-context`,
/// `/threads`) served locally, kept their `Host`, and passed. AWS ALB does not
/// set `X-Forwarded-Host` (only `For` / `Proto` / `Port`), so nothing upstream
/// of us supplies it either.
///
/// Prefer an inbound `X-Forwarded-Host` over `Host`: an edge that already
/// rewrote the host is the authority on what the client asked for. Same
/// order as `is_self_origin`.
fn preserve_public_host(incoming: &HeaderMap, out: &mut HeaderMap) {
    let public_host = incoming
        .get(HEADER_FORWARDED_HOST)
        .or_else(|| incoming.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Empty should never happen (axum guarantees `Host` on HTTP/1.1 and
    // synthesises one from `:authority` on HTTP/2). Setting an empty value
    // would be worse than leaving it off — `is_self_origin` treats an empty
    // host as "no match" either way, and a present-but-empty header would mask
    // a real `Host` further up.
    if let Ok(v) = HeaderValue::from_str(public_host)
        && !public_host.is_empty()
    {
        out.insert(HEADER_FORWARDED_HOST, v);
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    #[test]
    fn hop_by_hop_classification() {
        assert!(is_hop_by_hop("connection"));
        assert!(is_hop_by_hop("Transfer-Encoding"));
        assert!(is_hop_by_hop("UPGRADE"));
        assert!(!is_hop_by_hop("cookie"));
        assert!(!is_hop_by_hop("authorization"));
        assert!(!is_hop_by_hop("content-type"));
    }

    #[test]
    fn request_headers_preserve_auth_drop_hop_and_host() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("oxy_session=abc"));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer tok"),
        );
        h.insert(
            header::HOST,
            HeaderValue::from_static("app-dev.example.com"),
        );
        h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("123"));
        h.insert(HEADER_FORWARDED_BY, HeaderValue::from_static("spoofed"));

        let out = filter_request_headers(&h);
        // Auth preserved (the whole point — it's our own backend).
        assert_eq!(out.get(header::COOKIE).unwrap(), "oxy_session=abc");
        assert_eq!(out.get(header::AUTHORIZATION).unwrap(), "Bearer tok");
        assert_eq!(out.get(header::CONTENT_TYPE).unwrap(), "application/json");
        // Hop-by-hop + Host dropped; Content-Length dropped (body is re-framed
        // as a chunked stream); a client-spoofed loop marker is stripped.
        assert!(out.get(header::CONNECTION).is_none());
        assert!(out.get(header::HOST).is_none());
        assert!(out.get(header::CONTENT_LENGTH).is_none());
        assert!(out.get(HEADER_FORWARDED_BY).is_none());
    }

    /// Build the header map as the ide pod actually receives it: filtered,
    /// loop-marked, public host carried. Keep in sync with `forward_to_ide_opt`.
    fn forwarded(inbound: &HeaderMap) -> HeaderMap {
        let mut out = filter_request_headers(inbound);
        out.insert(HEADER_FORWARDED_BY, HeaderValue::from_static("serve"));
        preserve_public_host(inbound, &mut out);
        out
    }

    /// A browser request as it reaches a serve replica behind the ALB.
    fn browser_request() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("app.oxygen-hq.com"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.oxygen-hq.com"),
        );
        h.insert(
            header::REFERER,
            HeaderValue::from_static("https://app.oxygen-hq.com/customer-apps/acme/scout/"),
        );
        h.insert(header::COOKIE, HeaderValue::from_static("oxy_session=abc"));
        h
    }

    /// THE regression: the origin allowlist must survive the hop.
    ///
    /// `Origin` rides through but `Host` cannot, so before `preserve_public_host`
    /// the ide pod compared a public origin against an in-cluster Service name
    /// and refused every browser-issued custom-app data call with 403 "origin not
    /// allowed" — `/query` and `/semantic-query`, for every user, regardless of
    /// role, while their FleetOk siblings on the same gate chain passed.
    ///
    /// Asserted through `is_allowed_origin` itself rather than by checking that
    /// the header is merely present: the gate is the property that broke, and a
    /// future rename of the header it reads should fail HERE.
    #[test]
    fn forwarded_request_still_passes_the_origin_allowlist() {
        let inbound = browser_request();
        assert!(
            crate::server::router::is_allowed_origin(&inbound),
            "precondition: the request is allowed at the serve replica"
        );

        let out = forwarded(&inbound);
        assert!(
            out.get(header::HOST).is_none(),
            "Host still must not cross the hop — reqwest sets the upstream's"
        );
        assert!(
            crate::server::router::is_allowed_origin(&out),
            "origin allowlist must reach the same verdict on the ide pod; \
             without X-Forwarded-Host this is the 403 that broke every \
             custom-app query"
        );
    }

    /// An edge that already rewrote the host is the authority on what the client
    /// asked for, so an inbound `X-Forwarded-Host` wins over `Host`.
    #[test]
    fn inbound_forwarded_host_wins_over_host() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("internal-lb:8080"));
        h.insert(
            HEADER_FORWARDED_HOST,
            HeaderValue::from_static("app.oxygen-hq.com"),
        );
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.oxygen-hq.com"),
        );

        let out = forwarded(&h);
        assert_eq!(out.get(HEADER_FORWARDED_HOST).unwrap(), "app.oxygen-hq.com");
        assert!(crate::server::router::is_allowed_origin(&out));
    }

    /// Org subdomains and custom-app subdomains are separate hosts; the
    /// forwarded header must reflect the one the caller actually used, not a
    /// canonical one, or the allowlist passes for the wrong site.
    #[test]
    fn public_host_is_the_callers_host_not_a_canonical_one() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("acme.oxygen-hq.com"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://acme.oxygen-hq.com"),
        );

        let out = forwarded(&h);
        assert_eq!(
            out.get(HEADER_FORWARDED_HOST).unwrap(),
            "acme.oxygen-hq.com"
        );
        assert!(crate::server::router::is_allowed_origin(&out));
    }

    /// A cross-site origin must still be refused after the hop. The fix carries
    /// the host across; it must not become a blanket allow.
    #[test]
    fn foreign_origin_is_still_rejected_after_the_hop() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("app.oxygen-hq.com"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example.com"),
        );

        let out = forwarded(&h);
        assert!(
            !crate::server::router::is_allowed_origin(&out),
            "carrying the public host must not turn the allowlist into a pass-through"
        );
    }

    /// No `Host` anywhere → no header rather than an empty one. An empty
    /// `X-Forwarded-Host` would shadow a real `Host` at the upstream.
    #[test]
    fn absent_host_sets_no_forwarded_host() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("oxy_session=abc"));

        let out = forwarded(&h);
        assert!(out.get(HEADER_FORWARDED_HOST).is_none());
    }

    #[test]
    fn ide_unreachable_response_is_retryable_and_says_which_role() {
        let resp = ide_unreachable_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let h = resp.headers();
        // FE detection contract: `502` + this header, nothing else.
        assert_eq!(h.get(HEADER_REQUIRED_ROLE).unwrap(), "ide");
        assert_eq!(h.get(header::RETRY_AFTER).unwrap(), "5");
        // `x-oxy-unavailable` belongs to the materializing `503` alone. Stamping
        // it here too would make `isWorkspaceMaterializingError`'s value check
        // the only thing separating two different states.
        assert!(
            h.get(HEADER_UNAVAILABLE).is_none(),
            "the ide-down 502 must not claim a capability class"
        );
    }

    #[test]
    fn already_forwarded_detects_the_loop_marker() {
        let bare = Request::builder().body(Body::empty()).unwrap();
        assert!(!already_forwarded(&bare));
        let marked = Request::builder()
            .header(HEADER_FORWARDED_BY, "serve")
            .body(Body::empty())
            .unwrap();
        assert!(already_forwarded(&marked));
    }
}
