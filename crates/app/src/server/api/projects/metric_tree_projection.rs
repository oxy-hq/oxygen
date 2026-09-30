//! `/api/projects/{project_id}/semantic/metric-tree/projection` — the
//! scenario canvas's time axis, for customer-app bundles.
//!
//! Sibling of [`super::metric_tree`], which carries every other metric-tree op
//! on this surface; separated only because that file is already at its size
//! budget. Same posture as the ops there: enter through the customer-app gate,
//! load the layer from the compile boundary, cache on the request body.
//!
//! The analysis itself is NOT reimplemented here — validation, seasonality
//! resolution, the bucketed query and the fit are
//! [`crate::server::api::metric_tree_projection::run_projection`], shared
//! verbatim with the IDE handler, so a curve an SDK bundle draws is the curve
//! the Metric Tree canvas draws.
//!
//! **FleetOk**, like `baseline` and the rest of this data plane — pinned by
//! `the_customer_app_data_plane_is_fleet_ok` in `role_manifest_tests.rs`. The
//! scan resolves through the compile boundary first and the working copy
//! second, and warehouse execution needs only config + secrets, so any
//! replica answers it (the same argument `router/workspace.rs` makes for the
//! IDE twin). That is also why nothing below may read the working copy
//! directly: the one thing that did — the bucketing timezone — now comes
//! from the compiled `.monitor.yml` inside `run_projection`.

use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use entity::workspace_members::WorkspaceRole;
use uuid::Uuid;

use crate::server::api::custom_apps_gates::parse_versioned_body;
use crate::server::api::metric_tree::MetricTreeError;
use crate::server::api::metric_tree_projection::{
    ProjectionExec, ProjectionOutcome, ProjectionRequest, run_projection,
};
use crate::server::api::projects::semantic_boundary::{
    cache_lookup, cache_store, enter_semantic_boundary, err_with_code, load_layer, wants_refresh,
};

/// Map the shared projection core's error onto this surface's coded envelope.
///
/// The workspace twin answers `MetricTreeError` through its own
/// `IntoResponse`; bundles read `{ message, code }`, so the variants are
/// re-stated here rather than borrowed. `NotFound` keeps the same code
/// `baseline` uses for an unknown lever — it is the same typo with the same
/// fix, and an SDK switching on the code should not have to learn a second
/// spelling of it.
fn projection_error(err: MetricTreeError) -> Response {
    match err {
        MetricTreeError::NotFound(m) => {
            err_with_code(StatusCode::NOT_FOUND, m, "measure_not_found")
        }
        MetricTreeError::BadRequest(m) => {
            err_with_code(StatusCode::BAD_REQUEST, m, "projection_bad_request")
        }
        // This surface resolves its scan through `enter_semantic_boundary`,
        // which answers an uncompiled workspace itself — so reaching here means
        // the shared core refused after that. Answer it with the boundary's own
        // retryable code rather than a 500: it is the same condition with the
        // same fix, and the SDK already retries on that code.
        MetricTreeError::ScanUnavailable(m) => {
            tracing::warn!("metric-tree projection scan unavailable: {m}");
            let mut response = err_with_code(
                StatusCode::SERVICE_UNAVAILABLE,
                m,
                "semantic_needs_recompile",
            );
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("5"),
            );
            response
        }
        MetricTreeError::LayerLoad(e) => {
            tracing::error!("metric-tree projection layer load failed: {e}");
            err_with_code(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load semantic layer",
                "layer_load_failed",
            )
        }
        // Carries the timeout's "shorten the period or coarsen the grain"
        // advice, which is the caller's to act on — this surface passes its
        // op-failure text through for the same reason `explain` does.
        MetricTreeError::Op(e) => {
            tracing::error!("metric-tree projection failed: {e}");
            err_with_code(StatusCode::INTERNAL_SERVER_ERROR, e, "projection_failed")
        }
    }
}

/// `POST .../metric-tree/projection` — bucketed history plus the forward
/// curve, for the levers and everything downstream of them.
///
/// The third leg of scenario forecasting, after `baseline` (levels) and
/// `predict` (pure propagation): this is the only one that draws time. It
/// returns the BASELINE curve only — composing the scenario's second curve is
/// arithmetic over this and a `predict` result, and belongs on the client so
/// a lever edit doesn't cost a warehouse query per keystroke.
pub async fn post_projection(
    Path(project_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Gate before cache read so a cached hit can't bypass authorization.
    let boundary = match enter_semantic_boundary(&headers, project_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let key = String::from_utf8_lossy(&body).into_owned();
    if let Some(hit) = cache_lookup(&boundary, "mt-projection", &key, wants_refresh(uri.query())) {
        return hit;
    }
    let req: ProjectionRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let layer = match load_layer(boundary.scan.path_buf()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let exec = ProjectionExec {
        // The executor needs config + secrets, never the disk; downgrading
        // here says so rather than handing over a working copy it must not use.
        workspace_manager: boundary
            .proj_ctx
            .workspace_manager()
            .clone()
            .into_read_only(),
        user_id: boundary.app.user.id,
        // Customer-app requests carry no workspace role of their own to read
        // through; Viewer matches `runner_for`'s read-only posture here.
        role: WorkspaceRole::Viewer,
        // No pre-aggregation cache on this surface: the refresh-key cache is
        // attached by the workspace middleware, which bundles never pass
        // through. Rollups are still consulted, just without the shared
        // freshness memo.
        preagg_cache: None,
        preagg_renewal_threshold_secs: 120,
    };
    match run_projection(exec, layer, req).await {
        // A warehouse that was down when this ran is not a fact about the
        // workspace, and caching it would make `?refresh` the only way back —
        // same rule `baseline` follows. The refusals still ship.
        Ok(ProjectionOutcome {
            response,
            query_failed: true,
        }) => {
            tracing::warn!(
                project_id = %boundary.project_id(),
                "metric-tree projection query partially failed; responding without caching"
            );
            axum::Json(response).into_response()
        }
        Ok(ProjectionOutcome { response, .. }) => {
            cache_store(&boundary, "mt-projection", &key, &response)
        }
        Err(e) => projection_error(e),
    }
}
