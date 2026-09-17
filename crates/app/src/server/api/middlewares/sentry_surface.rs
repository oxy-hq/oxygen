//! Sentry, barrier 2: tag every event captured while serving a custom app, so
//! `oxy::sentry_config`'s `before_send` drops it (`oxy_telemetry::sentry_filter::drop_event`).
//! Barrier 1 only sees `tracing` targets; this also covers callees outside
//! `custom_apps_*`, `OxyError::capture_to_sentry`, and panics.
//!
//! A scope tag lives on a hub, and Sentry hubs are per thread, so the tag holds
//! only where the hub travels:
//! - the per-request hub `NewSentryLayer` binds on the outer router (serve.rs);
//! - [`custom_app_hub`], bound onto a background function job (`app_function_executor`);
//! - the isolate thread, host-call tasks and `spawn_blocking` closures, which
//!   carry `Hub::current()` across explicitly (`custom_apps_functions::{runtime, host}`).
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

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::header::HOST;
use axum::middleware::Next;
use axum::response::Response;
use futures::Stream;
use oxy_telemetry::sentry_filter::{
    CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG, is_custom_app_request,
};

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
/// `ide_proxy` documents at length, and both existing readers of a public host
/// resolve it exactly this way: `custom_apps_proxy`'s `public_host` and the
/// `is_self_origin` gate. Mirror them rather than inventing a third order.
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
pub fn mark_custom_app_surface() {
    sentry::configure_scope(|scope| scope.set_tag(CUSTOM_APP_SURFACE_TAG, CUSTOM_APP_SURFACE));
}

/// A fresh hub, already tagged, for custom-app work with no request around it:
/// a scheduled, manual or webhook-triggered function job.
pub fn custom_app_hub() -> Arc<sentry::Hub> {
    let hub = Arc::new(sentry::Hub::new_from_top(sentry::Hub::current()));
    hub.configure_scope(|scope| scope.set_tag(CUSTOM_APP_SURFACE_TAG, CUSTOM_APP_SURFACE));
    hub
}

/// Bind `hub` across every poll of `stream` — the stream counterpart of
/// [`sentry::SentryFutureExt::bind_hub`], which only extends `Future`.
///
/// A custom-app SSE body needs this and `bind_hub` cannot give it. Axum polls
/// the body *after* the handler future has resolved, so the tower layer's
/// `Hub::run` has already unwound and `Hub::current()` on the polling task is
/// the untagged default. Everything the body then captures — `agent_run_stream`'s
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::StatusCode;
    use axum::routing::any;
    use oxy_telemetry::sentry_filter::{CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG};
    use sentry::integrations::tower::NewSentryLayer;
    use tower::{ServiceBuilder, ServiceExt};

    /// As [`surface_tag_for`], but with the caller's host arriving the way the
    /// ide hop leaves it: `X-Forwarded-Host` set, `Host` rewritten to the
    /// in-cluster Service name (or absent).
    fn surface_tag_forwarded(
        forwarded_host: &str,
        host: Option<&str>,
        path: &str,
    ) -> Option<String> {
        let mut headers = vec![("x-forwarded-host", forwarded_host.to_string())];
        if let Some(host) = host {
            headers.push(("host", host.to_string()));
        }
        surface_tag_with_headers(&headers, path)
    }

    /// Serve one request behind the same two layers serve.rs installs, from a
    /// handler that captures one error, and return that event's surface tag.
    fn surface_tag_for(host: &str, path: &str) -> Option<String> {
        surface_tag_with_headers(&[("host", host.to_string())], path)
    }

    fn surface_tag_with_headers(headers: &[(&str, String)], path: &str) -> Option<String> {
        let events = sentry::test::with_captured_events(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let app = Router::new().route(
                    "/{*path}",
                    any(|| async {
                        sentry::capture_message("boom", sentry::Level::Error);
                        StatusCode::OK
                    }),
                );
                let service = ServiceBuilder::new()
                    .layer(NewSentryLayer::<Request>::new_from_top())
                    .layer(axum::middleware::from_fn(tag_custom_app_surface))
                    .service(app);
                let mut builder = Request::builder().uri(path);
                for (name, value) in headers {
                    builder = builder.header(*name, value);
                }
                let request = builder.body(Body::empty()).expect("request");
                service.oneshot(request).await.expect("response");
            });
        });
        assert_eq!(events.len(), 1, "exactly one captured event");
        events[0].tags.get(CUSTOM_APP_SURFACE_TAG).cloned()
    }

    #[test]
    fn a_request_on_a_custom_app_subdomain_is_tagged() {
        assert_eq!(
            surface_tag_for(
                "acme--store.customer-apps.oxygen-hq.com",
                "/api/projects/1/query"
            ),
            Some(CUSTOM_APP_SURFACE.to_string())
        );
    }

    #[test]
    fn a_customer_apps_path_on_the_admin_host_is_tagged() {
        assert_eq!(
            surface_tag_for("app.oxygen-hq.com", "/customer-apps/acme/store/fn/orders"),
            Some(CUSTOM_APP_SURFACE.to_string())
        );
    }

    /// The regression this fixes. On the ide pod `ide_proxy` has stripped
    /// `Host`, so a proxied custom-app data call (tenant SQL and results)
    /// carries its real host only in `X-Forwarded-Host` — and its path is
    /// `/api/**`, not `/customer-apps/**`, so the path rule cannot save it.
    #[test]
    fn a_proxied_custom_app_data_call_is_tagged_by_x_forwarded_host() {
        assert_eq!(
            surface_tag_forwarded(
                "acme--store.customer-apps.oxygen-hq.com",
                Some("oxy-ide.oxy-prod.svc.cluster.local"),
                "/api/projects/1/query"
            ),
            Some(CUSTOM_APP_SURFACE.to_string()),
            "a stripped Host must not un-tag a custom-app data call"
        );
    }

    /// `filter_request_headers` drops `Host` outright, so the header can be
    /// missing altogether rather than merely rewritten.
    #[test]
    fn x_forwarded_host_alone_is_enough() {
        assert_eq!(
            surface_tag_forwarded(
                "acme--store.customer-apps.oxygen-hq.com",
                None,
                "/api/projects/1/semantic-query"
            ),
            Some(CUSTOM_APP_SURFACE.to_string())
        );
    }

    /// An edge that already rewrote the host is the authority on what the
    /// client asked for, so `X-Forwarded-Host` outranks `Host` — the same
    /// precedence `custom_apps_proxy` and `is_self_origin` use.
    #[test]
    fn x_forwarded_host_outranks_a_platform_host_header() {
        assert_eq!(
            surface_tag_forwarded(
                "acme--store.customer-apps.oxygen-hq.com",
                Some("app.oxygen-hq.com"),
                "/api/threads"
            ),
            Some(CUSTOM_APP_SURFACE.to_string())
        );
    }

    /// The converse: a platform request forwarded by the same hop stays
    /// untagged, so this cannot start swallowing Oxy's own errors.
    #[test]
    fn a_forwarded_platform_request_is_not_tagged() {
        assert_eq!(
            surface_tag_forwarded(
                "app.oxygen-hq.com",
                Some("oxy-ide.oxy-prod.svc.cluster.local"),
                "/api/threads"
            ),
            None
        );
    }

    #[test]
    fn a_platform_request_is_not_tagged() {
        assert_eq!(
            surface_tag_for("app.oxygen-hq.com", "/api/customer-apps/1f0c/builds"),
            None
        );
    }

    /// Serve one request through the **real** custom-app data-plane route —
    /// `POST /api/projects/{id}/query`, mounted on the shipped `run_query`
    /// handler — behind the two layers serve.rs installs, and return the tag on
    /// an event captured later in the same request.
    ///
    /// The `Origin` is deliberately foreign, so `check_custom_app_gates` stops
    /// at its allowlist step and the test needs no database. The gate tags
    /// before it authenticates, so that is exactly the case the tag must
    /// already cover.
    fn data_plane_surface_tag(host: &str) -> Option<String> {
        route_surface_tag(
            Router::new().route(
                "/api/projects/{project_id}/query",
                axum::routing::post(crate::server::api::projects::query::run_query),
            ),
            host,
            "/api/projects/0195f0d2-0000-7000-8000-000000000000/query",
        )
    }

    /// As [`data_plane_surface_tag`], for any router. The capture happens in a
    /// layer *inside* `NewSentryLayer`, so it lands on the request's own hub —
    /// standing in for the handler's `error!`, an `OxyError::capture_to_sentry`
    /// or a panic later in the same request.
    fn route_surface_tag(app: Router, host: &str, path: &str) -> Option<String> {
        let events = sentry::test::with_captured_events(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let service = ServiceBuilder::new()
                    .layer(NewSentryLayer::<Request>::new_from_top())
                    // The shipped URL rule is in the stack on purpose: these
                    // URLs are the ones it cannot recognise, so whatever tag
                    // appears came from the gate.
                    .layer(axum::middleware::from_fn(tag_custom_app_surface))
                    .layer(axum::middleware::from_fn(
                        |req: Request, next: Next| async move {
                            let response = next.run(req).await;
                            sentry::capture_message("boom", sentry::Level::Error);
                            response
                        },
                    ))
                    .service(app);
                let request = Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", host)
                    // Not this host, and not a loopback pair: `is_allowed_origin`
                    // refuses it, so the gate returns 403 before any DB call.
                    .header("origin", "https://not-this-host.example")
                    .body(Body::empty())
                    .expect("request");
                service.oneshot(request).await.expect("response");
            });
        });
        assert_eq!(events.len(), 1, "exactly one captured event");
        events[0].tags.get(CUSTOM_APP_SURFACE_TAG).cloned()
    }

    /// C1. The canonical app URL is `<base>/customer-apps/<org>/<app>/`, and the
    /// bundle fetches its data same-origin with an empty `apiBaseUrl` — so the
    /// data call lands on the platform host with an `/api` path. No URL rule can
    /// see that; the gate can.
    #[test]
    fn a_custom_app_data_call_on_the_platform_host_is_tagged_by_the_gate() {
        assert_eq!(
            data_plane_surface_tag("app.oxygen-hq.com"),
            Some(CUSTOM_APP_SURFACE.to_string()),
            "a custom app's warehouse query must not reach Sentry untagged"
        );
    }

    /// The same call from an app served on an org subdomain. `org_host_dispatch`
    /// passes `/api/**` through untouched by design, so this request is
    /// indistinguishable from a platform one by host or path too.
    #[test]
    fn a_custom_app_data_call_on_an_org_subdomain_is_tagged_by_the_gate() {
        assert_eq!(
            data_plane_surface_tag("acme.oxygen-hq.com"),
            Some(CUSTOM_APP_SURFACE.to_string())
        );
    }

    /// The converse, and the reason the tag is keyed on the gate rather than on
    /// `/api/**`: an ordinary web-app call on the same host stays untagged, so
    /// Oxy's own errors still reach Sentry.
    #[test]
    fn a_plain_platform_api_call_stays_untagged() {
        assert_eq!(
            route_surface_tag(
                Router::new().route(
                    "/api/threads",
                    axum::routing::post(|| async { StatusCode::OK })
                ),
                "app.oxygen-hq.com",
                "/api/threads",
            ),
            None
        );
    }

    /// Why the gate is needed at all, stated as a fact about the URL rules:
    /// neither data-plane URL above matches either of them.
    #[test]
    fn the_url_rules_alone_cannot_see_the_data_plane() {
        use oxy_telemetry::sentry_filter::is_custom_app_request;

        for host in ["app.oxygen-hq.com", "acme.oxygen-hq.com"] {
            assert!(
                !is_custom_app_request(Some(host), "/api/projects/1/query"),
                "{host}"
            );
            assert!(
                !is_custom_app_request(Some(host), "/api/projects/1/semantic-query"),
                "{host}"
            );
        }
    }

    /// Run `body` on a current-thread runtime under a capture client, and
    /// return the tags of every event it captured, in order.
    ///
    /// Current-thread on purpose: a hub is per thread, so a multi-thread
    /// runtime would move a spawned task off the capture client and the
    /// *untagged* halves below would record nothing — passing for the wrong
    /// reason. Here every task is polled on this thread, so an unbound spawn
    /// really does capture, untagged.
    fn tags_captured_in<F>(body: F) -> Vec<Option<String>>
    where
        F: FnOnce(&tokio::runtime::Runtime),
    {
        let events = sentry::test::with_captured_events(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            body(&runtime);
        });
        events
            .into_iter()
            .map(|event| event.tags.get(CUSTOM_APP_SURFACE_TAG).cloned())
            .collect()
    }

    /// The mechanism behind the two custom-app data-plane spawns
    /// (`projects::agent_ask`, `projects::automation_run`): both return 202
    /// while the run continues, so by the time the pipeline captures anything
    /// the request's hub has unwound.
    ///
    /// The second half is the control: the *same* spawn without the binding
    /// captures untagged, so this cannot pass by inheriting a tag from
    /// somewhere else.
    #[test]
    fn a_spawn_carries_the_tag_only_when_it_is_bound() {
        use sentry::SentryFutureExt;

        let tags = tags_captured_in(|runtime| {
            runtime.block_on(async {
                // Read the hub on the spawning task, the way a handler does.
                let bound = sentry::Hub::run(custom_app_hub(), || {
                    tokio::spawn(
                        async {
                            sentry::capture_message("bound", sentry::Level::Error);
                        }
                        .bind_hub(sentry::Hub::current()),
                    )
                });
                bound.await.expect("bound task");

                let unbound = tokio::spawn(async {
                    sentry::capture_message("unbound", sentry::Level::Error);
                });
                unbound.await.expect("unbound task");
            });
        });

        assert_eq!(
            tags,
            vec![Some(CUSTOM_APP_SURFACE.to_string()), None],
            "the binding is what carries the tag past the request"
        );
    }

    /// The mechanism behind `projects::agent_run_stream`. An SSE body is a
    /// `Stream`, so `bind_hub` — a `Future` extension — cannot reach it, and
    /// axum polls it after the handler future has resolved and the tower
    /// layer's `Hub::run` has unwound.
    ///
    /// Both halves are polled outside the tagged hub, which is exactly where
    /// axum polls a body; only the bound one is tagged.
    #[test]
    fn a_stream_carries_the_tag_only_when_it_is_bound() {
        use futures::StreamExt;

        fn capturing_stream(message: &'static str) -> impl Stream<Item = u8> {
            async_stream::stream! {
                yield 1u8;
                sentry::capture_message(message, sentry::Level::Error);
            }
        }

        let tags = tags_captured_in(|runtime| {
            runtime.block_on(async {
                // Built inside the tagged hub, as the handler builds it...
                let bound = sentry::Hub::run(custom_app_hub(), || {
                    bind_hub_stream(capturing_stream("bound"), sentry::Hub::current())
                });
                // ...and drained outside it, as axum drains it.
                assert_eq!(bound.collect::<Vec<_>>().await, vec![1]);

                let unbound = sentry::Hub::run(custom_app_hub(), || capturing_stream("unbound"));
                assert_eq!(unbound.collect::<Vec<_>>().await, vec![1]);
            });
        });

        assert_eq!(
            tags,
            vec![Some(CUSTOM_APP_SURFACE.to_string()), None],
            "an SSE body outlives the hub that built it"
        );
    }

    /// Boundary: every spawn and stream on the custom-app **data plane** must
    /// carry a hub.
    ///
    /// The mechanism tests above prove `bind_hub` and [`bind_hub_stream`] work.
    /// They cannot prove any particular call site uses one — delete the
    /// `.bind_hub(..)` from `agent_ask` and both still pass, which is how three
    /// of these sites survived the first pass. Driving each site end to end is
    /// not the answer either: every one of them needs a live pipeline and a
    /// database to reach the point where it would capture.
    ///
    /// So the objection is mechanical and the source is the evidence, as in
    /// `tests/authz/authz_boundaries.rs`. In scope: every module that calls
    /// `check_custom_app_gates` — reaching that gate *is* the custom-app
    /// identity ([`mark_custom_app_surface`]), so the set grows with the data
    /// plane instead of being a list someone must remember to extend — plus the
    /// two places a hub is *made* rather than inherited from a request: the
    /// function runtime (`custom_apps_functions/`: the isolate thread, the
    /// host-call reply tasks, the blocking-pool hops in `host.rs`) and the job
    /// executor (`app_function_executor.rs`, which mints [`custom_app_hub`] for
    /// a scheduled run). Those are the carries the design names by hand, and
    /// they had no remove-it test until this walk reached them.
    ///
    /// Counting per file, rather than pairing each spawn with its own binding,
    /// is deliberate: a binding sits at the end of the spawned block, tens of
    /// lines from its `tokio::spawn(`, and a proximity rule there would be a
    /// guess. A count still fails on both moves that matter — a binding
    /// removed, or a new unbound spawn added. A file's trailing `#[cfg(test)]
    /// mod` is cut before counting: a spawn inside a test runs under the test's
    /// own hub, and demanding a binding there would prove nothing.
    #[test]
    fn every_custom_app_data_plane_spawn_carries_a_hub() {
        use std::fs;
        use std::path::{Path, PathBuf};

        /// Work that deliberately runs with no custom-app hub, and why.
        struct Unhubbed {
            /// Relative to `crates/app`.
            file: &'static str,
            site: &'static str,
            why: &'static str,
        }

        const UNHUBBED: &[Unhubbed] = &[Unhubbed {
            file: "src/server/api/projects/automation_run.rs",
            site: "spawn_periodic_sweep",
            why: "a boot-time loop started by `router::entry`, not request work: there is \
                  no request hub to inherit, and what it can capture is Oxy's own \
                  `customer_app_procedure_runs` UPDATE failing — a platform bug carrying \
                  no tenant payload, which is what Sentry is still for. A `custom_app_hub()` \
                  here would hide it.",
        }];

        /// A spawn, blocking-pool hop, thread or stream that outlives the hub
        /// it was created under. `spawn_blocking(` is matched bare so the
        /// `tokio::task::` spelling and a `use`d one both count; `thread::spawn(`
        /// likewise covers `std::thread::spawn(`.
        const DETECTORS: [&str; 6] = [
            "tokio::spawn(",
            "tokio::task::spawn(",
            "spawn_blocking(",
            "async_stream::stream!",
            "std::thread::Builder",
            "thread::spawn(",
        ];
        /// The ways to carry the hub across one: `bind_hub` on a future (the
        /// request's hub, or the one [`custom_app_hub`] mints for a job),
        /// [`bind_hub_stream`] on a stream, and `Hub::run` around a blocking
        /// closure or a thread body. The `bind_hub` forms are spelled out in
        /// full so a `.bind_hub(..)` of some other hub is not taken for a carry.
        const BINDINGS: [&str; 4] = [
            ".bind_hub(sentry::Hub::current())",
            ".bind_hub(crate::server::api::middlewares::sentry_surface::custom_app_hub())",
            "bind_hub_stream(",
            "Hub::run(",
        ];

        fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    rust_files(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }

        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&crate_root.join("src/server"), &mut files);
        files.sort();

        let mut scanned = 0usize;
        let mut sites = 0usize;
        for path in files {
            // This module defines both bindings and names every detector in its
            // own tests, so scanning it would only count itself.
            if path
                .file_name()
                .is_some_and(|name| name == "sentry_surface.rs")
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let relative = path
                .strip_prefix(crate_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let makes_its_own_hub = relative.starts_with("src/server/api/custom_apps_functions/")
                || relative == "src/server/app_function_executor.rs";
            if !text.contains("check_custom_app_gates") && !makes_its_own_hub {
                continue;
            }
            // Production code only: a trailing `#[cfg(test)] mod tests` spawns
            // under the test's own hub.
            let text = match text.rfind("#[cfg(test)]\nmod ") {
                Some(cut) => &text[..cut],
                None => text.as_str(),
            };
            let count = |needles: &[&str]| -> usize {
                needles.iter().map(|n| text.matches(n).count()).sum()
            };
            let detected = count(&DETECTORS);
            if detected == 0 {
                continue;
            }
            scanned += 1;
            sites += detected;

            let exempt: Vec<&Unhubbed> = UNHUBBED
                .iter()
                .filter(|entry| entry.file == relative)
                .collect();
            let bound = count(&BINDINGS);

            assert_eq!(
                detected,
                bound + exempt.len(),
                "{relative}: {detected} spawn/blocking/thread/stream site(s), {bound} hub \
                 binding(s), {} documented exemption(s) ({}).\n\n\
                 Work started here outlives the request hub `tag_custom_app_surface` \
                 tagged, so `sentry_config::before_send` can no longer tell it is a \
                 custom app's — and barrier 1 will not catch it either, since a callee's \
                 `error!` and a panic carry no `custom_apps` target. Read \
                 `sentry::Hub::current()` on the spawning task and bind it: \
                 `.bind_hub(..)` on a future, `bind_hub_stream(..)` on a stream, \
                 `sentry::Hub::run(hub, ..)` around a blocking closure or a thread body. \
                 If this site genuinely has no request hub, add it to `UNHUBBED` with \
                 the reason.",
                exempt.len(),
                exempt
                    .iter()
                    .map(|entry| entry.site)
                    .collect::<Vec<_>>()
                    .join(", "),
            );

            for entry in exempt {
                assert!(
                    text.contains(entry.site),
                    "{relative}: exemption for `{}` no longer matches anything in the file \
                     — it was documented as: {}",
                    entry.site,
                    entry.why,
                );
            }
        }

        // The walk itself has to be load-bearing: a bad root, a renamed gate or
        // a moved runtime directory would scan nothing and pass. 11 files and
        // 17 sites when this floor was set.
        assert!(
            scanned >= 9 && sites >= 14,
            "expected the data-plane walk to find at least the known sites, \
             scanned {scanned} file(s) / {sites} site(s)"
        );
    }

    #[test]
    fn a_background_job_hub_carries_the_tag() {
        let events = sentry::test::with_captured_events(|| {
            sentry::Hub::run(custom_app_hub(), || {
                sentry::capture_message("boom", sentry::Level::Error);
            });
        });
        assert_eq!(events.len(), 1, "exactly one captured event");
        assert_eq!(
            events[0]
                .tags
                .get(CUSTOM_APP_SURFACE_TAG)
                .map(String::as_str),
            Some(CUSTOM_APP_SURFACE)
        );
    }
}
