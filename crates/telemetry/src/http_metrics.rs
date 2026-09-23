//! RED metrics for every inbound HTTP request.
//!
//! ## Why this is a separate layer from [`crate::http_trace`]
//!
//! The obvious move is to record the histogram inside `OxyOnResponse`, which
//! already has the latency and the status in hand. It does not work:
//! `tower_http`'s `OnResponse` receives only the response, the latency and the
//! `Span`, and the two attributes a metric needs most — method and route — live
//! on the *request*. `MakeSpan` sees the request but takes it immutably, so it
//! cannot stash them in an extension either, and a `tracing` field cannot be
//! read back out of a span.
//!
//! A middleware that owns the whole request/response pair has all four facts
//! and can time it itself. That is this module.
//!
//! ## The route label
//!
//! `axum::extract::MatchedPath` — the route *pattern*, not the URL. This is the
//! same source `http_trace` uses for `http.route`, and reusing it is deliberate
//! rather than incidental: a second path parser would eventually drift, and the
//! failure mode of drifting here is a raw path (`/invitations/abc123/accept`)
//! becoming a label value, which is both a token in a metrics store and an
//! unbounded cardinality axis.
//!
//! Note the division of labour `http_trace` documents: the *pattern* is always
//! safe to record, because the secret lives in the value the pattern elides.
//! `url.path` needs redaction; `http.route` does not.
//!
//! Requests that match no route — the static fallback, an unknown path — carry
//! no route label at all, exactly as `http_trace` names their span by method
//! alone. One series per asset would swamp both the store and the dashboard.
//!
//! ## The method label is an allowlist, not the request's word for it
//!
//! `Method::from_bytes` validates that a method is a legal RFC 9110 *token*,
//! not that it is one of the nine standard verbs — so `curl -X ZZZZ1` mints a
//! method hyper will happily parse. Copied verbatim into a label, an
//! unauthenticated client on the public ALB could mint a new series per
//! request, multiplied by route and status. Either that grows the `/metrics`
//! body without bound, or it hits the SDK's per-instrument cardinality cap and
//! **real route series get folded into the overflow bucket** — a silent loss of
//! exactly the signal this module exists to provide.
//!
//! Semconv's remedy, followed here: allowlist the known methods and emit
//! `_OTHER` for anything else. (The raw value belongs on
//! `http.request.method_original`, a *span* attribute, not a metric label.)
//!
//! Health probes **are** recorded. They are real requests, and a store that
//! quietly omits some traffic is worse than a dashboard that filters it.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use http::Method;
use opentelemetry::KeyValue;

use crate::metrics;

/// The label a non-standard method folds into, per the HTTP semantic
/// conventions.
pub const METHOD_OTHER: &str = "_OTHER";

/// Raises `http.server.active_requests` for as long as it is held.
///
/// A guard rather than a bare pair of `add` calls, because **the decrement has
/// to survive the request future being dropped**. Axum and hyper drop the
/// service future when the client disconnects, so every request abandoned
/// before its response head exists — an impatient user on a slow warehouse
/// query, an LB idle timeout, a cancelled SSE open — would otherwise increment
/// and never decrement. The counter is cumulative for the process lifetime, so
/// that drift is monotonic and never self-heals: the saturation signal the
/// module doc sells as "a queue builds here before it shows up as latency"
/// would instead climb forever on a perfectly healthy fleet.
///
/// Same reasoning as `metrics::sources::IsolateGuard`, for the same reason.
struct InFlight(Vec<KeyValue>);

impl InFlight {
    fn enter(attrs: Vec<KeyValue>) -> Self {
        metrics::with_instruments(|i| i.http_active_requests.add(1, &attrs));
        Self(attrs)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // Byte-identical attributes to the increment, by construction: the
        // guard owns the vector rather than rebuilding it. A mismatch would
        // leak a series that nothing ever decrements.
        metrics::with_instruments(|i| i.http_active_requests.add(-1, &self.0));
    }
}

/// Record one request's duration and keep the in-flight gauge honest.
///
/// Mount with `Router::layer(axum::middleware::from_fn(record))`, alongside the
/// trace layer — `layer` runs after axum has matched, which is what makes
/// `MatchedPath` present.
pub async fn record(request: Request, next: Next) -> Response {
    let method = method_label(request.method());
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());

    let inflight = InFlight::enter(inflight_attributes(method, route.as_deref()));

    let started = Instant::now();
    // If the caller disconnects here the future is dropped, `inflight` drops
    // with it, and the gauge comes back down. Nothing below runs.
    let response = next.run(request).await;
    let elapsed = started.elapsed().as_secs_f64();
    let status = response.status();

    metrics::with_instruments(|i| {
        let mut attrs = inflight.0.clone();
        attrs.push(KeyValue::new(
            "http.response.status_code",
            i64::from(status.as_u16()),
        ));
        // Semconv: `error.type` is set for a 5xx and left absent otherwise — a
        // 4xx is the client's problem and must not inflate an error-rate panel
        // built on this attribute's presence.
        if status.is_server_error() {
            attrs.push(KeyValue::new("error.type", status.as_u16().to_string()));
        }
        i.http_request_duration.record(elapsed, &attrs);
    });

    response
}

/// The nine methods RFC 9110 defines. Anything else is [`METHOD_OTHER`].
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::CONNECT => "CONNECT",
        Method::OPTIONS => "OPTIONS",
        Method::TRACE => "TRACE",
        Method::PATCH => "PATCH",
        _ => METHOD_OTHER,
    }
}

fn inflight_attributes(method: &'static str, route: Option<&str>) -> Vec<KeyValue> {
    let mut attrs = Vec::with_capacity(2);
    attrs.push(KeyValue::new("http.request.method", method));
    if let Some(route) = route {
        attrs.push(KeyValue::new("http.route", route.to_owned()));
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/api/threads/{id}", get(|| async { "ok" }))
            .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
            .fallback(|| async { (StatusCode::NOT_FOUND, "nope") })
            .layer(axum::middleware::from_fn(record))
    }

    async fn call(uri: &str) -> StatusCode {
        app()
            .oneshot(HttpRequest::get(uri).body(Body::empty()).unwrap())
            .await
            .expect("router responded")
            .status()
    }

    /// The layer must be transparent. It sits in front of every request, so a
    /// bug here is a site-wide outage rather than a missing metric.
    #[tokio::test]
    async fn requests_pass_through_untouched() {
        assert_eq!(call("/api/threads/7").await, StatusCode::OK);
        assert_eq!(call("/boom").await, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(call("/nothing/here").await, StatusCode::NOT_FOUND);
    }

    /// The whole cardinality argument rests on this: a routed request is
    /// labelled by its *pattern*, and an unrouted one carries no route at all.
    #[test]
    fn a_routed_request_is_labelled_by_pattern_and_an_unrouted_one_is_not() {
        let routed = inflight_attributes("GET", Some("/api/threads/{id}"));
        assert_eq!(routed.len(), 2);
        assert_eq!(routed[1].key.as_str(), "http.route");
        assert_eq!(routed[1].value.to_string(), "/api/threads/{id}");

        let unrouted = inflight_attributes("GET", None);
        assert_eq!(
            unrouted.len(),
            1,
            "an unmatched path must not become a label, or every static asset \
             is its own series"
        );
    }

    /// The test above proves `inflight_attributes` labels whatever route it is
    /// handed. It cannot prove `record` hands it the right thing — it passes
    /// the pattern in directly, so replacing the `MatchedPath` lookup with
    /// `request.uri().path()` leaves it green while every `/api/threads/<uuid>`
    /// becomes its own series.
    ///
    /// This drives a real request through the layer and reads the label back
    /// off the rendered exposition, which is the only place the extraction is
    /// observable. The negative assertion is the load-bearing one: the URL's
    /// concrete id must appear nowhere in the body.
    ///
    /// Needs its own process because `init` installs into a `OnceLock` — which
    /// nextest gives it, and `cargo test` would not. This repo mandates
    /// nextest.
    #[tokio::test]
    async fn the_recorded_route_label_is_the_pattern_not_the_url() {
        use crate::metrics::{MetricsConfig, init, render_prometheus};

        let problems = init(
            &MetricsConfig {
                sdk_disabled: false,
                otlp_enabled: false,
                otlp_endpoint: None,
                export_interval: std::time::Duration::from_secs(60),
            },
            crate::resource::build(Some("serve")),
        );
        assert!(
            problems.is_empty(),
            "provider install reported {problems:?}"
        );

        assert_eq!(call("/api/threads/7").await, StatusCode::OK);

        let body = render_prometheus();
        assert!(
            body.contains(r#"http_route="/api/threads/{id}""#),
            "the route label must be the matched PATTERN:\n{body}"
        );
        assert!(
            !body.contains("/api/threads/7"),
            "the concrete URL leaked into a label — that is one series per id, \
             which is the cardinality explosion the pattern exists to prevent:\n{body}"
        );
    }

    /// Hyper parses any RFC 9110 token as a method, so an unauthenticated
    /// caller controls this value. Folding the unknown ones is what keeps the
    /// axis from being attacker-controlled.
    #[test]
    fn a_nonstandard_method_folds_to_other() {
        for standard in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::CONNECT,
            Method::OPTIONS,
            Method::TRACE,
            Method::PATCH,
        ] {
            assert_eq!(
                method_label(&standard),
                standard.as_str(),
                "a standard method must keep its own name"
            );
        }

        let minted = Method::from_bytes(b"ZZZZ1").expect("hyper accepts any token");
        assert_eq!(
            method_label(&minted),
            METHOD_OTHER,
            "an attacker-minted method must not become its own series"
        );
    }

    /// The failure this guards: a client that disconnects mid-request leaves
    /// the in-flight counter permanently high, and because the counter is
    /// cumulative the drift never self-heals.
    #[tokio::test]
    async fn a_dropped_request_future_still_decrements() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dropped = Arc::new(AtomicBool::new(false));

        // A sentinel that flips when the guard's scope unwinds, standing in for
        // the gauge (which is a no-op without an installed provider).
        struct Sentinel(Arc<AtomicBool>);
        impl Drop for Sentinel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        {
            let _guard = InFlight::enter(inflight_attributes("GET", Some("/x")));
            let _sentinel = Sentinel(Arc::clone(&dropped));
            assert!(!dropped.load(Ordering::SeqCst));
        }
        assert!(
            dropped.load(Ordering::SeqCst),
            "the in-flight guard must release on scope exit, which is what \
             covers a dropped request future"
        );
    }
}
