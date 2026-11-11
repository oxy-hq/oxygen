//! `x-oxy-request-id` — one server-minted id per HTTP request.
//!
//! ## Why the server mints it rather than trusting the caller
//!
//! This id is the join key that makes *"a user says the app broke at 3:04pm"* a
//! query instead of an archaeology project: the same value lands on the serve
//! row, the function invocation, every `ctx.log()` line that invocation writes,
//! and (once the client-error table exists) the browser beacon. A caller-supplied
//! id would let one request claim another's identity, so an inbound
//! `x-oxy-request-id` is **ignored and overwritten** — except on an internal
//! proxy hop, which is re-entering this middleware on the ide pod with an id
//! the client already has. That exception is gated on the receiving process's
//! own role, never on an inbound header, because a header is settable by
//! anyone who can reach the edge. That matches how the
//! platforms this borrows from behave — `cf-ray` and `x-vercel-id` are minted by
//! the edge, never accepted from the client. The browser learns the id from the
//! *response* header.
//!
//! ## Three places it shows up
//!
//! 1. **Request extensions** as [`RequestId`], for handlers that can take an
//!    extractor.
//! 2. **Request headers**, so the many handlers that already take a bare
//!    `HeaderMap` can read it without a signature change. Note this is *not* how
//!    it reaches an Oxy Function: `custom_apps_functions::sanitize_request_headers`
//!    strips every `x-oxy-*` header before the isolate sees the request, which is
//!    a rule worth keeping — the id is passed to the runtime explicitly instead.
//! 3. **Response headers**, which is the only way a browser (or a support ticket)
//!    can name the request that went wrong.
//!
//! Mounted on the OUTER router in `oxy-server`'s `commands/serve.rs` so it covers `/api`,
//! the `/customer-apps/{*path}` serve tree and the static SPA fallback alike.

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::convert::Infallible;
use uuid::Uuid;

/// Wire name. Re-exported from `oxy-shared` rather than declared here: the
/// custom-apps surface reads this header and is barred from importing
/// `server::api::middlewares` (see `custom_apps_boundary.rs`), so the name and
/// the parse live one crate down and the minting policy stays here.
pub use oxy_shared::utils::request_id::HEADER as REQUEST_ID_HEADER;

/// The id minted for the request currently being served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub Uuid);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Mint an id, expose it to the handler two ways, and echo it to the caller.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    // One carve-out from "never trust the caller": a request the serve fleet
    // has already forwarded to the ide pod carries the id the client was
    // given, and re-minting here would make the client's id and the ide pod's
    // spans name different requests — breaking the single join this header
    // exists to provide, on exactly the `IdeOnly` routes where two pods are
    // involved.
    //
    // **`x-oxy-forwarded-by` alone cannot carry that trust.** Nothing strips it
    // inbound — `already_forwarded` is a bare `contains_key`, and
    // `filter_request_headers` only sanitises it on the way *out* to the ide
    // pod — so `curl -H 'x-oxy-forwarded-by: serve' -H 'x-oxy-request-id: …'`
    // aimed at the edge would be honoured. Using the header as a loop guard is
    // fail-safe (a spoof gets a 421); using it for identity is fail-open, and
    // this key now anchors a client-error table.
    //
    // So the gate is the RECEIVING PROCESS: only an `ide` role is downstream of
    // the proxy. A `serve` or `all` process IS the internet edge and mints
    // unconditionally, whatever headers arrive.
    let internal_hop = accept_inbound(
        crate::server::role_manifest::current_process_role(),
        crate::server::ide_proxy::already_forwarded(&req),
    );
    let inbound = internal_hop
        .then(|| {
            req.headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| Uuid::parse_str(s).ok())
        })
        .flatten();
    let id = RequestId(inbound.unwrap_or_else(Uuid::new_v4));
    let header_name = HeaderName::from_static(REQUEST_ID_HEADER);

    req.extensions_mut().insert(id);
    // A UUID's Display is always a valid header value, so the fallible
    // conversion cannot realistically fail — but a panic here would take down
    // every request, so it degrades to "no header" instead.
    if let Ok(value) = HeaderValue::from_str(&id.0.to_string()) {
        req.headers_mut().insert(header_name.clone(), value.clone());
        let mut response = next.run(req).await;
        response.headers_mut().insert(header_name, value);
        return response;
    }
    next.run(req).await
}

/// May an inbound `x-oxy-request-id` be trusted on this hop?
///
/// Pure so the rule is testable without a process-wide role. Only an `ide`
/// process sits downstream of `ide_proxy`; a `serve` or `all` process is the
/// internet edge, where `x-oxy-forwarded-by` is just a header a caller typed.
fn accept_inbound(role: crate::server::role_manifest::Role, forwarded: bool) -> bool {
    matches!(role, crate::server::role_manifest::Role::Ide) && forwarded
}

/// Extractor. Returns `None` when the middleware is not mounted (unit tests that
/// build a bare router), rather than inventing an id that would correlate
/// nothing — an absent id is honest, a fabricated one is a false join.
pub struct MaybeRequestId(pub Option<RequestId>);

impl<S> FromRequestParts<S> for MaybeRequestId
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeRequestId(parts.extensions.get::<RequestId>().copied()))
    }
}

/// Read the id out of a `HeaderMap` a handler already holds.
pub use oxy_shared::utils::request_id::from_headers as request_id_from_headers;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    /// The handler sees an id, the caller gets the same one back, and the two
    /// agree — which is the whole contract.
    #[tokio::test]
    async fn mints_and_echoes_the_same_id() {
        let app = Router::new()
            .route(
                "/",
                get(|headers: axum::http::HeaderMap| async move {
                    request_id_from_headers(&headers)
                        .map(|id| id.to_string())
                        .unwrap_or_default()
                }),
            )
            .layer(axum::middleware::from_fn(request_id_middleware));

        let response = app
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let echoed = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("response carries the id")
            .to_str()
            .unwrap()
            .to_string();
        let seen_by_handler = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 128)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        assert!(Uuid::parse_str(&echoed).is_ok(), "not a uuid: {echoed}");
        assert_eq!(seen_by_handler, echoed);
    }

    /// The carve-out is gated on the RECEIVING PROCESS, not on a header.
    /// `x-oxy-forwarded-by` is not stripped inbound, so at the edge it is just
    /// a string a caller typed — trusting it for identity would let anyone pin
    /// every request to an id of their choosing, on a key that now anchors a
    /// client-error table.
    #[test]
    fn only_an_ide_process_trusts_a_forwarded_id() {
        use crate::server::role_manifest::Role;
        assert!(accept_inbound(Role::Ide, true), "the real internal hop");
        assert!(
            !accept_inbound(Role::Serve, true),
            "serve IS the edge — a forwarded-by header there is a spoof"
        );
        assert!(
            !accept_inbound(Role::All, true),
            "an all-in-one deployment is also the edge"
        );
        assert!(
            !accept_inbound(Role::Worker, true),
            "a worker is not downstream of ide_proxy either"
        );
        assert!(!accept_inbound(Role::Ide, false), "no hop, no exception");
    }

    /// End-to-end on the default (non-ide) role every test process has: a
    /// forged pair must not survive.
    #[tokio::test]
    async fn a_forged_forwarded_hop_at_the_edge_still_mints() {
        let forged = "11111111-2222-3333-4444-555555555555";
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(request_id_middleware));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .header(REQUEST_ID_HEADER, forged)
                    .header("x-oxy-forwarded-by", "serve")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.headers().get(REQUEST_ID_HEADER).unwrap(),
            forged,
            "a header alone must not buy the carve-out"
        );
    }

    /// A caller-supplied id must not survive at the EDGE: it is a join key, not
    /// an input. Only an internal hop (above) is trusted.
    #[tokio::test]
    async fn overwrites_a_caller_supplied_id() {
        let forged = "00000000-0000-0000-0000-00000000dead";
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(request_id_middleware));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .header(REQUEST_ID_HEADER, forged)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let echoed = response.headers().get(REQUEST_ID_HEADER).unwrap();
        assert_ne!(echoed, forged, "caller-supplied id was trusted");
    }
}
