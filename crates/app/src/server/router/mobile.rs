//! The Oxygen mobile app's sign-in (`api::auth::mobile`), on two sides of the
//! auth gate: the browser approves a code under its session
//! ([`build_mobile_routes`]), and the app, which has no credential yet,
//! redeems it and renews the session it gets ([`build_public_mobile_routes`]).
//!
//! Every route is `route_fleet`: Postgres only (`cli_auth_codes`, `users`) —
//! no working copy, no `.git`, no state dir.
//!
//! The personal-token routes these sat beside (`oxyc login`, `/user/tokens`,
//! the calling token, the OIDC exchange, leak response) are
//! `oxy-api-identity`'s, mounted through `SurfaceSeams::api` and
//! `SurfaceSeams::public`.

use axum::routing::post;

use crate::server::api::auth::mobile;

use super::AppState;
use super::role_router::RoleRouter;

/// Step one: the same session-only approval `oxyc login` takes, for a code
/// bound to an `oxygen://` redirect URI. Merged into the global tree.
pub(super) fn build_mobile_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet("/auth/mobile/authorize", post(mobile::authorize))
}

/// The app's half, and renewing the session it gets. No credential yet, or
/// one only these check. Merged into the public tree.
pub(super) fn build_public_mobile_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet("/auth/mobile/exchange", post(mobile::exchange))
        .route_fleet("/auth/mobile/refresh", post(mobile::refresh))
}
