//! Sentry, barrier 2: tag every event captured while serving a custom app, so
//! `oxy::sentry_config`'s `before_send` drops it ([`crate::sentry_filter::drop_event`]).
//! Barrier 1 only sees `tracing` targets; this also covers callees outside
//! `custom_apps_*`, `OxyError::capture_to_sentry`, and panics.
//!
//! A scope tag lives on a hub, and Sentry hubs are per thread, so the tag holds
//! only where the hub travels:
//! - the per-request hub `NewSentryLayer` binds on the outer router (serve.rs);
//! - [`custom_app_hub`], bound onto a background function job (`app_function_executor`);
//! - the isolate thread, host-call tasks and `spawn_blocking` closures, which
//!   carry `Hub::current()` across explicitly (`custom_apps_functions::{runtime, host}`);
//! - every request-scoped spawn inside the agentic crates, which goes through
//!   `agentic_core::hub_task` (`spawn_with_hub`, `spawn_blocking_with_hub`,
//!   `bind_current_hub`) and so carries whatever hub the handler bound without
//!   knowing the tag exists — pinned by the second scan test in
//!   `oxy-guards`' `tests/integration/sentry_hub_scan.rs`.
//!
//! [`tag_custom_app_surface`] decides from the URL — host label or
//! `/customer-apps/**` path. That is not enough for the custom-app **data
//! plane**: a bundle reads its data same-origin over `/api/projects/{id}/query`
//! and `/semantic-query`, and on the canonical `<base>/customer-apps/<org>/<app>/`
//! URL — or on an org subdomain, where `org_host_dispatch` passes `/api/**`
//! through untouched — that origin is an ordinary platform host with an
//! ordinary `/api` path. [`mark_custom_app_surface`] closes that by **identity**
//! instead: `check_custom_app_gates` calls it, and every request that enters
//! that gate is a custom-app bundle's data call by construction.
//!
//! Moved down from `oxy-app`'s `server::api::middlewares::sentry_surface` (which
//! still re-exports it for `oxy_app::{host, surface}`) on 2026-11-21; its tests,
//! the source walks over `oxy-app`, the surfaces and `crates/agentic` included,
//! live in `oxy-guards`' `tests/integration/sentry_hub_scan.rs` (oxy-app's
//! `tests/routing/` until decompose GRD; the gate-driven tag tests are
//! `oxy-workspace-runtime`'s `tests/integration/sentry_surface_gate.rs`).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::header::HOST;
use axum::middleware::Next;
use axum::response::Response;
use futures_core::Stream;

use crate::sentry_filter::is_custom_app_request;

/// The caller-visible host, `X-Forwarded-Host` first.
///
/// **`Host` alone is not enough on the ide pod.** `ide_proxy`'s
/// `filter_request_headers` strips `Host` from every forwarded request (reqwest
/// sets the upstream's), so on the ide side the only surviving record of what
/// the browser asked for is `X-Forwarded-Host`, which that hop adds. Reading
/// `Host` alone therefore left the custom-app *data plane* untagged: a proxied
/// `POST /api/projects/{id}/query` — tenant SQL and its results — carries
/// neither a custom-app host nor a `/customer-apps/**` path once it reaches the
/// ide pod, so every error captured under it would have gone to Sentry.
///
/// The same header loss already produced the "origin not allowed" 403s that
/// `ide_proxy` documents at length, and the existing readers of a public host
/// resolve it exactly this way: `ide_proxy::preserve_public_host` and the
/// `is_self_origin` gate. Mirror them rather than inventing another order.
///
/// **This trusts a caller-settable header, and that is affordable here.**
/// `HeaderMap::get` returns the FIRST value, so a header a client appends after
/// the edge's own cannot displace it; what the ordering relies on is the edge
/// setting `X-Forwarded-Host` itself on every inbound request. Where something
/// lets a client's value through, all that caller can do is label its OWN
/// request a custom app and have its OWN errors dropped from Sentry — one
/// request's telemetry, in the fail-closed direction this barrier already errs
/// in. Nothing authorizes off this value: it decides a Sentry tag and nothing
/// else. Keep it that way — the moment a permission reads `caller_host`, the
/// header needs a trust story this one does not have.
fn caller_host(req: &Request) -> Option<&str> {
    req.headers()
        .get("x-forwarded-host")
        .or_else(|| req.headers().get(HOST))
        .and_then(|value| value.to_str().ok())
        .or_else(|| req.uri().host())
}

/// Tag the current per-request hub when the request is for a custom app.
///
/// Must run inside `NewSentryLayer`, or the tag lands on a shared hub and
/// leaks onto other requests' events. Must also run after the host-dispatch
/// rewrites, so `/a/<slug>/` on an org subdomain already reads as
/// `/customer-apps/**`; the original host header is still present then.
pub async fn tag_custom_app_surface(req: Request, next: Next) -> Response {
    if is_custom_app_request(caller_host(&req), req.uri().path()) {
        mark_custom_app_surface();
    }
    next.run(req).await
}

/// Tag the current hub as a custom-app surface, for a caller that already
/// *knows* it is serving a custom app rather than inferring it from the URL.
///
/// The one caller is `check_custom_app_gates` — the gate every custom-app
/// data-plane endpoint runs first (`/api/projects/{id}/query`,
/// `/semantic-query`, the metric-tree ops, agent asks, automation runs, the
/// app's threads and activity). Reaching that gate *is* the identity: the web
/// app never calls these routes, and the gate's own chain (app session cookie
/// or project-scoped bearer token, Origin allowlist, resolved workspace,
/// org-membership) is what a custom-app caller passes through.
///
/// **Why not a third URL rule.** The bundle fetches with an empty
/// `apiBaseUrl`, so its data calls are same-origin — and its origin is usually
/// *not* a custom-app host: the canonical app URL is
/// `<base>/customer-apps/<org>/<app>/`, so the fetch goes to
/// `app.oxygen-hq.com/api/projects/{id}/query`, and on an org subdomain
/// `org_host_dispatch` deliberately leaves `/api/**` alone. Neither the host
/// label nor the `/customer-apps/**` path rule can see those, so the warehouse
/// error text `query.rs` logs — the `Code: 27` class the design names — was
/// reaching Sentry untagged, and so would a panic under the same request.
/// Adding a fourth URL shape would only move the guess; the gate is the fact.
///
/// Called at the **top** of the gate, before authentication, so the tag is on
/// the hub for everything the gate itself and the handler behind it may
/// capture — including a panic, which has no `tracing` target for barrier 1 to
/// read. That is the fail-closed direction the barrier is built on: a false
/// drop costs one platform event, a false keep ships customer data.
pub use crate::sentry_filter::mark_custom_app_surface;

/// A fresh hub, already tagged, for custom-app work with no request around it:
/// a scheduled, manual or webhook-triggered function job. Defined beside the
/// tag in [`crate::sentry_filter`], because the usage queue below
/// `oxy-app` needs it too.
pub use crate::sentry_filter::custom_app_hub;

/// Bind `hub` across every poll of `stream` — the stream counterpart of
/// [`sentry::SentryFutureExt::bind_hub`], which only extends `Future`.
///
/// A custom-app SSE body needs this and `bind_hub` cannot give it. Axum polls
/// the body *after* the handler future has resolved, so the tower layer's
/// `Hub::run` has already unwound and `Hub::current()` on the polling task is
/// the untagged default. Everything the body then captures — a callee's
/// `error!` on a target barrier 1 does not drop, or a panic under the same poll —
/// would reach Sentry untagged, carrying a tenant agent run's id and error text.
///
/// Boxing is what makes the adapter `Unpin`, so `poll_next` needs no
/// pin-projection dependency. It costs one allocation per SSE connection,
/// alongside a body that already does a database read every 250 ms.
pub fn bind_hub_stream<S: Stream>(stream: S, hub: Arc<sentry::Hub>) -> HubBoundStream<S> {
    HubBoundStream {
        inner: Box::pin(stream),
        hub,
    }
}

/// The stream returned by [`bind_hub_stream`].
pub struct HubBoundStream<S> {
    inner: Pin<Box<S>>,
    hub: Arc<sentry::Hub>,
}

impl<S: Stream> Stream for HubBoundStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Both fields are `Unpin` (a `Pin<Box<_>>` and an `Arc`), so the pin is
        // structural in neither and `get_mut` is the whole projection.
        let this = self.get_mut();
        let hub = Arc::clone(&this.hub);
        let inner = this.inner.as_mut();
        sentry::Hub::run(hub, || inner.poll_next(cx))
    }
}
