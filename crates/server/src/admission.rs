//! HTTP admission control + per-tenant fairness.
//!
//! Two layers protect the fleet from a request spike:
//!
//! 1. **Global ceiling** (`OXY_MAX_INFLIGHT_REQUESTS`, default 1024) — above it,
//!    new requests are shed with `503 + Retry-After` so a spike backs off
//!    instead of OOM-ing a replica and cascading across the fleet.
//! 2. **Per-workspace fairness** (`OXY_MAX_INFLIGHT_PER_WORKSPACE`, default a
//!    quarter of the global, floor 64) — a single workspace can't hold more than
//!    its share of the budget, so one noisy tenant can't 503 everyone else even
//!    while the global pool still has room.
//!
//! A request holds its slot for the whole call (true concurrency, not arrival
//! rate). Health / liveness probes are NEVER shed: shedding them makes the load
//! balancer evict a merely-busy pod, the opposite of what we want.
//!
//! **Attribution caveat (per-workspace key is request-, not auth-attributed).**
//! This layer sits OUTSIDE the auth/workspace middleware (it must shed before
//! routing/proxy work), so the per-workspace key is the `workspace_id` parsed
//! from the URL — NOT an authenticated identity. A caller can therefore reserve
//! slots against *any* workspace id, including one it can't access, and hold
//! them for the (fast) duration of the downstream auth rejection. We accept this
//! deliberately rather than re-layering accounting after auth: the abuse is
//! bounded three ways — workspace ids are unguessable v4 UUIDs (not enumerable),
//! pinning a victim's quarter-share also consumes that same quarter of the
//! GLOBAL ceiling (so the global limit caps total damage and an attacker can't
//! selectively starve many tenants at once), and an upstream/edge per-IP rate
//! limit bounds the request rate. So item 2's "can't 503 everyone else" holds
//! for ordinary authenticated traffic; it is fairness shaping under the global
//! ceiling, not an auth-attributed quota.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use tokio::sync::Semaphore;
use uuid::Uuid;

const DEFAULT_MAX_INFLIGHT: usize = 1024;

/// Configured global in-flight ceiling (read once).
fn max_inflight() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        let m = std::env::var("OXY_MAX_INFLIGHT_REQUESTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_INFLIGHT);
        tracing::info!(
            max_inflight = m,
            "admission control: global in-flight ceiling"
        );
        m
    })
}

/// Process-wide global permit pool.
fn inflight() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_inflight())))
}

/// Per-workspace in-flight ceiling (fairness). Defaults to a quarter of the
/// global budget (floor 64) so no single tenant monopolises the pool.
///
/// NB the default COUPLES to the global ceiling: on a single-tenant box (one
/// busy workspace) that tenant is capped at 25% of `OXY_MAX_INFLIGHT_REQUESTS`,
/// so raising the global ceiling ALONE gives it no extra headroom. To dedicate
/// a single-tenant deployment's full budget to its one workspace, raise
/// `OXY_MAX_INFLIGHT_PER_WORKSPACE` too (set it `>=` the global ceiling to make
/// per-tenant fairness a no-op).
fn per_workspace_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        let c = std::env::var("OXY_MAX_INFLIGHT_PER_WORKSPACE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| (max_inflight() / 4).max(64));
        tracing::info!(
            per_workspace = c,
            "admission control: per-workspace ceiling"
        );
        c
    })
}

/// Live per-workspace in-flight counts. Entries are removed when they hit zero,
/// so the map stays bounded by the number of *currently active* workspaces.
fn tenant_counts() -> &'static DashMap<Uuid, usize> {
    static COUNTS: OnceLock<DashMap<Uuid, usize>> = OnceLock::new();
    COUNTS.get_or_init(DashMap::new)
}

/// The exemption decision: a `GET` to a probe path. Method-gated like the
/// span layer, so a `POST /api/health` flood is a client's traffic and stays
/// under the ceiling.
fn is_probe_request(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET && is_probe(path)
}

/// Probes that must always be served regardless of load: the paths the
/// telemetry layer treats as probes (one list, `oxy_telemetry::http_trace`)
/// plus `/api/version`. The bare forms are kept for the pre-`/api` shape.
fn is_probe(path: &str) -> bool {
    oxy_telemetry::http_trace::is_probe(path)
        || matches!(
            path,
            "/api/version" | "/health" | "/ready" | "/live" | "/version" | "/healthz" | "/readyz"
        )
}

/// The workspace id in an `/api/{workspace_id}/...` path, if any. Non-workspace
/// paths (`/api/orgs/...`, `/ide`, static) return `None` and skip per-tenant
/// accounting (they fall under the global ceiling only).
fn workspace_id_from_path(path: &str) -> Option<Uuid> {
    Uuid::parse_str(path.strip_prefix("/api/")?.split('/').next()?).ok()
}

/// RAII decrement of a workspace's in-flight count; removes the entry at zero.
struct TenantGuard(Uuid);
impl Drop for TenantGuard {
    fn drop(&mut self) {
        let now_zero = match tenant_counts().get_mut(&self.0) {
            Some(mut n) => {
                *n = n.saturating_sub(1);
                *n == 0
            }
            None => false,
        }; // release the shard lock before remove_if
        if now_zero {
            tenant_counts().remove_if(&self.0, |_, &v| v == 0);
        }
    }
}

/// Reserve one of `ws`'s per-workspace slots, or `None` if it's already at cap.
fn try_enter_tenant(ws: Uuid, cap: usize) -> Option<TenantGuard> {
    let mut count = tenant_counts().entry(ws).or_insert(0);
    if *count >= cap {
        return None; // shard lock released on return; nothing reserved
    }
    *count += 1;
    drop(count); // release the shard lock before awaiting downstream
    Some(TenantGuard(ws))
}

/// Axum middleware. Place it OUTSIDE `enforce_role` (so it sheds before routing /
/// proxy work) but inside CORS (so the 503 still carries CORS headers).
pub async fn admission_control(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if is_probe_request(req.method(), path) {
        return next.run(req).await;
    }

    // Per-workspace fairness first: shed a tenant over its share even while the
    // global pool has room. `_tenant` releases the slot on drop (any exit path).
    // NB the workspace id here is request-attributed (from the URL), not yet
    // authenticated — see the module-level "Attribution caveat". The global
    // ceiling below is the hard backstop on total damage.
    let _tenant = match workspace_id_from_path(path) {
        Some(ws) => match try_enter_tenant(ws, per_workspace_cap()) {
            Some(guard) => Some(guard),
            None => return shed("per-workspace concurrency limit reached"),
        },
        None => None,
    };

    // Global ceiling. The owned permit lives for the whole request.
    match Arc::clone(inflight()).try_acquire_owned() {
        Ok(_permit) => next.run(req).await,
        Err(_) => shed("server is at capacity"),
    }
}

fn shed(reason: &'static str) -> Response {
    let n = shed_total().fetch_add(1, Ordering::Relaxed) + 1;
    // Throttle the warn so sustained overload doesn't drown the log.
    if n == 1 || n.is_multiple_of(256) {
        tracing::warn!(
            shed_total = n,
            reason,
            "admission control: shedding load (503 + Retry-After)"
        );
    }
    let mut resp = (
        StatusCode::SERVICE_UNAVAILABLE,
        "server is busy; please retry shortly",
    )
        .into_response();
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    resp
}

/// Total requests shed since process start (for the warn throttle + future
/// metrics export).
fn shed_total() -> &'static AtomicU64 {
    static N: AtomicU64 = AtomicU64::new(0);
    &N
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_are_exempt_from_shedding() {
        for p in [
            "/api/health",
            "/api/ready",
            "/api/live",
            "/api/version",
            "/health",
            "/ready",
            "/live",
            "/version",
            "/healthz",
            "/readyz",
        ] {
            assert!(is_probe(p), "{p} must be exempt from admission control");
        }
        for p in ["/api/x/threads", "/api/x/analytics/runs", "/ide", "/"] {
            assert!(!is_probe(p), "{p} must be subject to admission control");
        }
    }

    #[test]
    fn only_a_get_to_a_probe_path_is_exempt() {
        use axum::http::Method;
        assert!(is_probe_request(&Method::GET, "/api/live"));
        assert!(
            !is_probe_request(&Method::POST, "/api/live"),
            "a client's 405 is shed like any request"
        );
        assert!(!is_probe_request(&Method::HEAD, "/api/health"));
        assert!(!is_probe_request(&Method::GET, "/api/x/threads"));
    }

    #[test]
    fn workspace_id_extracted_only_from_api_workspace_paths() {
        let ws = "d9830be4-c6a4-4e3a-9b21-000000000001";
        let parsed = Uuid::parse_str(ws).unwrap();
        assert_eq!(
            workspace_id_from_path(&format!("/api/{ws}/analytics/runs")),
            Some(parsed)
        );
        assert_eq!(workspace_id_from_path(&format!("/api/{ws}")), Some(parsed));
        // Org routes + non-api + non-uuid → no per-tenant accounting.
        assert_eq!(workspace_id_from_path("/api/orgs/acme/onboarding"), None);
        assert_eq!(workspace_id_from_path("/ide"), None);
        assert_eq!(workspace_id_from_path("/healthz"), None);
    }

    #[test]
    fn tenant_guard_reserves_and_releases() {
        let ws = Uuid::parse_str("d9830be4-c6a4-4e3a-9b21-000000000002").unwrap();
        // Under cap: reserve succeeds; count reflects it.
        let g1 = try_enter_tenant(ws, 2).expect("first slot");
        let g2 = try_enter_tenant(ws, 2).expect("second slot");
        assert_eq!(*tenant_counts().get(&ws).unwrap(), 2);
        // At cap: reserve fails, count unchanged.
        assert!(try_enter_tenant(ws, 2).is_none(), "over cap is rejected");
        assert_eq!(*tenant_counts().get(&ws).unwrap(), 2);
        // Releasing frees slots; the entry is removed at zero.
        drop(g1);
        drop(g2);
        assert!(tenant_counts().get(&ws).is_none(), "entry removed at zero");
    }
}
