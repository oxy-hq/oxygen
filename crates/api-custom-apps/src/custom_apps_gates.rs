//! The data-plane gate chain, answered as a response: `oxy_custom_apps::gate`
//! holds the steps and the refusals; this maps a refusal onto the JSON envelope
//! the SDK reads (`oxy_app_core::gate_response`).
//!
//! It is also where a gated request meets the workspace runtime. The runtime
//! takes the gate's answer as plain fields (`GatedCaller`), so it does not
//! depend on `oxy-custom-apps`; the gate's context builds them in one place
//! (`CustomAppContext::gated`), [`CustomAppContextExt`] builds the project
//! context from them, and [`enter_semantic_boundary`] runs the gate and
//! then the runtime's semantic boundary. Up from `oxy-workspace-runtime`'s
//! `custom_apps_gates` in depth stage C (A2, 2026-11-27).
//!
//! Auth still runs BEFORE body parse: every handler takes its body as raw
//! bytes and parses it only after the gate passes, so an unauthenticated 422
//! cannot leak that a route exists or what it expects.

use axum::http::HeaderMap;
use axum::response::Response;
use oxy_custom_apps::gate::GateError;
use oxy_workspace_runtime::custom_apps_gates::GatedCallerExt;
use oxy_workspace_runtime::project_ctx::OxyProjectContext;
use oxy_workspace_runtime::semantic_boundary::{SemanticBoundary, open_semantic_boundary};
use uuid::Uuid;

pub use oxy_custom_apps::gate::{CustomAppContext, frontline_worker_with_app_grant};

/// The response for a refusal or failure the decision half reports.
fn gate_error_response(e: &GateError) -> Response {
    e.render(oxy_app_core::gate_response::gate_refusal)
}

/// `oxy_custom_apps::gate::check_custom_app_gates`, with a refusal turned
/// into its response. Returns the resolved context on success; handlers
/// pattern-match the `Err` arm and early-return.
pub async fn check_custom_app_gates(
    headers: &HeaderMap,
    project_id: Uuid,
) -> Result<CustomAppContext, Response> {
    oxy_custom_apps::gate::check_custom_app_gates(headers, project_id)
        .await
        .map_err(|e| gate_error_response(&e))
}

/// What [`CustomAppContext`] needs from the workspace runtime: the project
/// context built from the gate's answer (`CustomAppContext::gated`, the one
/// place that answer is built). It is a trait because the type lives below
/// `OxyProjectContext`; import it to call `ctx.build_project_context()`.
pub trait CustomAppContextExt {
    /// Build the per-request `OxyProjectContext` derived from the
    /// resolved workspace (`GatedCallerExt::build_project_context`).
    fn build_project_context(
        &self,
    ) -> impl std::future::Future<Output = Result<OxyProjectContext, Response>> + Send;
}

impl CustomAppContextExt for CustomAppContext {
    async fn build_project_context(&self) -> Result<OxyProjectContext, Response> {
        self.gated().build_project_context().await
    }
}

/// Run the customer-app gate chain, then the workspace runtime's semantic
/// boundary (`open_semantic_boundary`: the project context and the scan) —
/// the prelude the metric-tree twins enter by. The gate runs first, so a
/// request that fails it never reaches the boundary or its body parse.
pub async fn enter_semantic_boundary(
    headers: &HeaderMap,
    project_id: Uuid,
) -> Result<SemanticBoundary, Response> {
    // 1. Shared gates (auth → origin → workspace → org membership).
    let app = check_custom_app_gates(headers, project_id).await?;
    // 2. The project context and the scan, at the gate's caller and pin.
    open_semantic_boundary(app.gated()).await
}

#[cfg(test)]
#[path = "custom_apps_gates_tests.rs"]
mod tests;
