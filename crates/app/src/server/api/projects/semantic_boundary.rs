//! Shared prelude for customer-app semantic-analysis endpoints
//! (`metric-tree` + `world-model`).
//!
//! Both surfaces run the same dance before they can touch the semantic
//! model: pass the customer-app gate chain, build the project context, and
//! resolve a semantic-model scan directory. On the stateless serve fleet —
//! where customer apps actually run — the workspace FS scan path does not
//! exist, so the layer must be materialised from the compile boundary
//! (Postgres `semantic_views` / `semantic_topics` rows) into a tempdir.
//! This mirrors the `semantic_query.rs` prelude verbatim, including the
//! `X-Oxy-Needs-Recompile` 503 contract when the boundary isn't populated.
//!
//! Extracted so the metric-tree and world-model handler modules don't each
//! reimplement (and drift on) the gate + boundary logic.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use crate::agentic_wiring::OxyProjectContext;
use crate::server::api::custom_apps_gates::{CustomAppContext, check_custom_app_gates};
use crate::server::api::semantic_scan::{ScanDir, scan_dir};

#[derive(Serialize)]
struct ApiErr {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

/// `{ message }` JSON error — same envelope the other customer-app
/// endpoints use, so the SDK renders errors uniformly.
pub(crate) fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: None,
        }),
    )
        .into_response()
}

/// `{ message, code }` JSON error — `code` lets the SDK pattern-match.
pub(crate) fn err_with_code(
    status: StatusCode,
    msg: impl Into<String>,
    code: &'static str,
) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
        }),
    )
        .into_response()
}

/// The resolved semantic-model scan directory. `Materialised` holds the
/// compile-boundary tempdir guard — it MUST stay alive until every layer
/// parse / query finishes, so callers keep the whole [`SemanticBoundary`]
/// in scope for the duration of the request.
pub(crate) enum ScanHandle {
    /// Compile-boundary tempdir (the serve-fleet path).
    Materialised(ScanDir),
    /// Workspace FS scan path (local / ide path, when the boundary is empty
    /// but we're not on a stateless replica).
    WorkingCopy(PathBuf),
}

impl ScanHandle {
    /// Directory to parse the semantic model from.
    pub(crate) fn path(&self) -> &Path {
        match self {
            ScanHandle::Materialised(m) => m.path(),
            ScanHandle::WorkingCopy(p) => p.as_path(),
        }
    }

    /// Owned copy of the scan path, for threading into `spawn_blocking` /
    /// the metric-tree runner.
    pub(crate) fn path_buf(&self) -> PathBuf {
        self.path().to_path_buf()
    }
}

/// Everything a semantic-analysis handler needs after the shared prelude:
/// the gate context, the per-request project context, and the resolved scan
/// directory. Keep this value alive for the whole request — dropping it
/// early would delete the materialised tempdir out from under an in-flight
/// query.
pub(crate) struct SemanticBoundary {
    pub app: CustomAppContext,
    pub proj_ctx: OxyProjectContext,
    pub scan: ScanHandle,
}

impl SemanticBoundary {
    pub(crate) fn project_id(&self) -> Uuid {
        self.app.project_id
    }
}

/// Run the customer-app gate chain, build the project context, and resolve
/// the semantic-model scan directory (compile boundary first, FS fallback).
///
/// On a stateless serve replica with no materialised layer, returns the same
/// `503 + X-Oxy-Needs-Recompile` contract as `semantic_query.rs` (and
/// enqueues a lazy recompile) so the SDK's retry path kicks in rather than
/// the handler compiling against an empty directory.
pub(crate) async fn enter_semantic_boundary(
    headers: &HeaderMap,
    project_id: Uuid,
) -> Result<SemanticBoundary, Response> {
    // 1. Shared gates (auth → origin → workspace → org membership).
    let app = check_custom_app_gates(headers, project_id).await?;

    // 2. Per-request project context (WorkspaceManager + subject).
    let proj_ctx = app.build_project_context().await?;

    // 3. Resolve the scan directory.
    let materialised = match scan_dir(&proj_ctx.workspace_manager().config_manager).await {
        Ok(scan) => Some(scan),
        Err(e) => {
            tracing::warn!(
                project_id = %project_id,
                error = %e,
                "semantic-analysis: no scan directory available"
            );
            None
        }
    };

    let scan = match materialised {
        Some(m) => ScanHandle::Materialised(m),
        None => {
            // Stateless-fleet guard: no working copy, so the FS fallback
            // points at a non-existent dir. Return the NeedsRecompile contract
            // and enqueue a lazy compile instead of parsing an empty layer.
            //
            // Asks the manager, not `role == Serve` — a Worker is equally
            // diskless and fell straight through.
            if !proj_ctx.workspace_manager().config_manager.can_read_disk() {
                if let Ok(db) = oxy::database::client::establish_connection().await {
                    crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(
                        &db, project_id,
                    )
                    .await;
                }
                let mut response = err_with_code(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "workspace {project_id} has no compiled semantic model available on \
                         this stateless replica; a (re)compile has been enqueued — retry shortly"
                    ),
                    "semantic_needs_recompile",
                );
                if let Ok(val) = axum::http::HeaderValue::from_str(&project_id.to_string()) {
                    response.headers_mut().insert("x-oxy-needs-recompile", val);
                }
                return Err(response);
            }
            ScanHandle::WorkingCopy(
                proj_ctx
                    .workspace_manager()
                    .config_manager
                    .semantics_scan_path()
                    .to_path_buf(),
            )
        }
    };

    Ok(SemanticBoundary {
        app,
        proj_ctx,
        scan,
    })
}

/// Parse the airlayer semantic model from a scan directory, off the async
/// runtime (it's blocking CPU work that walks every `.view.yml`/`.topic.yml`).
pub(crate) async fn load_layer(
    scan_path: PathBuf,
) -> Result<oxy_airlayer_compat::SemanticLayer, Response> {
    // Sentry hubs are per thread: carry the request's onto the blocking pool so
    // a panic while parsing the model is captured under the custom-app surface
    // tag (`middlewares::sentry_surface`), not on the pool thread's bare hub.
    let hub = sentry::Hub::current();
    match tokio::task::spawn_blocking(move || {
        sentry::Hub::run(hub, || oxy_airlayer_compat::load_layer_from_dir(&scan_path))
    })
    .await
    {
        Ok(Ok(layer)) => Ok(layer),
        Ok(Err(e)) => Err(err_with_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load semantic model: {e}"),
            "semantic_layer_load_failed",
        )),
        Err(e) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("semantic model load task panicked: {e}"),
        )),
    }
}

/// Parse the `?refresh` flag from the raw query string (present, or
/// `refresh=…`) — the cache-bypass convention shared with `semantic_query.rs`.
pub(crate) fn wants_refresh(query: Option<&str>) -> bool {
    query
        .map(|q| {
            q.split('&')
                .any(|kv| kv == "refresh" || kv.starts_with("refresh="))
        })
        .unwrap_or(false)
}

/// Return a cached JSON body for `(project_id, ns, key)` unless `refresh`.
/// Project-scoped (`project_id` first) per the customer-apps-perf contract —
/// the multi-tenant isolation boundary.
pub(crate) fn cache_lookup(
    project_id: Uuid,
    ns: &'static str,
    key: &str,
    refresh: bool,
) -> Option<Response> {
    if refresh {
        return None;
    }
    super::result_cache::get(project_id, ns, "", key).map(|arc| {
        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            (*arc).clone(),
        )
            .into_response()
    })
}

/// Serialize `value`, store it under `(project_id, ns, key)`, and respond
/// with the JSON body. Only successful responses reach here — errors are
/// never cached.
pub(crate) fn cache_store<T: Serialize>(
    project_id: Uuid,
    ns: &'static str,
    key: &str,
    value: &T,
) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            let arc = std::sync::Arc::new(bytes);
            super::result_cache::put(project_id, ns, "", key, arc.clone());
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                (*arc).clone(),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("semantic-analysis serialize failed: {e}");
            Json(value).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wants_refresh;

    #[test]
    fn refresh_absent_is_false() {
        assert!(!wants_refresh(None));
        assert!(!wants_refresh(Some("")));
        assert!(!wants_refresh(Some("root=orders.net_revenue&limit=50")));
    }

    #[test]
    fn refresh_bare_flag_is_true() {
        assert!(wants_refresh(Some("refresh")));
        assert!(wants_refresh(Some("limit=50&refresh")));
    }

    #[test]
    fn refresh_with_value_is_true() {
        assert!(wants_refresh(Some("refresh=1")));
        assert!(wants_refresh(Some("root=x&refresh=true")));
    }

    #[test]
    fn refresh_substring_is_not_matched() {
        // A param whose name merely contains "refresh" must not trip the flag.
        assert!(!wants_refresh(Some("auto_refresh=1")));
        assert!(!wants_refresh(Some("refreshed=1")));
    }
}
