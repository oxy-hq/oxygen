//! The tenant's billing routes and the Stripe webhook, out of `oxy-app`'s
//! `server::api::billing` in the admin split's PR 9. Mounted by this crate's
//! [`routes`](crate::routes) (the api seam, under `/orgs/{org_id}/billing`)
//! and [`public_routes`](crate::public_routes) (the public seam,
//! `POST /webhooks/stripe`).
//!
//! The service gate (`billing_service`, `billing_disabled`) lives in
//! `oxy_billing::gate` (decompose B1); this module takes it from
//! `oxy_billing` directly. The past-due dispatcher ([`notify`]) stayed in
//! `oxy-app` at PR 9 and followed its callers here in decompose B2.

pub mod handlers;
pub mod notify;
pub mod webhook;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_app_core::guards::org_context::org_middleware;

/// [`router`] at `/billing`, behind `org_middleware`, for
/// [`routes`](crate::routes) to nest under `/orgs/{org_id}`: the shape
/// `build_org_routes` gave it (`nest_all("/billing", ..)` inside the org
/// tree's `org_middleware`), minus the subscription guard, which billing never
/// sat behind. Here rather than in `lib.rs` so the route catalog resolves
/// `router` to this module's, not the Billing queue's `admin::billing::router`.
pub(crate) fn org_routes() -> Router<AppState> {
    Router::new()
        .nest("/billing", router())
        .layer(from_fn(org_middleware))
}

/// Routes nested under `/orgs/{org_id}/billing/*`. Webhook endpoint is
/// mounted via [`public_routes`](crate::public_routes); `/config/stripe` was
/// removed in the 2026-04-28 sales-gated redesign (no Stripe.js consumer).
///
/// `/` (full summary), `/portal-session`, and `/invoices` are admin-only.
/// `/status` returns the minimum needed to drive the FE paywall and is
/// readable by any org member.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::get_billing))
        .route("/status", get(handlers::get_billing_status))
        .route("/portal-session", post(handlers::post_portal))
        .route("/invoices", get(handlers::get_invoices))
        .route(
            "/checkout-sessions/{session_id}",
            get(handlers::get_checkout_session),
        )
}
