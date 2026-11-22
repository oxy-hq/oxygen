//! Probe + metrics surface for `oxy worker`.
//!
//! Exposes `GET /healthz`, `GET /readyz` and — when the caller passes
//! metrics state, which today is exactly when `--health-port` /
//! `OXY_WORKER_HEALTH_PORT` is set on `oxy worker` — `GET /metrics`, all on
//! one dedicated TCP port. `spawn` is `pub` and still supports `None` for
//! tests and local mode, so treat the flag as the current caller rather than
//! the rule. No Oxy *application* routes are mounted, so a probe can never
//! reach an authenticated endpoint.
//!
//! It is NOT stateless. `/metrics` makes two DB round trips per scrape (see
//! `worker_metrics::metrics`), so this listener is DB-touching and
//! unauthenticated. Keep it cluster-internal — never behind a Service or
//! Ingress that faces anything else.
//!
//! That is a deployment constraint, NOT something this file enforces: the
//! bind below is `0.0.0.0`, so on a host-network pod or a node with a public
//! IP the port is reachable by anything that can route to it. Enforce it with
//! a NetworkPolicy or by not publishing the port. The exposure matters
//! because the queue-depth query is a sequential scan over every retained row
//! (`worker_metrics::read_queue_depth`), which is unbounded if either
//! retention TTL is disabled.
//!
//! The original "probe-only, so it can't hit stateful endpoints" guarantee
//! stopped being true when `/metrics` landed here.
//!
//! Lifetime is tied to the same umbrella `CancellationToken` the worker
//! uses, so `Ctrl+C`/SIGTERM cleanly tears the health server down with the
//! rest of the worker.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use oxy_shared::errors::OxyError;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Liveness/readiness state shared between the worker and the probe handlers.
///
/// `Arc<AtomicBool>` flags keep the read path lock-free for `/readyz`, which
/// k8s typically polls every few seconds across the entire fleet.
#[derive(Clone)]
pub struct WorkerHealthState {
    started_at: Instant,
    version: &'static str,
    worker_id: Arc<String>,
    router_connected: Arc<AtomicBool>,
    recovery_alive: Arc<AtomicBool>,
}

impl WorkerHealthState {
    pub fn new(version: &'static str, worker_id: String) -> Self {
        Self {
            started_at: Instant::now(),
            version,
            worker_id: Arc::new(worker_id),
            router_connected: Arc::new(AtomicBool::new(false)),
            recovery_alive: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn router_connected_handle(&self) -> Arc<AtomicBool> {
        self.router_connected.clone()
    }

    pub fn recovery_alive_handle(&self) -> Arc<AtomicBool> {
        self.recovery_alive.clone()
    }
}

/// Spawn the health + metrics server on `port`. Returns the join
/// handle so the shutdown path can drain it. Bind errors propagate
/// as `OxyError`. The `metrics_state` argument is optional: if
/// `None`, only `/healthz` + `/readyz` are mounted (handy for tests
/// or local-mode where the queue-depth DB read isn't wanted).
pub async fn spawn(
    port: u16,
    state: WorkerHealthState,
    metrics_state: Option<crate::worker_metrics::MetricsState>,
    shutdown: CancellationToken,
) -> Result<JoinHandle<()>, OxyError> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("worker health: bind {addr}: {e}")))?;

    let health_router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state);

    let router = if let Some(ms) = metrics_state {
        let metrics_router = Router::new()
            .route("/metrics", get(crate::worker_metrics::metrics))
            .with_state(ms);
        Router::new().merge(health_router).merge(metrics_router)
    } else {
        health_router
    };

    let serve_token = shutdown.clone();
    let handle = tokio::spawn(async move {
        tracing::info!(target: "worker.health", %addr, "health server listening");
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                serve_token.cancelled().await;
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(target: "worker.health", error = ?e, "health server exited with error");
        } else {
            tracing::info!(target: "worker.health", "health server stopped");
        }
    });
    Ok(handle)
}

async fn healthz(
    axum::extract::State(state): axum::extract::State<WorkerHealthState>,
) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "version": state.version,
            "worker_id": state.worker_id.as_str(),
            "uptime_secs": uptime,
        })),
    )
}

async fn readyz(
    axum::extract::State(state): axum::extract::State<WorkerHealthState>,
) -> impl IntoResponse {
    let router_ok = state.router_connected.load(Ordering::Relaxed);
    let recovery_ok = state.recovery_alive.load(Ordering::Relaxed);
    let ready = router_ok && recovery_ok;
    let code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    // TODO(worker-fleet): `router_connected` only flips once at startup —
    // it does not yet reflect LISTEN socket loss or reconnect attempts. So
    // /readyz can report ready:true during a Postgres outage. The LISTEN
    // keepalive currently masks this for short outages, but a long Postgres
    // outage should drop us out of the LB. Fix by exposing a "connection
    // healthy" signal from `PostgresTaskRouter` and watching it here.
    // Tracked alongside the scope-survey items in
    // `internal-docs/worker-fleet.md`.
    (
        code,
        Json(json!({
            "ready": ready,
            "router_connected": router_ok,
            "recovery_alive": recovery_ok,
        })),
    )
}
