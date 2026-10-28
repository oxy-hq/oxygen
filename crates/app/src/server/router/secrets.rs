//! Secret routes. Each handler enforces `WorkspaceAdmin` via its extractor
//! signature, so no route-level middleware is needed.

use super::role_router::RoleRouter;
use axum::routing::{get, post};

use crate::api::secrets;

use super::AppState;

pub(super) fn build_secret_routes(app_state: AppState) -> RoleRouter {
    RoleRouter::new(app_state)
        .route_fleet("/", get(secrets::list_secrets).post(secrets::create_secret))
        .route_fleet("/bulk", post(secrets::bulk_create_secrets))
        .route_fleet(
            "/{id}",
            get(secrets::get_secret)
                .put(secrets::update_secret)
                .delete(secrets::delete_secret),
        )
        .route_fleet("/{id}/value", get(secrets::reveal_secret))
}
