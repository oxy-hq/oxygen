//! Cloud-only global routes: organization CRUD (with org-scoped sub-routes
//! for members, invitations, onboarding, workspaces, GitHub integration) and
//! the per-user GitHub account/installation routes. Logout is
//! `oxy-api-identity`'s, through `SurfaceSeams::api`.
//!
//! Not mounted in local mode — see [`super::protected`].

use axum::Router;
use axum::middleware;

use crate::admin;
use oxy_app_core::guards::{org_context, subscription_guard};
use oxy_server_authz::{oxy_owner_or_app_admin_guard, platform_cap_guard};

use oxy_shared::fleet_role::RouteRoleDecl;

use super::role_router::RoleRouter;
use super::{AdminNest, AdminSection, AppState};

pub(super) fn build_global_routes(
    app_state: &AppState,
    admin: Vec<AdminSection>,
    admin_nests: Vec<AdminNest>,
) -> RoleRouter {
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
    let tree = RoleRouter::new(app_state.clone())
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
        // Assume-role ("act as", `/assume*`) is `oxy-api-admin`'s since the admin
        // split (2026-11-20), merged at this same root through `SurfaceSeams::api`,
        // undeclared (FleetOk) as it was here. Authenticated, NOT admin-gated: staff
        // act as any org, a partner acts as an assigned client with `develop_apps`,
        // and the handlers authorize (`assume::may_act_as`, which stayed in
        // `oxy-app`). It must sit outside /admin because acting CLOSES /admin — the
        // exit cannot live behind the door it locks.
        // `/admin/*` runs under the permissive owner-or-app-admin guard so
        // app admins can reach feature flags, custom apps, orgs / users /
        // workspaces management, and internal jobs. The sensitive subset —
        // billing operations (`oxy-api-billing`'s section since the admin
        // split's PR 9) — escalates to strict OXY_OWNER via `route_layer`
        // inside `admin::router()`. The `app_admins` table
        // itself takes `PlatformGrants` plus a per-row bound instead
        // (`oxy-api-admin`'s section since 2026-11-20).
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
        );
    // The console's sibling trees (`SurfaceSeams::admin_nests`), each nested
    // at `/admin{prefix}` behind the three layers `staff_nest` applies, in the
    // order the composition hands them in: internal jobs (`oxy-api-admin`,
    // admin split PR 7), then the compile console (`oxy-api-previews`, PR 6).
    admin_nests.into_iter().fold(tree, |tree, nest| {
        assert!(
            nest.prefix.len() > 1 && nest.prefix.starts_with('/') && !nest.prefix.ends_with('/'),
            "an admin nest's prefix is `/name`, relative to `/admin`: got {:?}",
            nest.prefix
        );
        tree.nest_all(
            &format!("/admin{}", nest.prefix),
            nest.role,
            staff_nest(nest.routes, nest.capability),
            nest.why,
        )
    })
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

/// The three layers an `/admin` sibling nest ([`AdminNest`]) runs under, as
/// `.layer` rather than `route_layer`, so a mounted path asked with a method it
/// does not take answers them before its 405 (a path the tree does not match
/// at all is the outer router's 404, as it always was). Innermost first:
///
/// 1. the capability the nest names (`platform_cap_guard`). A sibling nest gets
///    no capability gate from `admin::router`, so it names its own;
/// 2. the refusal while acting as a tenant, exactly as `admin::router` applies
///    it internally. A SIBLING nest never passes through `admin::router`, so
///    the block that router applies does not reach it; without this line the
///    tree is silently drivable mid-impersonation;
/// 3. the staff door outermost, the same permissive guard the `/admin` nest
///    runs under.
fn staff_nest(routes: Router<AppState>, capability: oxy_server_authz::Action) -> Router<AppState> {
    routes
        .layer(middleware::from_fn(platform_cap_guard::require(capability)))
        .layer(middleware::from_fn(admin::assume::block_admin_while_acting))
        .layer(middleware::from_fn(
            oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware,
        ))
}

fn build_org_routes(app_state: &AppState) -> RoleRouter {
    // Two sub-routers under the same `org_middleware`:
    //   - `gated` covers everything that requires an active subscription
    //     (members, invitations, onboarding, workspace CRUD, github)
    //   - `bypass` covered `/billing/*`, the only org-scoped tree the user
    //     can hit while paywalled (so they can subscribe / open the portal).
    //     It is `oxy-api-billing`'s since the admin split's PR 9, merged
    //     through `SurfaceSeams::api` with `org_middleware` re-applied and no
    //     subscription guard, declared FleetOk there as `nest_all` declared it.
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
        // The org's workspace list, delete and rename (`/workspaces`,
        // `/workspaces/{id}`, `/workspaces/{id}/rename`) are
        // `oxy-api-workspaces`' since 2026-11-16, merged through
        // `SurfaceSeams::api` with this tree's `org_middleware` + subscription
        // guard re-applied, still FleetOk.
        // Slack installation management (`/slack/install`, `/slack/installation`)
        // is `oxy-api-slack`'s, merged through `SurfaceSeams::api` with this
        // tree's `org_middleware` + subscription guard re-applied.
        .map_router(|r| {
            r.layer(middleware::from_fn(
                subscription_guard::subscription_guard_middleware,
            ))
        });

    // `/billing/*` (`oxy-api-billing`'s since the admin split's PR 9, above) is
    // the only org-scoped tree a paywalled user may reach, so it sits outside
    // the subscription guard above. So does `/feature-flags`, `oxy-api-admin`'s
    // since 2026-11-21, merged through `SurfaceSeams::api` with `org_middleware`
    // re-applied and no subscription guard, declared FleetOk there as
    // `route_fleet` declared it: the shell reads it on every page, the paywall
    // included.
    gated.map_router(|r| r.layer(middleware::from_fn(org_context::org_middleware)))
}
