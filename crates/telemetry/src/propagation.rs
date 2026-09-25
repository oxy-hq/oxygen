//! W3C `traceparent` / `tracestate` on the wire.
//!
//! Inbound, the HTTP span in [`crate::http_trace`] adopts a valid, sampled
//! `traceparent` as its parent, so a browser SDK or an upstream service can
//! hand Oxy the trace it started (an unsampled one is only linked, so a
//! caller's sampling decision can't drop Oxy's span). Outbound, the one internal hop Oxy makes on
//! its own behalf — a stateless `serve` replica forwarding an `IdeOnly` route
//! to the `ide` pod — stamps the current span's context on the forwarded
//! request, so both pods' spans land in one trace instead of two that share
//! only an `x-oxy-request-id`.
//!
//! Neither direction needs an OTLP endpoint: the `otel` layer runs on every
//! server-shaped process, so the hop is linked even on a cluster with no
//! receiver yet. Both are no-ops only without that layer (`OTEL_SDK_DISABLED`,
//! or a one-shot CLI command), where there is no span context to read and an
//! inbound header is simply left untouched for whoever is downstream.

use http::{HeaderMap, HeaderValue};
use opentelemetry::Context;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// The parent context carried by `traceparent` / `tracestate`, when the
/// headers hold a well-formed one. Anything malformed reads as "no parent".
pub fn extract(headers: &HeaderMap) -> Option<Context> {
    let cx = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(headers))
    });
    cx.span().span_context().is_valid().then_some(cx)
}

/// Write the current `tracing` span's context into `headers` as
/// `traceparent` (+ `tracestate`), replacing any inbound value. Does nothing
/// when no traced span is active — an event outside any span, or a process
/// without the `otel` layer.
pub fn inject_current(headers: &mut HeaderMap) {
    let cx = tracing::Span::current().context();
    if !cx.span().span_context().is_valid() {
        return;
    }
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderInjector(headers))
    });
}

/// Parent `span` on whatever span is entered on this thread — by OpenTelemetry
/// context (a pair of ids), not by `tracing` span handle. Nothing is pinned
/// open: the parent closes on its own schedule, which is why this is safe for
/// a span that outlives its caller (an isolate thread after a cancel). Call it
/// before `span` is entered; a span already started keeps its parent.
pub fn adopt_current_parent(span: &tracing::Span) {
    let cx = tracing::Span::current().context();
    if cx.span().span_context().is_valid() {
        let _ = span.set_parent(cx);
    }
}

/// The current span's context as a W3C `traceparent` value, for a payload
/// that crosses a queue rather than an HTTP hop. `None` outside a traced span.
pub fn current_traceparent() -> Option<String> {
    let mut headers = HeaderMap::new();
    inject_current(&mut headers);
    headers
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Link `span` to the context a `traceparent` value carries — a *link*, not a
/// parent: the right relation for work caused by, but not part of, a request
/// (a scheduled run and the tick that queued it). `false` when the value is
/// malformed or no export layer is installed.
pub fn link_from_traceparent(span: &tracing::Span, traceparent: &str) -> bool {
    let Ok(value) = HeaderValue::from_str(traceparent) else {
        return false;
    };
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", value);
    match extract(&headers) {
        Some(cx) => {
            span.add_link(cx.span().span_context().clone());
            true
        }
        None => false,
    }
}

/// The current span's ids as the lowercase hex a `traceparent` carries —
/// `(trace_id, span_id)` — for a row that should join the platform trace.
/// `None` outside a traced span.
pub fn current_ids() -> Option<(String, String)> {
    let cx = tracing::Span::current().context();
    let span = cx.span();
    let sc = span.span_context();
    sc.is_valid().then(|| {
        (
            format!("{:032x}", sc.trace_id()),
            format!("{:016x}", sc.span_id()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::layer::SubscriberExt;

    fn install_propagator() {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    }

    #[test]
    fn without_an_exported_span_nothing_is_injected() {
        install_propagator();
        let mut headers = HeaderMap::new();
        inject_current(&mut headers);
        assert!(headers.is_empty());
    }

    #[test]
    fn injects_the_current_span_and_extract_reads_it_back() {
        install_propagator();
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));

        let mut headers = HeaderMap::new();
        let expected = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("hop");
            let _g = span.enter();
            inject_current(&mut headers);
            let sc = span.context().span().span_context().clone();
            (
                format!("{:032x}", sc.trace_id()),
                format!("{:016x}", sc.span_id()),
            )
        });

        let traceparent = headers["traceparent"].to_str().unwrap().to_string();
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(parts.len(), 4, "{traceparent}");
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1], expected.0);
        assert_eq!(parts[2], expected.1);

        let parent = extract(&headers).expect("a valid traceparent extracts");
        let sc = parent.span().span_context().clone();
        assert!(sc.is_remote());
        assert_eq!(format!("{:032x}", sc.trace_id()), expected.0);
    }

    #[test]
    fn adopting_the_current_parent_joins_the_trace_without_a_span_handle() {
        install_propagator();
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!("request");
            let _g = request.enter();
            // `parent: None` is the isolate's shape: no tracing parent, so
            // nothing keeps `request` open — yet it lands in the same trace.
            let isolate = tracing::info_span!(parent: None, "isolate");
            adopt_current_parent(&isolate);
            let trace = |s: &tracing::Span| {
                format!("{:032x}", s.context().span().span_context().trace_id())
            };
            assert_eq!(trace(&isolate), trace(&request));
            assert!(current_ids().is_some());
            assert!(current_traceparent().unwrap().starts_with("00-"));
        });
        assert!(current_ids().is_none(), "no span, no ids");
        assert!(current_traceparent().is_none());
    }

    #[test]
    fn a_link_is_added_from_a_traceparent_value() {
        install_propagator();
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("job");
            assert!(link_from_traceparent(
                &span,
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
            ));
            assert!(!link_from_traceparent(&span, "garbage"));
        });
    }

    #[test]
    fn a_malformed_traceparent_reads_as_no_parent() {
        install_propagator();
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", "garbage".parse().unwrap());
        assert!(extract(&headers).is_none());
        assert!(extract(&HeaderMap::new()).is_none());
    }
}
