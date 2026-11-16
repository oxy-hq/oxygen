//! Cloud-only global routes: organization CRUD (with org-scoped sub-routes
//! for members, invitations, onboarding, workspaces, GitHub integration) and
//! the per-user GitHub account/installation routes. Logout is
//! `oxy-api-identity`'s, through `SurfaceSeams::api`.
//!
//! Not mounted in local mode — see [`super::protected`].

use axum::Router;
use axum::middleware;
use axum::routing::{delete, get, patch};

use crate::api::billing;
use crate::api::middlewares::{
    app_scope_guard, org_context, oxy_owner_or_app_admin_guard, platform_cap_guard,
    subscription_guard,
};
use crate::api::{admin, workspaces};

use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use super::role_router::RoleRouter;
use super::{AdminSection, AppState};

pub(super) fn build_global_routes(app_state: &AppState, admin: Vec<AdminSection>) -> RoleRouter {
    // The extracted sections' declarations join the console's own, under the
    // same `/admin` prefix; collected before `admin::router` takes the sections.
    let admin_decls: Vec<RouteRoleDecl> = admin::router_roles()
        .iter()
        .copied()
        .chain(
            admin
                .iter()
                .flat_map(|section| section.decls.iter().copied()),
        )
        .collect();
    RoleRouter::new(app_state.clone())
        // `/logout` and the mobile app's sign-in approval
        // (`/auth/mobile/authorize`) are `oxy-api-identity`'s, merged beside
        // this tree through `SurfaceSeams::api` (2026-11-15), as are the
        // personal-token routes.
        // Chat (`/chat/channels*`, the SSE stream included, every route
        // FleetOk) is `oxy-api-threads`' since 2026-11-16, merged at this same
        // root through `SurfaceSeams::api`; its delivery listener starts
        // through `SurfaceSeams::background`.
        // The assignment graph (`/work`, `/work/{id}`) is `oxy-api-operating-graph`'s
        // (2026-11-14), merged at this same root through `SurfaceSeams::api`.
        // Still NOT under `/orgs/{org_id}`: a frontline worker holds no org
        // membership, and `org_middleware` would reject them.
        // Documents — read side — moved to the `oxy-api-documents` sibling
        // crate, merged at this same root by `oxy-server` through the
        // `extra_api_routes` seam. Still NOT under `/orgs/{org_id}`: frontline
        // workers hold no org membership, and `org_middleware` would reject them.
        // Notifications moved to the `oxy-api-notifications` sibling crate,
        // merged at this same root by `oxy-server` through the api seam. Still
        // no org gate: self-scoped, and a frontline worker holds no org
        // membership by design.
        .merge_undeclared(
            airhouse::api::router::<AppState>(),
            "airhouse provisioning is per-user and per-org, never per-workspace",
        )
        // Per-org OLTP status. Belongs here, not only in the local-mode router:
        // cloud reaches these through `build_global_routes`, and mounting it
        // solely in `build_local_protected_routes` left
        // `/api/oltp/me/connection` unregistered under `serve --enterprise` —
        // the SPA catch-all then answered it with index.html and HTTP 200, so
        // the settings panel read a missing route as "not provisioned".
        .merge_undeclared(
            oxy_oltp::api::router::<AppState>(),
            "per-org OLTP status is Postgres-only; no workspace files on any path",
        )
        .nest("/orgs/{org_id}", build_org_routes(app_state))
        // Assume-role ("act as"). Authenticated, NOT admin-gated: staff act as any
        // org, a partner acts as an assigned client with `develop_apps`. The
        // handlers authorize (`assume::may_act_as`). It must sit outside /admin
        // because acting CLOSES /admin — the exit cannot live behind the door it
        // locks.
        .merge_undeclared(
            admin::assume::router(),
            "assume-role sessions are Postgres rows; the exit cannot live behind the door it locks",
        )
        // `/admin/*` runs under the permissive owner-or-app-admin guard so
        // app admins can reach feature flags, custom apps, orgs / users /
        // workspaces management, and internal jobs. The sensitive subset —
        // billing operations and the `app_admins` table itself — escalates
        // to strict OXY_OWNER via `route_layer` inside `admin::router()`.
        //
        // Declared rather than `nest_all`: the console is FleetOk wholesale
        // except creating an org, which scaffolds its Default workspace onto
        // node-local disk. `admin::router_roles` states both.
        .nest_declared(
            "/admin",
            admin::router(admin).layer(middleware::from_fn(
                oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware,
            )),
            &admin_decls,
        )
        // Internal Jobs is mounted as a sibling nest because its routes
        // were flattened (no `/internal-jobs/` prefix on each route). The
        // outer guard mirrors the broader `/admin/*` permissive guard.
        .nest_all(
            "/admin/internal-jobs",
            RouteRole::FleetOk,
            admin::internal_jobs::router()
                // Operating the durable task fleet is Oxy's own machinery, not tenant
                // data — `Cap::OperatePlatform`. A sibling nest gets no capability gate
                // from `admin::router`, so it names its own, exactly as it must name
                // its own `block_admin_while_acting`.
                .layer(middleware::from_fn(platform_cap_guard::require(
                    crate::server::authz::Action::PlatformOperate,
                )))
                // Refuse the staff surface while acting, exactly like `admin::router`
                // does internally. This is a SIBLING nest — it never passes through
                // `admin::router`, so the block it applies does not reach here. Any
                // future `/admin/*` sibling needs this line too, or it silently
                // becomes drivable mid-impersonation.
                .layer(middleware::from_fn(admin::assume::block_admin_while_acting))
                .layer(middleware::from_fn(
                    oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware,
                )),
            "job admin reads the task queue tables",
        )
        // Compile boundary operator surface (Phase 1.6a+). List recent
        // revisions, drill into one, manually enqueue a Compile task.
        // Same guard layering as internal-jobs.
        .nest_all(
            "/admin/compiles",
            RouteRole::FleetOk,
            admin::compiles::router()
                // Compile history is platform machinery — `Cap::OperatePlatform`.
                .layer(middleware::from_fn(platform_cap_guard::require(
                    crate::server::authz::Action::PlatformOperate,
                )))
                // Sibling nest — see internal-jobs above.
                .layer(middleware::from_fn(admin::assume::block_admin_while_acting))
                .layer(middleware::from_fn(
                    oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware,
                )),
            "the compile operator surface reads the revisions tables",
        )
    // NOTE: Slack webhook + OAuth-callback + magic-link routes are NOT
    // registered here. They ride `SurfaceSeams::public` (`oxy-api-slack`),
    // outside the auth middleware layer this file sits inside, because:
    //   - Slack's webhook POSTs (/slack/events, /slack/interactivity) have
    //     no user auth — they're signature-verified inside the handler.
    //   - The OAuth callback is reached via a browser redirect from
    //     slack.com — the browser carries no Authorization header.
    //   - The magic-link landing handles auth state itself via
    //     OptionalAuthenticatedUser; bouncing through the auth middleware
    //     first would 401 every unauth'd user.
}

fn build_org_routes(app_state: &AppState) -> RoleRouter {
    // Two sub-routers under the same `org_middleware`:
    //   - `gated` covers everything that requires an active subscription
    //     (members, invitations, onboarding, workspace CRUD, github)
    //   - `bypass` covers `/billing/*`, the only org-scoped tree the user
    //     can hit while paywalled (so they can subscribe / open the portal)
    let gated = RoleRouter::new(app_state.clone())
        // The org-scoped frontline routes (`/frontline/workers*`,
        // `/frontline/devices*`, `/frontline/device/leave`) and every handler
        // behind them live in the `oxy-api-frontline` sibling crate, which
        // re-applies this tree's `org_middleware` + subscription guard and
        // declares them FleetOk.
        // The client's partner-publish consent switch
        // (`/partner-publish-consent`) is `oxy-api-tenancy`'s since 2026-11-16,
        // in its org tree with this tree's `org_middleware` + subscription
        // guard re-applied, declared FleetOk there.
        // Organization → API access (service accounts, their tokens, the
        // inventory of every token reaching the org, its token policy) is
        // `oxy-api-identity`'s, merged through `SurfaceSeams::api` with this
        // tree's `org_middleware` + subscription guard re-applied.
        // The operating graph — locations, their hierarchy and per-integration
        // ids, the positions vocabulary (`/roles`) and assignments — is
        // `oxy-api-operating-graph`'s (2026-11-14), merged through
        // `SurfaceSeams::api` with this tree's `org_middleware` + subscription
        // guard re-applied. `internal-docs/operating-graph.md`.
        // Documents — manage side — moved to `oxy-api-documents`, which nests
        // them at `/orgs/{org_id}` itself and re-applies `org_middleware` +
        // `subscription_guard`, the two layers this function wraps them in.
        // The three workspace-creating onboarding routes moved to the
        // `oxy-api-onboarding` sibling crate. They CREATE a checkout on disk
        // through raw `std::fs` and git rather than an extractor, so no type
        // gate can see them — and the crate sits outside this router, so
        // `route_ide` cannot reach them either. They are declared by hand at
        // the `nest_declared` seam that mounts the crate.
        .route_fleet("/workspaces", get(workspaces::list_workspaces))
        // Deletes the row, schedules and Airhouse tenant: Postgres only.
        .route_fleet("/workspaces/{id}", delete(workspaces::delete_workspace))
        .route_fleet(
            "/workspaces/{id}/rename",
            patch(workspaces::rename_workspace),
        )
        // Slack installation management (`/slack/install`, `/slack/installation`)
        // is `oxy-api-slack`'s, merged through `SurfaceSeams::api` with this
        // tree's `org_middleware` + subscription guard re-applied.
        .map_router(|r| {
            r.layer(middleware::from_fn(
                subscription_guard::subscription_guard_middleware,
            ))
        });

    // `/billing/*` is the only org-scoped tree a paywalled user may reach, so it
    // sits outside the subscription guard above. So does `/feature-flags`: the
    // shell reads it on every page, the paywall included. Postgres-backed cache,
    // FleetOk.
    gated
        .route_fleet(
            "/feature-flags",
            get(crate::server::feature_flags::routes::org_flags),
        )
        .nest_all(
            "/billing",
            RouteRole::FleetOk,
            billing::router(),
            "Stripe customer + subscription rows",
        )
        .map_router(|r| r.layer(middleware::from_fn(org_context::org_middleware)))
}
