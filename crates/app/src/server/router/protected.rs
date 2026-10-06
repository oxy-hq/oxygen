//! Composes protected (auth-gated) routes for cloud and local modes.
//!
//! Cloud mounts [`build_global_routes`] alongside the workspace tree and
//! applies the standard auth middleware. Local mode omits global routes
//! and swaps in a guest-only auth stack plus the local workspace context.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};

use agentic_http::AgenticState;
use oxy_auth::middleware::{AuthState, api_key_only_middleware, auth_middleware};
use oxy_shared::errors::OxyError;

use crate::api::middlewares::api_key_query::api_key_query_middleware;
use crate::api::middlewares::app_grant_scope::app_grant_scope_middleware;
use crate::api::middlewares::app_publish_token_scope::app_publish_token_scope_middleware;
use crate::api::middlewares::local_context::local_context_middleware;
use crate::api::middlewares::subscription_guard::workspace_subscription_guard_middleware;
use crate::api::middlewares::timeout::timeout_middleware;
use crate::api::middlewares::token_grant_scope::token_grant_scope_middleware;
use crate::api::middlewares::token_usage::token_usage_middleware;
use crate::api::middlewares::workspace_context::{
    workspace_access_middleware, workspace_middleware,
};
use oxy_app_core::serve_mode::ServeMode;

use super::AppState;
use super::global::build_global_routes;
use super::role_router::{Decl, RoleRouter};
use super::workspace::{
    build_external_workspace_routes, build_workspace_preview_routes, build_workspace_routes,
};
use crate::server::route_catalog::RouteCatalog;

pub(super) fn build_protected_routes(
    app_state: AppState,
    agentic_state: Arc<AgenticState>,
    // Workspace-scoped surface crates (composed by the `oxy-server` root) merge
    // HERE — before the workspace middleware — so they inherit the workspace
    // subscription guard + `workspace_middleware` rather than reproducing them.
    // Empty in local mode.
    //
    // They carry their own role declarations because a merge has no prefix to
    // hang one on, and unlike the Postgres-only surfaces this seam also carries
    // `oxy-api-onboarding`, which clones and scaffolds a checkout on disk. Left
    // undeclared it would fall to the FleetOk default and let a stateless
    // replica clone into a workspace it does not own.
    extra_workspace_routes: Router<AppState>,
    extra_workspace_decls: Vec<oxy_shared::fleet_role::RouteRoleDecl>,
    catalog: RouteCatalog,
    // Staff-console sections from extracted surfaces (`SurfaceSeams::admin`);
    // they live in the global tree, so only cloud mode has them.
    admin_sections: Vec<super::AdminSection>,
) -> (Router<AppState>, Vec<Decl>) {
    let root = RoleRouter::new(app_state.clone())
        .merge(build_catalog_routes(&app_state, catalog))
        .merge(build_global_routes(&app_state, admin_sections));
    // Beside the workspace tree, not in it: see `build_workspace_preview_routes`.
    let previews = build_workspace_preview_routes(&app_state)
        .map_router(|r| r.layer(middleware::from_fn(workspace_access_middleware)));
    let workspace = build_workspace_routes(app_state.clone(), agentic_state, true, false)
        .merge_declared(extra_workspace_routes, &extra_workspace_decls)
        .map_router(|r| {
            r.layer(middleware::from_fn(workspace_subscription_guard_middleware))
                .layer(middleware::from_fn_with_state(
                    app_state,
                    workspace_middleware,
                ))
        });

    let (router, decls, _) = root
        .nest("/{workspace_id}", workspace)
        .nest("/{workspace_id}/previews", previews)
        .into_parts();
    let mut decls = crate::server::role_manifest::api_prefixed(decls);
    // `/customer-apps/{*path}` is mounted on the OUTER router in serve.rs, not
    // here, but this is the declaration set that gets installed — so the split
    // that module declares rides along rather than living in a table.
    decls.extend(
        crate::server::api::custom_apps_serve::serve_dispatch_roles()
            .iter()
            .map(|d| (d.method, d.path.to_string(), d.role)),
    );
    (router, decls)
}

/// Route discovery for the `oxyc` CLI: `GET /api/_catalog`.
///
/// ITS OWN BUILDER, and that is not stylistic. `crates/route-catalog/build_route_catalog.rs`
/// walks a fixed list of SEED functions to generate the route table, and
/// `build_protected_routes` is not one of them (it composes the global and
/// workspace trees, which are seeded individually — seeding it too would
/// double-count every route). A route written inline there is therefore
/// invisible to the very catalog it serves, so `/api/_catalog` would be the one
/// endpoint missing from the endpoint list. A named builder can be seeded on
/// its own, and `oxy-route-catalog`'s `the_catalog_lists_itself` fails if it
/// ever stops being.
///
/// `catalog` is the generated table, handed down from `oxy-server` through
/// [`SurfaceSeams`](super::SurfaceSeams) and bound into the handler here.
///
/// Merged into BOTH the cloud and local protected routers, because local mode
/// omits the global tree.
///
/// `route_fleet` and not `route_ide`: the handler reads a table compiled into
/// the binary — no workspace working copy, no `.git`, no state dir — so any
/// replica may answer it, and pinning it to the singleton would make the CLI's
/// discovery depend on the one instance that can be down.
pub(super) fn build_catalog_routes(app_state: &AppState, catalog: RouteCatalog) -> RoleRouter {
    RoleRouter::new(app_state.clone()).route_fleet(
        "/_catalog",
        axum::routing::get(move |query| crate::api::catalog::get_catalog(catalog, query)),
    )
}

/// Takes the declarations as well as the routes, and installs them, because a
/// served router that never installed them classifies every workspace route
/// FleetOk — a replica would answer git and file requests locally, off a working
/// copy it does not have. Dropping the call used to break nothing that any test
/// could see; asking for the value here makes forgetting it a compile error.
pub(super) fn apply_middleware(
    protected_routes: Router<AppState>,
    declarations: Vec<Decl>,
) -> Result<Router<AppState>, OxyError> {
    crate::server::role_manifest::install_declarations(declarations);
    Ok(api_auth_layers(protected_routes))
}

/// The `/api` authentication stack, exactly as served. Public so `oxy-server`'s
/// token regression tests (`tests/integration/token_auth`) send requests through
/// these same layers rather than a copy of them.
pub fn api_auth_layers<S: Clone + Send + Sync + 'static>(routes: Router<S>) -> Router<S> {
    routes
        // Innermost: runs AFTER auth has attached identity + any admin-token
        // marker, so it can confine admin-token requests to the customer-apps
        // admin surface before they reach a handler. No-op for cookie/JWT/
        // API-key sessions.
        .layer(middleware::from_fn(app_publish_token_scope_middleware))
        // Beside it, the allow-list for a sandbox agent token: the sandbox loop
        // and nothing else, 404 otherwise. This is what makes `Admit` below
        // safe, so the two are never separated. No-op for any other credential.
        .layer(middleware::from_fn(app_grant_scope_middleware))
        // Beside it, and for the same reason: after auth, before any handler.
        // A grant-bound API token is refused (404) on the flat routes that
        // answer from raw membership; an all-access token an org has blocked
        // keeps the ones that leave that org out. No-op for sessions, legacy
        // keys and all-access tokens no org has blocked.
        .layer(middleware::from_fn(token_grant_scope_middleware))
        .layer(middleware::from_fn(timeout_middleware))
        // Inside auth (it reads the credential auth attached), outside the
        // timeout (so a timed-out request is counted with its real status).
        .layer(middleware::from_fn(token_usage_middleware))
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(oxy_auth::token::SandboxAgent::Admit),
            auth_middleware,
        ))
        // Run BEFORE the auth gate so EventSource (SSE) can authenticate
        // via `?api_key=` query param — browsers can't attach headers to
        // EventSource requests. axum applies `.layer` from the outside in,
        // so this declaration places the query-param promoter outermost,
        // which is what we want.
        .layer(middleware::from_fn(api_key_query_middleware))
}

/// The `/external/api` authentication stack, exactly as served (CORS aside).
/// Order mirrors [`api_auth_layers`]: `api_key_query` is OUTERMOST (runs
/// first) so EventSource's `?api_key=` is promoted to the `X-API-Key` header
/// before the API-key-only gate reads it.
pub fn external_auth_layers<S: Clone + Send + Sync + 'static>(routes: Router<S>) -> Router<S> {
    routes
        .layer(middleware::from_fn(timeout_middleware))
        .layer(middleware::from_fn(token_usage_middleware))
        .layer(middleware::from_fn(api_key_only_middleware))
        .layer(middleware::from_fn(api_key_query_middleware))
}

/// Local-mode protected routes: mount the same `build_workspace_routes` content
/// surface under `/{workspace_id}` (mirroring the cloud router's URL shape, so
/// existing `Path<WorkspacePath>` extractors still work). The URL segment in
/// local mode is always `LOCAL_WORKSPACE_ID` (nil UUID) — clients hardcode it.
///
/// `build_global_routes` (org + workspace CRUD) is intentionally omitted.
/// However the per-user Airhouse routes ARE mounted here too: they're per-user
/// + per-org and don't depend on workspace context. Local mode seeds a
/// nil-UUID org with the local guest user as Owner, so the existing
/// per-org provision flow works untouched.
pub(super) fn build_local_protected_routes(
    app_state: AppState,
    agentic_state: Arc<AgenticState>,
    // Local mode reaches these too — their handlers carry live `is_local()`
    // branches — so merge before the local-context layer, exactly as cloud does.
    extra_workspace_routes: Router<AppState>,
    extra_workspace_decls: Vec<oxy_shared::fleet_role::RouteRoleDecl>,
    catalog: RouteCatalog,
) -> (Router<AppState>, Vec<Decl>) {
    // Same discovery surface as cloud. Local mode omits the global tree, so it
    // is merged separately rather than inherited — a CLI that can enumerate
    // production but not a developer's own box is the wrong way round.
    let root = RoleRouter::new(app_state.clone()).merge(build_catalog_routes(&app_state, catalog));
    let workspace = build_workspace_routes(app_state.clone(), agentic_state, false, true)
        .merge_declared(extra_workspace_routes, &extra_workspace_decls)
        .map_router(|r| {
            r.route_layer(middleware::from_fn_with_state(
                app_state,
                local_context_middleware,
            ))
        });

    let (router, decls, _) = root
        .merge_undeclared(
            airhouse::api::router::<AppState>(),
            "airhouse provisioning is per-user and per-org, never per-workspace",
        )
        .merge_undeclared(
            oxy_oltp::api::router::<AppState>(),
            "per-org OLTP status is Postgres-only; no workspace files on any path",
        )
        .nest("/{workspace_id}", workspace)
        .into_parts();
    // Same as the cloud arm: absolute paths, because `install_declarations` no
    // longer prefixes.
    (router, crate::server::role_manifest::api_prefixed(decls))
}

/// See [`apply_middleware`] — same reason, local mode.
pub(super) fn apply_local_middleware(
    protected_routes: Router<AppState>,
    declarations: Vec<Decl>,
) -> Result<Router<AppState>, OxyError> {
    crate::server::role_manifest::install_declarations(declarations);
    Ok(protected_routes
        .route_layer(middleware::from_fn(timeout_middleware))
        .route_layer(middleware::from_fn_with_state(
            AuthState::guest_only(),
            auth_middleware,
        ))
        .route_layer(middleware::from_fn(api_key_query_middleware)))
}

/// Loud JSON 404 for the external API surface. Without an explicit fallback,
/// an unmatched `/external/api/*` path lets its default 404 propagate up to
/// serve.rs's `.fallback_service(main)` and gets the SPA HTML back — which is
/// exactly why a mis-wired external route (the clip-playback gap) surfaced as
/// `undefined` JSON fields in the client instead of a clear 404. An explicit
/// fallback intercepts before the SPA. Auth runs as an outer layer, so an
/// unauthenticated miss still returns 401, not this.
async fn external_api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "not_found",
            "message": "no such external API route",
        })),
    )
        .into_response()
}

/// Build the EXTERNAL API surface: the curated workspace routes
/// (`build_external_workspace_routes`) under `/{workspace_id}`, gated by
/// API-key-ONLY auth and served with wide-open CORS.
///
/// This is a fully self-contained `Router` (state applied) intended to be
/// mounted at the top level (`/external/api`) *outside* the global
/// `build_cors_layer`, so it carries only its own permissive
/// `build_external_cors_layer`. It reuses the same workspace-resolution
/// middleware as the main surface (so the `{workspace_id}` context + handlers
/// behave identically) but swaps the cookie-accepting `auth_middleware` for
/// `api_key_only_middleware` — that swap is what makes `*`-origin CORS safe
/// (no ambient cookie credential ⇒ no CSRF).
pub(super) fn build_external_api_router(
    app_state: AppState,
    agentic_state: Arc<AgenticState>,
    mode: ServeMode,
) -> Router {
    let curated = build_external_workspace_routes(&app_state, agentic_state).into_router();

    // Resolve the `{workspace_id}` context exactly as the main surface does,
    // per mode. Runs AFTER auth (it needs the authenticated user).
    let with_context = match mode {
        ServeMode::Cloud => curated
            .layer(middleware::from_fn(workspace_subscription_guard_middleware))
            .layer(middleware::from_fn_with_state(
                app_state.clone(),
                workspace_middleware,
            )),
        ServeMode::Local => curated.route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            local_context_middleware,
        )),
    };

    let routes = Router::new()
        .nest("/{workspace_id}", with_context)
        // Explicit 404 so an unmatched external path returns JSON, not the SPA
        // HTML it would otherwise fall through to (serve.rs `.fallback_service`).
        // A nested miss propagates its default 404 up to this fallback; auth
        // (below) still runs first, so unauth misses stay 401.
        .fallback(external_api_not_found);
    external_auth_layers(routes)
        .layer(super::build_external_cors_layer())
        .with_state(app_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::role_manifest::{RouteRole, declared_role};

    /// No two declarations may tie on specificity while disagreeing on pod.
    ///
    /// When they do, `declared_role_in` keeps whichever was mounted first, so
    /// reordering a `.route_*` call silently reclassifies a route — an IdeOnly
    /// route becoming FleetOk is a replica answering a request off a working
    /// copy it does not have, with nothing red to show for it.
    ///
    /// This is the check that found `/secrets/env` vs `/secrets/{id}` and
    /// `/tests/project-runs` vs `/tests/{pathb64}`. Both were classified
    /// correctly, but only because the literal pattern happened to be mounted
    /// first.
    #[test]
    fn no_two_declarations_tie_on_specificity_with_different_pods() {
        use crate::server::role_manifest::{pattern_matches, specificity};

        let app_state = super::super::bare_app_state();
        let (_routes, decls) = build_protected_routes(
            app_state,
            super::super::test_agentic_state(),
            Router::new(),
            Vec::new(),
            Default::default(),
            Vec::new(),
        );

        let mut ambiguous = Vec::new();
        for (i, (m1, p1, r1)) in decls.iter().enumerate() {
            for (m2, p2, r2) in decls.iter().skip(i + 1) {
                if r1 == r2 || specificity(m1, p1) != specificity(m2, p2) {
                    continue;
                }
                // Different concrete methods can never collide on one request.
                if m1 != m2 && *m1 != "*" && *m2 != "*" {
                    continue;
                }
                // Probe from BOTH sides. The pair loop visits each pair once
                // with `p1` the earlier declaration, so probing `p1` alone
                // missed every case where `p2`'s own path is what the other
                // pattern also claims.
                //
                // Each probe is a pattern's OWN path with params filled by a
                // token no literal equals — i.e. a request some route is really
                // mounted at. Merging the two patterns' literals instead
                // (taking `orgs` from one and `git-state` from the other) build
                // paths nothing serves: it flagged eleven pairs like
                // `/api/orgs/{org_id}` vs `/api/{workspace_id}/git-state`,
                // which collide only if an org id is literally "git-state".
                let collides = [skeleton(p1), skeleton(p2)]
                    .iter()
                    .any(|probe| pattern_matches(p1, probe) && pattern_matches(p2, probe));
                if collides {
                    ambiguous.push(format!("{m1} {p1} {r1:?} vs {m2} {p2} {r2:?}"));
                }
            }
        }

        assert!(
            ambiguous.is_empty(),
            "these declarations tie on specificity but disagree on pod, so mount \
             order decides which one wins:\n  {}",
            ambiguous.join("\n  ")
        );
    }

    /// A pattern's own path, with params filled by a token no literal equals.
    ///
    /// The point is a path something is really mounted at, so a collision found
    /// here is one a real request can reach.
    ///
    /// The alternative — merging the two patterns' literals segment-wise, one
    /// probe per pair — was tried and rejected: it proposes paths nothing
    /// serves. Taking `orgs` from `/api/orgs/{org_id}` and `git-state` from
    /// `/api/{workspace_id}/git-state` builds `/api/orgs/git-state`, which
    /// collides only if an org id is literally "git-state". It flagged eleven
    /// such pairs.
    fn skeleton(pattern: &str) -> String {
        pattern
            .split('/')
            .map(|seg| {
                if seg.starts_with('{') {
                    // Not a UUID: the nil UUID is `LOCAL_WORKSPACE_ID`
                    // elsewhere in this crate, and a token that means nothing
                    // reads better in the failure message.
                    "__param__"
                } else {
                    seg
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The SERVER path must install the declarations. `classify` reads what the
    /// router declared; with nothing installed every workspace route classifies
    /// FleetOk and a replica answers git and file requests locally, off a
    /// working copy it does not have.
    ///
    /// `apply_middleware` takes them by value so they cannot be dropped on the
    /// way through. This covers the other half — that it installs them — by
    /// going through `apply_middleware` itself rather than calling the
    /// installer, which is what a test of this can get wrong.
    #[test]
    fn apply_middleware_installs_the_route_roles() {
        let app_state = super::super::bare_app_state();
        let (routes, decls) = build_protected_routes(
            app_state,
            super::super::test_agentic_state(),
            Router::new(),
            Vec::new(),
            Default::default(),
            Vec::new(),
        );
        assert!(decls.len() > 150, "only {} declarations", decls.len());

        let _installed = apply_middleware(routes, decls).expect("middleware applies");

        let git = "/api/d9830be4-c6a4-4f89-11d3-9a0c0305e82c/pull-changes";
        assert_eq!(
            declared_role("POST", git),
            Some(RouteRole::IdeOnly),
            "the registry must answer for a git route — apply_middleware did not install"
        );
    }
}
