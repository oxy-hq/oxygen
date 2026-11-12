//! Cloud-only global routes: logout, organization CRUD (with org-scoped
//! sub-routes for members, invitations, onboarding, workspaces, GitHub
//! integration) and the per-user GitHub account/installation routes.
//!
//! Not mounted in local mode — see [`super::protected`].

use axum::Router;
use axum::middleware;
use axum::routing::{delete, get, patch, post, put};

use crate::api::billing;
use crate::api::middlewares::{
    app_scope_guard, org_context, oxy_owner_or_app_admin_guard, platform_cap_guard,
    subscription_guard,
};
use crate::api::{admin, user, workspaces};
use crate::server::api::chat;
use crate::server::api::work;

use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use super::role_router::RoleRouter;
use super::tokens::{build_org_api_access_routes, build_token_routes};
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
        .route_fleet("/logout", get(user::logout))
        // Personal access tokens and the calling-token pair. Merged HERE, not in
        // `build_protected_routes`: this builder is the route catalog's seed for
        // the global tree, so a merge beside it would serve the routes and leave
        // them out of `oxyc routes`.
        .merge(build_token_routes(app_state))
        // ── Chat ────────────────────────────────────────────────────────────
        //
        // Every route here is `route_fleet`, INCLUDING the SSE stream, and that
        // is the interesting call. The route-classification skill pins live
        // streams to the ide — but that rule is about runs executing in-process
        // against a working copy. This stream is a fan-out over persisted data,
        // woken by Postgres LISTEN/NOTIFY, and touches no working copy, no
        // `.git` and no state dir.
        //
        // Pinning it to the singleton would be actively wrong twice over: chat
        // would die on every deploy, and it would only work at all for whoever
        // happened to be routed to the ide. This is the "reads must stay HA"
        // half of that skill, and a conversation is the most read-shaped thing
        // in the product.
        .route_fleet(
            "/chat/channels",
            get(chat::handlers::list_channels).post(chat::handlers::create_channel),
        )
        .route_fleet(
            "/chat/channels/{id}/join",
            post(chat::handlers::join_channel),
        )
        .route_fleet(
            "/chat/channels/{id}/messages",
            get(chat::handlers::list_messages).post(chat::handlers::post_message),
        )
        .route_fleet("/chat/channels/{id}/read", post(chat::handlers::mark_read))
        .route_fleet("/chat/channels/{id}/stream", get(chat::handlers::stream))
        // The assignment graph. NOT nested under `/orgs/{org_id}`, and that is
        // load-bearing: `org_middleware` resolves an org membership, and a
        // frontline worker holds none by design. Nesting these would lock out
        // exactly the people the graph exists to route work to.
        //
        // Authorization is the query filter — you see what you are assigned,
        // supervise, or hold the addressed role for. See the module docs.
        .route_fleet(
            "/work",
            get(work::handlers::list).post(work::handlers::create),
        )
        // Documents — read side — moved to the `oxy-api-documents` sibling
        // crate, merged at this same root by `oxy-server` through the
        // `extra_api_routes` seam. Still NOT under `/orgs/{org_id}`: frontline
        // workers hold no org membership, and `org_middleware` would reject them.
        .route_fleet("/work/{id}", axum::routing::patch(work::handlers::update))
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
        .route_fleet(
            "/partner-publish-consent",
            get(crate::server::api::partner_publish_consent::get_consent)
                .put(crate::server::api::partner_publish_consent::set_consent),
        )
        // Organization → API access: service accounts, their tokens, and the
        // inventory of every token reaching the org. Pure Postgres, FleetOk.
        .merge(build_org_api_access_routes(app_state))
        // Locations and tenant-defined roles. Under `/orgs/{org_id}` so the
        // `OrgAdmin` extractor can see the org it is guarding — a body-carried
        // org is invisible to a path-resolved guard, which is the hole
        // `create_app` has to patch by hand.
        //
        // `route_fleet`: Postgres only, no working copy.
        .route_fleet(
            "/locations",
            get(crate::server::api::operating_graph::locations::list_locations)
                .post(work::handlers::create_location),
        )
        // The rest of the operating graph — the hierarchy, what each
        // integration calls a place, the positions vocabulary, and who holds
        // which position where. `internal-docs/operating-graph.md`.
        .route_fleet(
            "/locations/{id}",
            axum::routing::patch(crate::server::api::operating_graph::locations::patch_location),
        )
        .route_fleet(
            "/locations/{id}/external-ids/{system}",
            put(crate::server::api::operating_graph::locations::put_external_id)
                .delete(crate::server::api::operating_graph::locations::delete_external_id),
        )
        .route_fleet(
            "/roles",
            get(work::handlers::list_roles).post(work::handlers::create_role),
        )
        // Documents — manage side — moved to `oxy-api-documents`, which nests
        // them at `/orgs/{org_id}` itself and re-applies `org_middleware` +
        // `subscription_guard`, the two layers this function wraps them in.
        .route_fleet(
            "/roles/{id}",
            axum::routing::patch(crate::server::api::operating_graph::positions::patch_role)
                .delete(crate::server::api::operating_graph::positions::delete_role_handler),
        )
        .route_fleet(
            "/assignments",
            get(crate::server::api::operating_graph::assignments::list)
                .post(crate::server::api::operating_graph::assignments::create),
        )
        .route_fleet(
            "/assignments/{id}",
            axum::routing::delete(crate::server::api::operating_graph::assignments::delete),
        )
        // The three workspace-creating onboarding routes moved to the
        // `oxy-api-onboarding` sibling crate. They CREATE a checkout on disk
        // through raw `std::fs` and git rather than an extractor, so no type
        // gate can see them — and the crate sits outside this router, so
        // `route_ide` cannot reach them either. They are declared by hand at
        // the `nest_declared` seam that mounts the crate.
        .route_fleet("/workspaces", get(workspaces::list_workspaces))
        // Deletes the working copy. The compiler found this one: the handler
        // takes `WorkspaceRootWorkingCopy`, which resolves only from `IdeState`.
        .route_ide("/workspaces/{id}", delete(workspaces::delete_workspace))
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
