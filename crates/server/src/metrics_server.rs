//! `GET /metrics` for the roles that are not `oxy worker`.
//!
//! ## Why this exists at all
//!
//! Until now the only `/metrics` in the process was `worker_metrics`, mounted
//! on `oxy worker`'s health port. `oxy serve` and `oxy ide` bound no metrics
//! port, so the entire serve fleet — every HTTP route, every custom app, every
//! V8 isolate — exported no metrics whatsoever.
//!
//! That had a concrete consequence beyond the missing signal.
//! `oxy_abandoned_isolates_total` counts isolate threads wedged in a host call,
//! and it was reachable only through the worker's endpoint. But **route-mode**
//! invocations — `/customer-apps/**` and `/fn`, the bulk of them — run on
//! **serve**, which published nothing. Standing this endpoint up is what makes
//! that metric, and every metric this work adds, reachable from the fleet that
//! produces most of them.
//!
//! An earlier version of this comment said the worker "never runs a function"
//! and that an alert could "never have fired". Not true: scheduled and job-mode
//! invocations are `TaskSpec`s of kind `app_function` that the worker fleet
//! claims, so the worker does create isolates — measured on oxy-dev,
//! `oxy_custom_app_isolates_live_peak` on a worker pod read 1. The real gap was
//! that the counter covered scheduled runs only.
//!
//! ## Why a separate port rather than the main router
//!
//! Three reasons, in order of weight:
//!
//! 1. **It is not public.** The main router is behind the ALB; a separate port
//!    is reachable only in-cluster, which is where a scraper lives. Mounting
//!    `/metrics` on the public router would need an auth decision, and the
//!    right auth for a scrape endpoint is "not routable from outside".
//! 2. **No route classification.** A route under `server/router/` has to be
//!    classified in `role_manifest.rs` and would go through admission,
//!    role-routing and the request-id and trace layers. A scrape needs none of
//!    it, and `oxy_http_server_request_duration_seconds` should not count its
//!    own scrapes.
//! 3. It is the shape `worker_health` already uses, so an operator configures
//!    one thing, not two.
//!
//! ## This port must not be exposed
//!
//! It binds `0.0.0.0`, matching `worker_health`. "In-cluster only" is therefore
//! a **network-policy property, not a code one** — nothing here authenticates a
//! scrape. The body carries org and app UUIDs and per-route latency, so it is
//! operational data about tenants even though it holds none of their content.
//! Do not add it to an ingress, a Service of type LoadBalancer, or an ALB
//! target group. `internal-docs/platform-metrics.md` says the same thing where
//! an operator will read it.

use std::net::SocketAddr;

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use oxy_shared::errors::OxyError;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Port for the metrics server. Unset means no port is bound — the same
/// default-off posture as `OXY_WORKER_HEALTH_PORT`.
pub const METRICS_PORT_ENV: &str = "OXY_METRICS_PORT";

/// Resolve the port from `arg`, else [`METRICS_PORT_ENV`], else off.
pub fn resolve_metrics_port(arg: Option<u16>) -> Option<u16> {
    arg.or_else(|| {
        std::env::var(METRICS_PORT_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u16>().ok())
    })
}

/// Bind the metrics server and serve until `shutdown` fires.
pub async fn start(port: u16, shutdown: CancellationToken) -> Result<JoinHandle<()>, OxyError> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("metrics server: bind {addr}: {e}")))?;

    let router = Router::new().route("/metrics", get(metrics));

    let serve_token = shutdown.clone();
    let handle = tokio::spawn(async move {
        tracing::info!(target: "oxy.metrics", %addr, "metrics server listening");
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                serve_token.cancelled().await;
            })
            .await;
        match result {
            Ok(()) => tracing::info!(target: "oxy.metrics", "metrics server stopped"),
            Err(e) => {
                tracing::warn!(target: "oxy.metrics", error = ?e, "metrics server exited with error")
            }
        }
    });
    Ok(handle)
}

/// The scrape handler.
///
/// Unlike the worker's, this one makes **no database round trip** — every
/// series it serves is read out of in-process instrument state. That keeps a
/// scrape off the critical path of a fleet whose whole job is serving requests,
/// and it means a Postgres outage does not also blind the metrics that would
/// explain it.
async fn metrics() -> Response {
    let body = oxy_telemetry::metrics::render_prometheus();
    if body.is_empty() {
        // A 200 with an empty body reads to a scraper as "this target is
        // healthy and has no series", which is indistinguishable from a
        // correctly-working target on an idle process. It is neither: it means
        // no meter provider was installed, which is a configuration fault.
        // 503 makes `up` go to 0 and says so out loud.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("content-type", "text/plain; charset=utf-8")],
            "no metrics provider is installed on this process\n",
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(
            "content-type",
            oxy_telemetry::metrics::exposition::CONTENT_TYPE,
        )],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_port_beats_the_environment() {
        unsafe { std::env::set_var(METRICS_PORT_ENV, "9100") };
        assert_eq!(resolve_metrics_port(Some(7777)), Some(7777));
        assert_eq!(resolve_metrics_port(None), Some(9100));
        unsafe { std::env::remove_var(METRICS_PORT_ENV) };
        assert_eq!(
            resolve_metrics_port(None),
            None,
            "default is off — binding a port nobody asked for is a surprise in \
             every deployment that has not planned for it"
        );
    }

    #[test]
    fn an_unparseable_port_is_off_rather_than_a_panic() {
        unsafe { std::env::set_var(METRICS_PORT_ENV, "not-a-port") };
        assert_eq!(resolve_metrics_port(None), None);
        unsafe { std::env::remove_var(METRICS_PORT_ENV) };
    }

    /// A process with no provider must not answer 200-with-nothing: to a
    /// scraper that is indistinguishable from a healthy, idle target, and it is
    /// the failure mode that let `oxy_abandoned_isolates_total` read as calm
    /// forever on the fleet that could not produce it.
    #[tokio::test]
    async fn an_uninstalled_provider_is_a_503_not_an_empty_200() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        assert!(
            !oxy_telemetry::metrics::is_installed(),
            "this test needs a process with no provider; nextest gives each \
             test its own process, so something in THIS test installed one"
        );

        let res = Router::new()
            .route("/metrics", get(metrics))
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .expect("router responded");
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// The whole path, end to end: install the provider the way `main` does,
    /// record through the same typed entry point the custom-app telemetry uses,
    /// and read it back off the HTTP endpoint a scraper would hit.
    ///
    /// The unit tests in `oxy-telemetry` prove the renderer; this proves the
    /// *wiring* — that the provider a process installs is the one this handler
    /// serves from.
    #[tokio::test]
    async fn the_endpoint_serves_what_was_recorded() {
        use axum::body::Body;
        use axum::http::Request;
        use oxy_telemetry::metrics::{MetricsConfig, init, record};
        use tower::ServiceExt;

        let problems = init(
            &MetricsConfig {
                sdk_disabled: false,
                // No OTLP: the point is that the scrape path stands alone.
                otlp_enabled: false,
                otlp_endpoint: None,
                export_interval: std::time::Duration::from_secs(60),
            },
            oxy_telemetry::resource::build(Some("serve")),
        );
        assert!(
            problems.is_empty(),
            "provider install reported {problems:?}"
        );

        record::custom_app_function("org-1", "app-1", "handler", "success", 0.25, Some(0.006), 4);

        let res = Router::new()
            .route("/metrics", get(metrics))
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .expect("router responded");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some(oxy_telemetry::metrics::exposition::CONTENT_TYPE),
        );

        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(
            body.contains("oxy_custom_app_function_duration_seconds_count"),
            "the recorded invocation is missing from:\n{body}"
        );
        assert!(
            body.contains(r#"oxy_org_id="org-1""#),
            "org must always be labelled:\n{body}"
        );
        assert!(
            body.contains(r#"oxy_app="app-1""#),
            "an app is labelled by its id by default, with nothing configured:\n{body}"
        );
        assert!(
            body.contains(r#"oxy_function="handler""#),
            "and so is its function:\n{body}"
        );
        assert!(
            body.contains("oxy_db_pool_connections"),
            "observable gauges must be collected on scrape, not only on record:\n{body}"
        );
        assert!(
            body.contains("oxy_custom_app_isolates_abandoned_total"),
            "the isolate counter has to be reachable from THIS fleet — that is \
             the bug this endpoint exists to fix:\n{body}"
        );
    }
}
