//! Personal access tokens (API-tokens design §8 Phase 2). Cloud-only, beside
//! the other global routes: local mode has no accounts to own a token.
//!
//! Every route here is `route_fleet`: they read and write Postgres only
//! (`api_tokens`, `api_token_grants`, `api_keys`, `audit_events`,
//! `api_token_usage_daily`) — no working copy, no `.git`, no state dir — so
//! any replica may answer, and managing a credential never depends on the one
//! instance that can be down.
//!
//! The workspace inventory (`GET /{workspace_id}/api-tokens`) is mounted with
//! the workspace tree, next to the legacy `/api-keys` routes.
//!
//! `oxyc login` is two routes on two sides of the auth gate: the browser asks
//! for a code under its session ([`build_token_routes`]), and the CLI, which
//! has no credential yet, redeems it ([`build_public_token_routes`]).

use axum::routing::{delete, get, patch, post};

use crate::server::api::org_api_access::handlers as org_handlers;
use crate::server::api::user_tokens::{cli_login, handlers, introspect, leak, options};

use super::AppState;
use super::role_router::RoleRouter;

pub(super) fn build_token_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        // Session-only: a token cannot mint, widen, extend or revoke a token.
        .route_fleet(
            "/user/tokens",
            get(handlers::list_tokens).post(handlers::create_token),
        )
        .route_fleet(
            "/user/tokens/{id}",
            get(handlers::get_token)
                .patch(handlers::update_token)
                .delete(handlers::revoke_token),
        )
        .route_fleet("/user/tokens/{id}/extend", post(handlers::extend_token))
        .route_fleet(
            "/user/tokens/{id}/regenerate",
            post(handlers::regenerate_token),
        )
        .route_fleet(
            "/user/tokens/{id}/activity",
            get(handlers::get_token_activity),
        )
        .route_fleet("/user/token-options", get(options::get_token_options))
        // `oxyc login`, step one. Session-only like the rest: it is the first
        // half of minting a token.
        .route_fleet("/auth/cli/authorize", post(cli_login::authorize))
        // Any credential: the calling token, about itself. A session gets 404.
        .route_fleet(
            "/auth/token",
            get(introspect::get_calling_token).delete(introspect::revoke_calling_token),
        )
}

/// Organization → API access (design §8 Phase 3): the org's service accounts,
/// their `oxy_sat_` tokens, and the inventory of every token that reaches the
/// org. Paths are relative to `/orgs/{org_id}`, where `org_middleware` has
/// already checked the caller's standing in the org.
///
/// `route_fleet`, all of them: every handler reads and writes Postgres only
/// (`service_accounts`, `users`, `api_tokens`, `api_token_grants`,
/// `org_token_policies`, `audit_events`, `api_token_usage_daily`) — no working copy, no `.git`, no
/// state dir. Revoking a credential must never wait on the one instance that
/// can be down.
pub(super) fn build_org_api_access_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet(
            "/service-accounts",
            get(org_handlers::list_service_accounts).post(org_handlers::create_service_account),
        )
        .route_fleet(
            "/service-accounts/{sa_id}",
            get(org_handlers::get_service_account)
                .patch(org_handlers::update_service_account)
                .delete(org_handlers::delete_service_account),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/tokens",
            get(org_handlers::list_account_tokens).post(org_handlers::create_account_token),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/tokens/{id}",
            delete(org_handlers::revoke_account_token),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/tokens/{id}/extend",
            post(org_handlers::extend_account_token),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/tokens/{id}/regenerate",
            post(org_handlers::regenerate_account_token),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/tokens/{id}/activity",
            get(org_handlers::get_account_token_activity),
        )
        // Trust policies: which GitHub Actions runs may act as the account.
        // Postgres only (and, at registration, one call to GitHub's API), so
        // any replica answers.
        .route_fleet(
            "/service-accounts/{sa_id}/trust-policies",
            get(org_handlers::list_trust_policies).post(org_handlers::create_trust_policy),
        )
        .route_fleet(
            "/service-accounts/{sa_id}/trust-policies/{id}",
            patch(org_handlers::update_trust_policy).delete(org_handlers::delete_trust_policy),
        )
        .route_fleet("/tokens", get(org_handlers::list_org_tokens))
        .route_fleet(
            "/tokens/{id}/activity",
            get(org_handlers::get_org_token_activity),
        )
        .route_fleet(
            "/tokens/{id}/revoke-grant",
            post(org_handlers::revoke_org_grant),
        )
        // The org's token policy: one Postgres row, read on every replica.
        .route_fleet(
            "/token-policy",
            get(org_handlers::get_token_policy).put(org_handlers::put_token_policy),
        )
}

/// The half of `oxyc login` that runs before there is a credential: the CLI
/// redeems its one-time code. Mounted with the public tree, beside the other
/// `/auth/*` handshakes. Postgres only (`cli_auth_codes`, `api_tokens`,
/// `audit_events`), so any replica answers.
pub(super) fn build_public_token_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet("/auth/cli/exchange", post(cli_login::exchange))
        // Trusted access: a GitHub Actions run trades its OIDC token for a
        // 15-minute `oxy_ci_` token. Public by construction — the JWT is the
        // credential, and the claims are what gate it.
        .route_fleet(
            "/auth/oidc/exchange",
            post(crate::server::api::oidc_exchange::exchange),
        )
        // Leak response: a secret scanner reports exposed tokens and each
        // new-format one is revoked. Public by construction — the report is
        // the credential — and rate-limited per client. Postgres (and the
        // owner's mail) only, so any replica answers.
        .route_fleet("/auth/tokens/revoke-leaked", post(leak::revoke_leaked))
}
