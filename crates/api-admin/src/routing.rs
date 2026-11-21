//! `/admin/routing-manifest` — returns the IdeOnly classification table plus
//! the current process's role. Auth-gated by the same Owner / Global Admin
//! guard as the rest of `/admin/*` so the table isn't world-readable.
//!
//! Out of `oxy-app`'s `api::admin::routing` (admin split, 2026-11-21): an
//! `AdminSection` behind `PlatformOperate` ([`crate::admin_sections`]), where
//! it was the head of `admin::router`'s `staff_surface` chain. The table it
//! dumps is still `oxy-app`'s `role_manifest`, read through the prelude.

use axum::{Json, Router, routing::get};
use oxy_app::surface::roles::{current_process_role, dump_manifest};
use oxy_app_core::AppState;

/// Relative to `/admin`: `oxy-app`'s console merges it behind the staff
/// door, the refusal while acting and `PlatformOperate`.
pub fn router() -> Router<AppState> {
    Router::new().route("/routing-manifest", get(routing_manifest))
}

async fn routing_manifest() -> Json<serde_json::Value> {
    let role = current_process_role();
    let entries: Vec<serde_json::Value> = dump_manifest()
        .into_iter()
        .map(|(method, path, role_kind)| {
            serde_json::json!({
                "method": method,
                "path": path,
                "role": role_kind,
            })
        })
        .collect();
    Json(serde_json::json!({
        "process_role": role.as_str(),
        "ide_only": entries,
    }))
}
