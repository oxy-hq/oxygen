//! Frontline HTTP surface: PIN sign-in for crew who hold no org membership, the
//! kiosks a PIN may be typed on, and the org-admin routes that enrol workers and
//! bind tablets.
//!
//! - [`frontline`]: the roster, the PIN login, enrolment and standing.
//! - [`frontline_devices`]: kiosk binding — the enrol link, the device cookie,
//!   and the org-admin device CRUD.
//! - [`frontline_kiosk_cookie`]: the `oxy_kiosk` cookie and its page-readable
//!   hint — one attribute builder for setting and clearing both.
//! - [`frontline_kiosk_mode`]: "leave kiosk mode" from the tablet itself, and
//!   the signed-in account the enrol confirm page warns about.
//!
//! Mounted by the `oxy-server` composition root through two seams, because the
//! surface sits on both sides of the auth stack:
//!
//! - [`public_routes`] rides `SurfaceSeams::public`, merged beside oxy-app's
//!   `build_public_routes` with **no** auth layer. A worker has nothing to
//!   authenticate with until they have signed in, and a tablet has nothing to
//!   present until it is bound.
//! - [`routes`] rides `SurfaceSeams::api`, merged into the protected tree before
//!   `apply_middleware`, and re-applies the org tree's own `org_middleware` +
//!   subscription guard.
//!
//! Every route here reads and writes Postgres and nothing else, so all of them
//! are FleetOk — declared explicitly in [`public_route_roles`] and
//! [`route_roles`] rather than left to the default, because the type gate
//! `RoleRouter::route_fleet` used to provide does not reach across the crate
//! line. Signing in has to survive the ide restarting: pinning login to the
//! singleton would mean a deploy locks every store out of its own checklists.

pub mod frontline;
pub mod frontline_devices;
pub mod frontline_kiosk_cookie;
pub mod frontline_kiosk_mode;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{get, patch, post, put};
use oxy_app::server::api::frontline_admin;
use oxy_app::server::api::middlewares::org_context::org_middleware;
use oxy_app::server::api::middlewares::subscription_guard::subscription_guard_middleware;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The unauthenticated half: sign-in and the kiosk binding it requires.
///
/// `device` tells the login page whether it is on an enrolled kiosk;
/// `devices/bind` is the one-time enrol link an admin opens on the tablet — GET
/// shows a confirm page, POST binds, because the link travels through things
/// that unfurl URLs.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/frontline/roster", get(frontline::roster))
        .route("/frontline/login", post(frontline::login))
        .route("/frontline/device", get(frontline_devices::device_status))
        .route(
            "/frontline/devices/bind",
            get(frontline_devices::bind_page).post(frontline_devices::bind_submit),
        )
}

/// The org-admin half, at `/orgs/{org_id}/frontline/*`.
///
/// Nested under the org rather than beside `/frontline/login`, because those are
/// public by necessity and these are the opposite: an org admin adding a person
/// to their org, which belongs with the rest of member management.
///
/// `org_middleware` must run BEFORE `subscription_guard` (the guard reads the
/// `OrgContext` the middleware inserts, and `OrgAdmin` needs it too). axum
/// applies the last-declared `.layer` as outermost, so this yields request order
/// `org_middleware → subscription_guard → handler` — the same order these routes
/// had inside oxy-app's `build_org_routes`.
///
/// `list_workers`, `set_worker_apps` and `reset_worker_pin` still live in oxy-app
/// (`frontline_admin`). They mount here because `/frontline/workers` pairs
/// `list_workers` with `enrol` on one path, and one path's verbs cannot be split
/// across two routers.
pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/orgs/{org_id}/frontline", org_frontline_routes())
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

fn org_frontline_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/workers",
            get(frontline_admin::list_workers).post(frontline::enrol),
        )
        // What a manager does after enrolment: which apps a worker opens, and a
        // forgotten PIN re-issued at the counter.
        .route(
            "/workers/{user_id}/apps",
            put(frontline_admin::set_worker_apps),
        )
        .route(
            "/workers/{user_id}/pin",
            post(frontline_admin::reset_worker_pin),
        )
        // The other half of enrolment. PATCH because nothing is deleted — a
        // worker who leaves keeps their row so their work stays attributed.
        .route("/workers/{user_id}", patch(frontline::set_standing))
        // The kiosks a PIN may be entered on. Revoking is how a lost tablet is
        // switched off; PATCH tunes a kiosk's sign-out without a new enrol link.
        .route(
            "/devices",
            get(frontline_devices::list_devices).post(frontline_devices::create_device),
        )
        .route(
            "/devices/{id}",
            axum::routing::delete(frontline_devices::revoke_device)
                .patch(frontline_devices::update_device),
        )
        // A lost or expired link for a tablet that never bound. Unbound only:
        // moving a bound kiosk is revoke-and-enrol, not a quiet re-point.
        .route(
            "/devices/{id}/enrol-link",
            post(frontline_devices::reissue_enrol_link),
        )
        // "Leave kiosk mode", from the tablet itself: revokes the kiosk the
        // request's own `oxy_kiosk` cookie names and clears that cookie. No
        // `{id}` in the path — the cookie is what says which kiosk, so an admin
        // can only ever switch off the browser they are holding.
        .route("/device/leave", post(frontline_kiosk_mode::leave_kiosk))
}

const FLEET: RouteRole = RouteRole::FleetOk;

/// Every route [`public_routes`] mounts, and which pod may serve it. Paths are
/// absolute: the public seam merges this crate at the `/api` root.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/frontline/roster",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/frontline/login",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/frontline/device",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/frontline/devices/bind",
            role: FLEET,
        },
    ]
}

/// Every route [`routes`] mounts, and which pod may serve it. Absolute paths,
/// for the same reason as [`public_route_roles`].
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/workers",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/workers/{user_id}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/workers/{user_id}/apps",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/workers/{user_id}/pin",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/devices",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/devices/{id}",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/devices/{id}/enrol-link",
            role: FLEET,
        },
        RouteRoleDecl {
            method: "*",
            path: "/orgs/{org_id}/frontline/device/leave",
            role: FLEET,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// axum checks for route conflicts eagerly as a router is built, so building
    /// each seam's Router is enough to catch an overlapping path within this
    /// surface — no server, no DB, no state.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
        let _ = public_routes();
    }

    /// Reproduce both merge points against stand-ins of oxy-app's real trees, so
    /// a path that collides with them panics here rather than at boot. The
    /// stand-ins are hand-maintained; keep them in sync with `build_public_routes`
    /// and `build_global_routes` (which need a DB-backed AppState).
    #[test]
    fn merges_into_the_two_seam_trees_without_conflict() {
        async fn h() {}

        let public: Router<AppState> = Router::new()
            .route("/auth/magic-link/request", post(h))
            .route("/health", get(h))
            .merge(public_routes());

        let root: Router<AppState> = Router::new()
            .nest("/orgs/{org_id}", Router::new().route("/members", get(h)))
            .merge(routes());

        let _ = (public, root);
    }

    /// The declaration lists are the only thing naming these routes to
    /// `classify`, so compare their PATHS with what the routers mount — a count
    /// would pass with a renamed route and a dead declaration beside it.
    ///
    /// The mounted set is read from this file's `.route("…")` literals, the same
    /// view the route catalog (`build_route_catalog.rs`) takes, which is why the
    /// paths stay literals rather than shared consts: the catalog only follows
    /// literals, and a const path would drop the route from `/api/_catalog`.
    /// [`declared_paths_are_really_mounted`] then checks that view against the
    /// built routers, so a mount the parse misreads cannot pass here.
    #[test]
    fn the_declaration_lists_cover_every_mounted_path() {
        let (public, org) = mounted_paths();
        assert_eq!(
            declared(public_route_roles()),
            public,
            "public routes and their declarations diverged",
        );
        assert_eq!(
            declared(route_roles()),
            org,
            "org-scoped routes and their declarations diverged",
        );
        assert!(
            public_route_roles()
                .iter()
                .chain(route_roles())
                .all(|d| d.role == RouteRole::FleetOk),
            "every frontline route reads and writes Postgres only",
        );
    }

    /// Every declared path resolves to a mounted route: `TRACE`, which no route
    /// here accepts, answers 405 on a mounted path and 404 on anything else,
    /// before any handler or state is touched. The org half is probed without
    /// `routes()`'s layers, since `org_middleware` would answer first.
    #[tokio::test]
    async fn declared_paths_are_really_mounted() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        let state = oxy_app::server::router::bare_app_state();
        let public = public_routes().with_state(state.clone());
        let org = Router::new()
            .nest(org_prefix(), org_frontline_routes())
            .with_state(state);
        let probes = [(public, public_route_roles()), (org, route_roles())];
        for (router, decls) in probes {
            for decl in decls {
                let uri = decl
                    .path
                    .split('/')
                    .map(|seg| {
                        if seg.starts_with('{') {
                            "00000000-0000-0000-0000-000000000000"
                        } else {
                            seg
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                let req = Request::builder()
                    .method(Method::TRACE)
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap();
                let status = router.clone().oneshot(req).await.unwrap().status();
                assert_eq!(
                    status,
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{} is declared but {uri} is not mounted",
                    decl.path,
                );
            }
        }
    }

    fn declared(decls: &[RouteRoleDecl]) -> BTreeSet<String> {
        decls.iter().map(|d| d.path.to_string()).collect()
    }

    /// This file's non-test source.
    fn source() -> &'static str {
        let src = include_str!("lib.rs");
        &src[..src.find("#[cfg(test)]").expect("test module marker")]
    }

    /// The body of `fn name` — up to the next top-level item.
    fn fn_body(name: &str) -> &'static str {
        let src = source();
        let start = src.find(&format!("fn {name}(")).expect("router fn");
        let rest = &src[start..];
        let end = rest[1..].find("\n}").map_or(rest.len(), |i| i + 2);
        &rest[..end]
    }

    /// Every string literal that opens a call to `marker` in `body`.
    fn literal_args(body: &str, marker: &str) -> Vec<String> {
        body.match_indices(marker)
            .filter_map(|(i, _)| {
                let arg = body[i + marker.len()..].trim_start();
                let arg = arg.strip_prefix('"')?;
                Some(arg[..arg.find('"')?].to_string())
            })
            .collect()
    }

    /// The prefix `routes()` nests the org half under.
    fn org_prefix() -> &'static str {
        let prefixes = literal_args(fn_body("routes"), ".nest(");
        assert_eq!(prefixes.len(), 1, "routes() nests exactly one tree");
        Box::leak(prefixes.into_iter().next().unwrap().into_boxed_str())
    }

    /// `(public, org)` mounted paths, absolute, as the `.route` calls name them.
    fn mounted_paths() -> (BTreeSet<String>, BTreeSet<String>) {
        let public = literal_args(fn_body("public_routes"), ".route(");
        let org = literal_args(fn_body("org_frontline_routes"), ".route(")
            .into_iter()
            .map(|p| format!("{}{p}", org_prefix()))
            .collect();
        (public.into_iter().collect(), org)
    }
}
