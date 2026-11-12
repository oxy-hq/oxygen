//! The Slack integration's HTTP surface and bot: the org's install and
//! uninstall, Slack's webhooks (events and interactivity), the OAuth callback,
//! the account-link landing page, Socket Mode, and everything a Slack message
//! runs through on its way to an agent and back. Moved out of `oxy-app`'s
//! `integrations::slack` (design: internal-docs/2026-04-21-universal-slack-bot-design.md).
//!
//! Mounted by the `oxy-server` composition root through two seams, because the
//! surface sits on both sides of the auth stack:
//!
//! - [`public_routes`] rides `SurfaceSeams::public`, with **no** auth layer.
//!   Slack's webhooks carry no user credential (each is signature-verified in
//!   its handler), the OAuth callback is a bare browser redirect from
//!   slack.com, and the link landing page handles a signed-out visitor itself
//!   (`OptionalAuthenticatedUser`).
//! - [`routes`] rides `SurfaceSeams::api`, inside the auth stack, and re-applies
//!   the org tree's `org_middleware` + subscription guard, as the two routes had
//!   inside `oxy-app`'s `build_org_routes`. The admin check is the `OrgAdmin`
//!   extractor on the handlers.
//!
//! Every route reads and writes Postgres (and calls Slack) and nothing else, so
//! all are FleetOk, declared in [`route_roles`] and [`public_route_roles`].
//! [`boot::start_background`] is called by `oxy serve`.
//!
//! What other contexts need from Slack — the integration's config, the org's
//! installation, a person's link — is `oxy-slack`, below this crate.

// A Slack run awaits an agentic run, SQL and chart rendering through
// `OxyProjectContext`, and laying that future out passes rustc's default query
// depth in the optimized build (`oxy-server`'s `recursion_limit_guard`).
#![recursion_limit = "256"]

pub mod blocks;
pub mod boot;
pub mod chart_render;
/// `config`, the installation and user-link services and `linking::by_email`
/// went down to `oxy-slack`, which `oxy-api-notifications` and the anomaly
/// notifier read too. Re-exported at their old paths.
pub use oxy_slack::config;
/// `client` was extracted to the `oxy-slack-client` crate (shared with
/// `oxy-cameras`). Re-exported here so every in-crate `client::` path is
/// unchanged. See internal-docs/domain-boundaries.md.
pub use oxy_slack_client as client;
pub mod error;
pub mod events;
pub mod home;
pub mod linking;
pub mod oauth;
pub mod pickers;
pub mod render;
pub mod resolution;
pub mod scopes;
pub mod services;
pub mod signature;
pub mod socket_mode;
pub mod types;
pub mod webhooks;
pub mod workspace;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use oxy_app::surface::{org_middleware, subscription_guard_middleware};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

const FLEET: RouteRole = RouteRole::FleetOk;

/// The unauthenticated half: Slack's webhooks, the OAuth callback and the
/// account-link landing page.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/slack/oauth/callback", get(oauth::callback::callback))
        .route("/slack/events", post(webhooks::events::handle_events_route))
        .route(
            "/slack/interactivity",
            post(webhooks::interactivity::handle_interactivity),
        )
        .route("/slack/link", get(linking::landing::landing))
        .route("/slack/link/confirm", post(linking::landing::confirm))
}

/// The org half, at `/orgs/{org_id}/slack/*`: start an install, read and
/// remove the org's installation.
///
/// `org_middleware` must run BEFORE `subscription_guard` (the guard and
/// `OrgAdmin` read the `OrgContext` it inserts). axum applies the
/// last-declared `.layer` as outermost, so request order is
/// `org_middleware → subscription_guard → handler`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{org_id}/slack/install",
            post(oauth::install::start_install),
        )
        .route(
            "/orgs/{org_id}/slack/installation",
            get(oauth::status::get_status).delete(oauth::disconnect::disconnect),
        )
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

/// The roles of [`public_routes`]: Postgres and Slack's API only.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/slack/oauth/callback",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/slack/events",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/slack/interactivity",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/slack/link",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/slack/link/confirm",
            role: FLEET,
        },
    ]
}

/// The roles of [`routes`], for the same reason.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/slack/install",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/slack/installation",
            role: FLEET,
        },
    ]
}
