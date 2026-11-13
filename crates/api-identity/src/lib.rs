//! Identity tokens over HTTP: personal access tokens, legacy API keys, an
//! org's API access, `oxyc login`, the GitHub Actions OIDC exchange and leak
//! response (API-tokens design; sandbox agent credential design).
//!
//! - [`user_tokens`]: the caller's tokens, `oxyc login` (code and device), the
//!   calling token about itself, the token's browser sign-in, leak response,
//!   sandbox agent and agent tokens, the workspace inventory, the hygiene the
//!   sweep runs;
//! - [`api_keys`]: the legacy workspace `/api-keys` endpoint;
//! - [`org_api_access`]: an org's service accounts, their tokens and trust
//!   policies, the inventory of every token reaching the org, its token policy;
//! - [`oidc_exchange`]: a GitHub Actions run trading its OIDC token for an
//!   `oxy_ci_` token;
//! - [`admin`]: the two staff-console sections;
//! - [`token_sweep`]: the periodic token hygiene the global driver runs.
//!
//! The lifecycle audit these write is `oxy-identity`'s
//! (`oxy_identity::token_lifecycle`), below the surfaces, because the
//! custom-app sandbox sweep writes it too.
//!
//! Mounted by the `oxy-server` composition root through six seams, each the
//! place its routes had in `oxy-app`'s router, with the same middleware:
//!
//! - [`routes`] rides `SurfaceSeams::api`, merged into the protected tree
//!   before `apply_middleware`, so it takes the standard `/api` auth stack;
//!   cloud only, as `build_global_routes` was. Its org half re-applies the org
//!   tree's `org_middleware` + subscription guard, the only two layers
//!   `build_org_routes`' `gated` tree put between those routes and the auth
//!   stack.
//! - [`public_routes`] rides `SurfaceSeams::public`, beside oxy-app's
//!   `build_public_routes`, outside the auth stack: the CLI's half of
//!   `oxyc login`, the browser's ticket redeem, the OIDC exchange and leak
//!   reports hold no credential yet. That tree added no layer of its own.
//! - [`workspace_routes`] rides `SurfaceSeams::workspace`, merged inside the
//!   `/{workspace_id}` nest before the workspace middleware and subscription
//!   guard, which it inherits.
//! - [`admin_sections`] ride `SurfaceSeams::admin`: `oxy-app` applies each
//!   one's capability, the staff guard and the acting block, as it did for the
//!   in-tree sections.
//! - [`openapi`] rides `SurfaceSeams::openapi`: the legacy key operations.
//! - [`global_ticks`] rides `SurfaceSeams::global_ticks`: the token sweep.
//!
//! Every route reads and writes Postgres only (and, for a trust policy or an
//! OIDC exchange, GitHub's API) — no working copy, no `.git`, no state dir — so
//! all are FleetOk, declared explicitly in [`route_roles`],
//! [`public_route_roles`] and [`workspace_route_roles`] because the type gate
//! `route_fleet` gave them does not reach across the crate line. Managing or
//! revoking a credential must never wait on the one instance that can be down.

// The token handlers' futures were laid out under oxy-app's raised limit; the
// `--release` build that produces the image takes more query depth than a
// debug one, and PR CI never runs it. Raised here for the same reason.
#![recursion_limit = "256"]

pub mod admin;
pub mod api_keys;
mod authz;
pub mod oidc_exchange;
pub mod org_api_access;
pub mod public_rate_limit;
pub mod token_sweep;
pub mod user_tokens;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, patch, post};
use oxy_app::surface::seams::GlobalTickSeam;
use oxy_app::surface::{Action, AdminSection, org_middleware, subscription_guard_middleware};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use org_api_access::handlers as org_handlers;
use user_tokens::{
    browser_session, cli_login, device_login, handlers, introspect, inventory, leak, options,
};

/// The authenticated half, at the `/api` root: the caller's tokens, the
/// session's halves of `oxyc login`, the calling token, and an org's API
/// access under `/orgs/{org_id}`.
pub fn routes() -> Router<AppState> {
    token_routes().merge(org_routes())
}

/// Personal access tokens and the calling-token pair. Session-only, every
/// management route: a token cannot mint, widen, extend or revoke a token.
pub fn token_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/user/tokens",
            get(handlers::list_tokens).post(handlers::create_token),
        )
        .route(
            "/user/tokens/{id}",
            get(handlers::get_token)
                .patch(handlers::update_token)
                .delete(handlers::revoke_token),
        )
        .route("/user/tokens/{id}/extend", post(handlers::extend_token))
        .route(
            "/user/tokens/{id}/regenerate",
            post(handlers::regenerate_token),
        )
        .route(
            "/user/tokens/{id}/activity",
            get(handlers::get_token_activity),
        )
        .route("/user/token-options", get(options::get_token_options))
        // `oxyc login`, step one. Session-only like the rest: it is the first
        // half of minting a token.
        .route("/auth/cli/authorize", post(cli_login::authorize))
        // `oxyc … --no-browser`: the page reads a device-code request and
        // decides it, under the session.
        .route(
            "/auth/cli/device/requests/{user_code}",
            get(device_login::describe),
        )
        .route(
            "/auth/cli/device/requests/{user_code}/approve",
            post(device_login::approve),
        )
        .route(
            "/auth/cli/device/requests/{user_code}/deny",
            post(device_login::deny),
        )
        // Any credential: the calling token, about itself. A session gets 404.
        .route(
            "/auth/token",
            get(introspect::get_calling_token).delete(introspect::revoke_calling_token),
        )
        // A personal token asks for the one-time ticket that signs a browser
        // in as it. A session gets 404: there is no token to sign in as.
        .route("/auth/browser-ticket", post(browser_session::issue_ticket))
}

/// Organization → API access (design §8 Phase 3), at `/orgs/{org_id}/*`.
///
/// `org_middleware` must run BEFORE `subscription_guard` (the guard reads the
/// `OrgContext` the middleware inserts, and `OrgAdminFor` needs it too). axum
/// applies the last-declared `.layer` as outermost, so this yields request
/// order `org_middleware → subscription_guard → handler`: the order these
/// routes had inside oxy-app's `build_org_routes`.
pub fn org_routes() -> Router<AppState> {
    Router::new()
        .nest("/orgs/{org_id}", org_api_access_routes())
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

/// The org's service accounts, their tokens and trust policies, the tokens
/// that reach the org, and its token policy. Relative to `/orgs/{org_id}`.
fn org_api_access_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/service-accounts",
            get(org_handlers::list_service_accounts).post(org_handlers::create_service_account),
        )
        .route(
            "/service-accounts/{sa_id}",
            get(org_handlers::get_service_account)
                .patch(org_handlers::update_service_account)
                .delete(org_handlers::delete_service_account),
        )
        .route(
            "/service-accounts/{sa_id}/tokens",
            get(org_handlers::list_account_tokens).post(org_handlers::create_account_token),
        )
        .route(
            "/service-accounts/{sa_id}/tokens/{id}",
            delete(org_handlers::revoke_account_token),
        )
        .route(
            "/service-accounts/{sa_id}/tokens/{id}/extend",
            post(org_handlers::extend_account_token),
        )
        .route(
            "/service-accounts/{sa_id}/tokens/{id}/regenerate",
            post(org_handlers::regenerate_account_token),
        )
        .route(
            "/service-accounts/{sa_id}/tokens/{id}/activity",
            get(org_handlers::get_account_token_activity),
        )
        // Trust policies: which GitHub Actions runs may act as the account.
        // Postgres only (and, at registration, one call to GitHub's API).
        .route(
            "/service-accounts/{sa_id}/trust-policies",
            get(org_handlers::list_trust_policies).post(org_handlers::create_trust_policy),
        )
        .route(
            "/service-accounts/{sa_id}/trust-policies/{id}",
            patch(org_handlers::update_trust_policy).delete(org_handlers::delete_trust_policy),
        )
        .route("/tokens", get(org_handlers::list_org_tokens))
        .route(
            "/tokens/{id}/activity",
            get(org_handlers::get_org_token_activity),
        )
        .route(
            "/tokens/{id}/revoke-grant",
            post(org_handlers::revoke_org_grant),
        )
        // The org's token policy: one Postgres row, read on every replica.
        .route(
            "/token-policy",
            get(org_handlers::get_token_policy).put(org_handlers::put_token_policy),
        )
}

/// The unauthenticated half, beside the other `/auth/*` handshakes: each holds
/// no credential yet, or carries its own (a one-time code, a ticket, a JWT, a
/// leak report).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        // `oxyc login`'s CLI half: redeem the one-time code.
        .route("/auth/cli/exchange", post(cli_login::exchange))
        // The device-code flow's CLI half: start a request, then poll it.
        // Rate-limited per client.
        .route("/auth/cli/device", post(device_login::start))
        .route("/auth/cli/device/poll", post(device_login::poll))
        // The browser's half of a token's sign-in link: the ticket it redeems
        // is the only thing that names one.
        .route(
            "/auth/browser-ticket/redeem",
            post(browser_session::redeem_ticket),
        )
        // Trusted access: a GitHub Actions run trades its OIDC token for a
        // 15-minute `oxy_ci_` token. The JWT is the credential, and the claims
        // are what gate it.
        .route("/auth/oidc/exchange", post(oidc_exchange::exchange))
        // Leak response: a secret scanner reports exposed tokens and each
        // new-format one is revoked. The report is the credential; rate-limited
        // per client.
        .route("/auth/tokens/revoke-leaked", post(leak::revoke_leaked))
}

/// Inside `/{workspace_id}`: the legacy API keys and the tokens that can reach
/// the workspace.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .nest("/api-keys", api_key_routes())
        // Read-only, Postgres only (api_tokens, api_token_grants, org_members,
        // users). The tokens that can reach this workspace, for its admins.
        .route("/api-tokens", get(inventory::list_workspace_tokens))
}

fn api_key_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(api_keys::list_api_keys).post(api_keys::create_api_key),
        )
        .route(
            "/{id}",
            get(api_keys::get_api_key).delete(api_keys::delete_api_key),
        )
        .route("/{id}/extend", post(api_keys::extend_api_key))
        .route("/{id}/activity", get(api_keys::get_api_key_activity))
}

/// The staff-console sections, each behind the capability it had in-tree.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        // Every staff member's sandbox agent tokens: the cross-admin view,
        // which is what `operate_platform` buys. An App Operator may mint one
        // and does not hold this; they manage their own on `/user/tokens`.
        AdminSection {
            capability: Action::PlatformOperate,
            routes: admin::sandbox_agent_tokens::router(),
            decls: Vec::new(),
        },
        // Every personal token that carries a standing, across owners: the
        // grant table's capability governs the grant in credential form.
        AdminSection {
            capability: Action::PlatformGrants,
            routes: admin::standing_tokens::router(),
            decls: Vec::new(),
        },
    ]
}

/// Both sections, relative to `/admin`, for the route catalog's seed only:
/// [`admin_sections`] mounts them one capability each.
pub fn admin_routes() -> Router<AppState> {
    admin::sandbox_agent_tokens::router().merge(admin::standing_tokens::router())
}

/// The legacy key operations, merged into the served OpenAPI document
/// (`oxyc schema` reads it).
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(api_keys::create_api_key))
        .routes(utoipa_axum::routes!(api_keys::list_api_keys))
        .routes(utoipa_axum::routes!(api_keys::get_api_key))
        .routes(utoipa_axum::routes!(api_keys::delete_api_key))
        .routes(utoipa_axum::routes!(api_keys::extend::extend_api_key))
        .into_openapi()
}

/// Token hygiene on the global driver's pass: spent OIDC jtis, long-expired
/// `ci` tokens, expiry notices, unused tokens expired. Throttled per process
/// and idempotent, so every driver running it is harmless.
pub fn global_ticks() -> Vec<GlobalTickSeam> {
    vec![GlobalTickSeam {
        name: "token_sweep",
        tick: |db, _now| {
            Box::pin(async move {
                token_sweep::sweep(db).await;
                0
            })
        },
    }]
}

const FLEET: RouteRole = RouteRole::FleetOk;

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: FLEET,
    }
}

/// Every route [`routes`] mounts, and which pod may serve it. Relative to the
/// `/api` root.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    API_ROLES
}

const API_ROLES: &[RouteRoleDecl] = &[
    fleet("/user/tokens"),
    fleet("/user/tokens/{id}"),
    fleet("/user/tokens/{id}/extend"),
    fleet("/user/tokens/{id}/regenerate"),
    fleet("/user/tokens/{id}/activity"),
    fleet("/user/token-options"),
    fleet("/auth/cli/authorize"),
    fleet("/auth/cli/device/requests/{user_code}"),
    fleet("/auth/cli/device/requests/{user_code}/approve"),
    fleet("/auth/cli/device/requests/{user_code}/deny"),
    fleet("/auth/token"),
    fleet("/auth/browser-ticket"),
    fleet("/orgs/{org_id}/service-accounts"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/tokens"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/tokens/{id}"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/tokens/{id}/extend"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/tokens/{id}/regenerate"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/tokens/{id}/activity"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/trust-policies"),
    fleet("/orgs/{org_id}/service-accounts/{sa_id}/trust-policies/{id}"),
    fleet("/orgs/{org_id}/tokens"),
    fleet("/orgs/{org_id}/tokens/{id}/activity"),
    fleet("/orgs/{org_id}/tokens/{id}/revoke-grant"),
    fleet("/orgs/{org_id}/token-policy"),
];

/// Every route [`public_routes`] mounts. Relative to the `/api` root.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    PUBLIC_ROLES
}

const PUBLIC_ROLES: &[RouteRoleDecl] = &[
    fleet("/auth/cli/exchange"),
    fleet("/auth/cli/device"),
    fleet("/auth/cli/device/poll"),
    fleet("/auth/browser-ticket/redeem"),
    fleet("/auth/oidc/exchange"),
    fleet("/auth/tokens/revoke-leaked"),
];

/// Every route [`workspace_routes`] mounts. Relative to `/{workspace_id}`.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROLES
}

const WORKSPACE_ROLES: &[RouteRoleDecl] = &[
    fleet("/api-keys"),
    fleet("/api-keys/{id}"),
    fleet("/api-keys/{id}/extend"),
    fleet("/api-keys/{id}/activity"),
    fleet("/api-tokens"),
];

#[cfg(test)]
mod tests;
