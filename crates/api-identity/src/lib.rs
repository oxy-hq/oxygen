//! Identity over HTTP: signing in, who the caller is, personal access tokens,
//! legacy API keys, an org's API access, `oxyc login`, the GitHub Actions OIDC
//! exchange and leak response (API-tokens design; sandbox agent credential
//! design).
//!
//! - [`sign_in`]: magic link, OAuth (Google / Okta / GitHub), the OAuth state,
//!   session hydration, the public auth config, `return_to` validation, the dev
//!   sign-in bypass and the mobile app's sign-in. The session they mint (the
//!   JWT, the `oxy_session` cookie, the login payload) is `oxy-app`'s, through
//!   `oxy_app::surface::session`, shared with frontline's PIN sign-in;
//! - [`user`]: `GET /user` and `/logout`;
//! - [`user_tokens`]: the caller's tokens, `oxyc login` (code and device), the
//!   calling token about itself, the token's browser sign-in, leak response,
//!   sandbox agent and agent tokens, the workspace inventory, the hygiene the
//!   sweep runs;
//! - [`org_api_access`]: an org's service accounts, their tokens and trust
//!   policies, the inventory of every token reaching the org, its token policy;
//! - [`oidc_exchange`]: a GitHub Actions run trading its OIDC token for an
//!   `oxy_ci_` token;
//! - [`admin`]: the staff-console sections (two token sections and the user
//!   directory);
//! - [`token_sweep`]: the periodic token hygiene the global driver runs.
//!
//! The lifecycle audit these write is `oxy-identity`'s
//! (`oxy_identity::token_lifecycle`), below the surfaces, because the
//! custom-app sandbox sweep writes it too.
//!
//! Mounted by the `oxy-server` composition root through six seams, each the
//! place its routes had in `oxy-app`'s router, with the same middleware:
//!
//! - [`routes`] (the tokens, `/logout`, the mobile sign-in approval and the
//!   org's API access) rides `SurfaceSeams::api`, merged into the protected tree
//!   before `apply_middleware`, so it takes the standard `/api` auth stack;
//!   cloud only, as `build_global_routes` was. Its org half re-applies the org
//!   tree's `org_middleware` + subscription guard, the only two layers
//!   `build_org_routes`' `gated` tree put between those routes and the auth
//!   stack.
//! - [`public_routes`] rides `SurfaceSeams::public`, beside oxy-app's
//!   `build_public_routes`, outside the auth stack: signing in, `GET /user`,
//!   the CLI's half of `oxyc login`, the browser's ticket redeem, the OIDC
//!   exchange and leak reports hold no credential yet (or check their own).
//!   That tree added no layer of its own.
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
mod authz;
pub mod oidc_exchange;
pub mod org_api_access;
pub mod public_rate_limit;
pub mod sign_in;
pub mod token_activity;
pub mod token_sweep;
pub mod user;
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
    token_routes().merge(session_routes()).merge(org_routes())
}

/// The session's own two routes, from `build_global_routes`: logging out, and
/// the mobile app's sign-in approval (the same session-only approval `oxyc
/// login` takes, for a code bound to an `oxygen://` redirect URI).
pub fn session_routes() -> Router<AppState> {
    Router::new()
        .route("/logout", get(user::logout))
        .route("/auth/mobile/authorize", post(sign_in::mobile::authorize))
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
        // What approving a service-account token would create, for the page.
        // Session-only, and writes nothing.
        .route(
            "/auth/cli/service-account/preview",
            post(cli_login::preview_service_account),
        )
        // The same for a trust policy `oxyc init-ci` asks for.
        .route(
            "/auth/cli/trust-policy/preview",
            post(cli_login::preview_trust_policy),
        )
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
    sign_in_public_routes().merge(token_public_routes())
}

/// Signing in, from `build_public_routes`: the providers, the magic link, the
/// session hydration and config the login page reads, the dev sign-in bypass,
/// the mobile app's redeem and refresh, and who the caller is.
///
/// The mobile app's half holds no credential yet, or one only it checks. The
/// dev sign-in bypass is public by necessity (it IS the login), but 404s
/// unless an allow-list resolves — `OXY_DEV_LOGIN_EMAILS` in any build, or the
/// staff roster on a debug build for a loopback caller — and only ever issues
/// a session for an address on it. The three rungs and why they differ:
/// `sign_in::dev_login`.
fn sign_in_public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/mobile/exchange", post(sign_in::mobile::exchange))
        .route("/auth/mobile/refresh", post(sign_in::mobile::refresh))
        .route("/auth/config", get(sign_in::get_config))
        .route("/auth/session", get(sign_in::get_session))
        .route("/auth/oauth/state", post(sign_in::issue_oauth_state))
        .route("/auth/google", post(sign_in::google_auth))
        .route("/auth/github", post(sign_in::github_auth))
        .route("/auth/okta", post(sign_in::okta_auth))
        .route(
            "/auth/magic-link/request",
            post(sign_in::request_magic_link),
        )
        .route("/auth/magic-link/verify", post(sign_in::verify_magic_link))
        .route("/auth/return-to/validate", get(sign_in::validate_return_to))
        .route(
            "/auth/dev-login",
            get(sign_in::dev_login_get).post(sign_in::dev_login),
        )
        .route("/user", get(user::get_current_user_public))
}

/// The personal-token handshakes.
fn token_public_routes() -> Router<AppState> {
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

/// Inside `/{workspace_id}`: the tokens that can reach the workspace.
///
/// `/api-keys` used to be nested here — mint, list, revoke, extend and the
/// activity of a legacy API key. The format is gone, so the routes are too: a
/// request to one now 404s, which is the honest answer for a surface that no
/// longer exists.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        // Read-only, Postgres only (api_tokens, api_token_grants, org_members,
        // users). The tokens that can reach this workspace, for its admins.
        .route("/api-tokens", get(inventory::list_workspace_tokens))
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
        // The cross-tenant user directory, behind the capability it had in
        // `oxy-app` before the admin split (PR 5) moved it here. A bounded
        // grant passes this gate; the handlers fence each person and org.
        AdminSection {
            capability: Action::PlatformUsers,
            routes: admin_users_routes(),
            decls: Vec::new(),
        },
    ]
}

/// The user directory (`/users*`), relative to `/admin`: its own section
/// (`PlatformUsers`), and its own function so the route catalog can seed it.
pub fn admin_users_routes() -> Router<AppState> {
    Router::new().merge(admin::users_admin::router())
}

/// The two token sections, relative to `/admin`, for the route catalog's
/// seed only: [`admin_sections`] mounts them one capability each. The user
/// directory has its own seed, [`admin_users_routes`].
pub fn admin_routes() -> Router<AppState> {
    admin::sandbox_agent_tokens::router().merge(admin::standing_tokens::router())
}

/// This crate's operations in the served OpenAPI document (`oxyc schema`
/// reads it). It held the legacy `/api-keys` operations until those routes
/// were removed.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(user::get_current_user_public))
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
    fleet("/auth/cli/service-account/preview"),
    fleet("/auth/cli/trust-policy/preview"),
    fleet("/auth/cli/device/requests/{user_code}"),
    fleet("/auth/cli/device/requests/{user_code}/approve"),
    fleet("/auth/cli/device/requests/{user_code}/deny"),
    fleet("/auth/token"),
    fleet("/auth/browser-ticket"),
    fleet("/logout"),
    fleet("/auth/mobile/authorize"),
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
    fleet("/auth/mobile/exchange"),
    fleet("/auth/mobile/refresh"),
    fleet("/auth/config"),
    fleet("/auth/session"),
    fleet("/auth/oauth/state"),
    fleet("/auth/google"),
    fleet("/auth/github"),
    fleet("/auth/okta"),
    fleet("/auth/magic-link/request"),
    fleet("/auth/magic-link/verify"),
    fleet("/auth/return-to/validate"),
    fleet("/auth/dev-login"),
    fleet("/user"),
];

/// Every route [`workspace_routes`] mounts. Relative to `/{workspace_id}`.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROLES
}

const WORKSPACE_ROLES: &[RouteRoleDecl] = &[fleet("/api-tokens")];

#[cfg(test)]
mod tests;
