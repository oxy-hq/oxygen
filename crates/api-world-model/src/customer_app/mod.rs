//! The world model's customer-app twins: what a custom-app bundle reads
//! through the SDK (`useWorldModel`, `useWorldModelInstances`,
//! `useMeasureBreakdown`, peer cohorts) at
//! `/api/projects/{project_id}/semantic/{world-model*,cohort}`.
//!
//! The IDE serves the same model on the workspace tree (`crate::graph`,
//! `crate::cohort`), and both answer from `oxy_world_model::{graph, cohort}`.
//! What differs is how a caller gets in and what it may see. A bundle passes
//! the customer-app gate, inline (session cookie or API key), so these routes
//! ride `SurfaceSeams::public` outside the auth stack, as they did in
//! `oxy-app`'s public tree. Every read is scoped to the viewer's reach. The
//! gate, the project context, the compile-boundary scan, the project-scoped
//! result cache and the error envelope are the data plane's
//! (`oxy_workspace_runtime::{custom_apps_gates, semantic_boundary}`), shared with the metric-tree
//! twins in `oxy-api-custom-apps`. The gate itself runs here, in
//! [`enter_semantic_boundary`], since the workspace runtime takes only its
//! answer (depth stage C, A2).

use axum::http::HeaderMap;
use axum::response::Response;
use oxy_app_core::operating_graph::reach::Reach;
use oxy_custom_apps::gate::CustomAppContext;
use oxy_workspace_runtime::semantic_boundary::{SemanticBoundary, open_semantic_boundary};
use uuid::Uuid;

pub mod cohort;
pub mod world_model;

/// The response for a refusal or failure the gate's decision half reports:
/// the same `oxy_app_core::gate_response::gate_refusal` envelope
/// `oxy-api-custom-apps` answers with (`a_gate_error_becomes_the_response_it_always_was`
/// pins both).
fn gate_error_response(e: &oxy_custom_apps::gate::GateError) -> Response {
    e.render(oxy_app_core::gate_response::gate_refusal)
}

/// Run the customer-app gate chain, then the workspace runtime's semantic
/// boundary (the project context and the scan) at the gate's caller and pin.
/// The gate runs first, so a request that fails it never reaches the boundary
/// or its body parse. Returns the gate's own context beside the boundary: the
/// twins read the viewer, the connection and the org from it.
///
/// `oxy-api-custom-apps` runs the same chain for the metric-tree twins
/// (`custom_apps_gates::enter_semantic_boundary`); a surface may not import
/// another, so each states the two steps, and both answer a refusal with
/// `oxy_app_core::gate_response::gate_refusal`.
pub async fn enter_semantic_boundary(
    headers: &HeaderMap,
    project_id: Uuid,
) -> Result<(CustomAppContext, SemanticBoundary), Response> {
    // 1. Shared gates (auth → origin → workspace → org membership).
    let app = oxy_custom_apps::gate::check_custom_app_gates(
        headers,
        project_id,
        oxy_server_authz::enforcer(),
    )
    .await
    .map_err(|e| gate_error_response(&e))?;
    // 2. The project context and the scan, at the gate's caller and pin.
    //    `gated()` is the one place the gate's answer is built (it carries the
    //    staging pin that picks the result-cache partition).
    let boundary = open_semantic_boundary(app.gated()).await?;
    Ok((app, boundary))
}

/// `oxy_custom_apps::reach::reach_for_viewer` for the gate's viewer: the
/// caller of `ctx.user`, WITH the credential the request arrived with, as the
/// domain built it before it took a caller (`Caller::from_user`).
pub async fn viewer_reach(ctx: &CustomAppContext, project_id: Uuid, app_id: Option<Uuid>) -> Reach {
    let viewer = oxy_server_authz::Caller::from_user(&ctx.user);
    oxy_custom_apps::reach::reach_for_viewer(
        &ctx.db,
        ctx.org_id,
        viewer.as_contract(),
        project_id,
        app_id,
    )
    .await
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
