//! Workspace previews' surface: staff open a branch of a workspace on the real
//! product, against real data, without the branch being live. The design is
//! in `oxy-app`'s `server::previews` module doc and
//! `internal-docs/workspace-previews.md`.
//!
//! This crate is the upper half, moved out of `oxy-app` on 2026-11-14:
//!
//! - the `/{workspace_id}/previews` routes ([`routes`], [`handlers`]) and the
//!   create / refresh / delete logic behind them ([`service`], [`revisions`]);
//! - the Airway change check ([`analyze`], [`checks`]), the transform compares
//!   ([`compare`]), Airway samples ([`sample`]) and preview-schema drops
//!   ([`drop`]) — four task kinds ([`tasks`]);
//! - the held runs' submit and read-back ([`runs`]);
//! - the TTL sweep that queues drops and compares ([`maintenance`],
//!   [`background`]);
//! - since 2026-11-16, the staging branch compile's two routes
//!   ([`compile_staging`], [`workspace_routes`]): `oxyc publish
//!   --semantic-branch` and `oxyc compile` compile a branch into a revision
//!   that is never promoted, through the same `compile_request` a preview's
//!   create and refresh use. They sit inside the workspace tree, not beside it.
//!
//! The lower half stays in `oxy-app` (`oxy_app::surface::previews`), because the
//! workspace runtime and the middleware below this crate read it: the registry
//! and its DDL, the namespace, the sources, the pin and read-only guard, the
//! run platform (`runtime`) and the run queue (`queue`: the queue step, the
//! change-check enqueue the compile worker calls, the run lifecycle, and the
//! per-pass sweep the global driver runs inline — rows only, all of it).
//! Nothing below may name this crate; `oxy-app` refuses to start without its
//! task kinds and its maintenance job (`PREVIEW_TASK_KINDS`,
//! `PREVIEW_BACKGROUND_JOBS`).
//!
//! Every route is FleetOk: creating and refreshing ask GitHub where the branch
//! is and queue a compile of that commit, and the rest touch only Postgres.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod analyze;
pub mod checks;
pub mod compare;
pub mod compile_staging;
pub mod drop;
pub mod handlers;
pub mod maintenance;
pub mod revisions;
pub mod runs;
pub mod sample;
pub mod service;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use oxy_app::surface::seams::{BackgroundSeam, TaskSeam};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The previews routes, relative to `/{workspace_id}/previews`.
///
/// Mounted BESIDE the workspace tree rather than inside it, behind the access
/// check alone (`workspace_access_middleware`), through
/// `SurfaceSeams::previews`. Two reasons: the full workspace middleware
/// resolves a revision and builds a workspace manager for `?branch=`, and here
/// `?branch=` names the preview being deleted or refreshed, not a working copy
/// to read; and a request carrying the preview header must still be able to
/// manage previews, which the read-only guard inside `workspace_middleware`
/// would refuse.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            post(handlers::create_preview)
                .get(handlers::list_previews)
                .delete(handlers::delete_preview),
        )
        .route("/refresh", post(handlers::refresh_preview))
        // Postgres only (the analyze row and its run outcome): any replica.
        .route("/checks", get(handlers::get_checks))
        // Held procedure dry runs: the handlers write and read rows; the run
        // itself executes on the worker fleet. Any replica.
        .route("/runs", post(handlers::start_run).get(handlers::list_runs))
        .route("/runs/{run_id}", get(handlers::get_run))
        // Airway samples' sandbox companies: rows only. Any replica.
        .route(
            "/sources",
            get(handlers::list_sources).put(handlers::put_source),
        )
}

/// Every path in [`routes`], FleetOk for every method, as `route_fleet`
/// declared them in `oxy-app`: create and refresh look the branch up on GitHub
/// and queue a compile of that commit (no working copy), and the rest read or
/// write Postgres rows only — viewing a preview's checks or runs must not need
/// the ide.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    const fn fleet(path: &'static str) -> RouteRoleDecl {
        RouteRoleDecl {
            method: "*",
            path,
            role: RouteRole::FleetOk,
        }
    }
    const ROLES: &[RouteRoleDecl] = &[
        fleet("/"),
        fleet("/refresh"),
        fleet("/checks"),
        fleet("/runs"),
        fleet("/runs/{run_id}"),
        fleet("/sources"),
    ];
    ROLES
}

/// The workspace seam's share: merged INSIDE `/api/{workspace_id}` (both
/// modes), where `build_workspace_routes` mounted them, so they keep the
/// workspace middleware, the subscription guard and the preview read-only
/// refusal keyed on `/compile/staging`. Unlike [`routes`], which sits beside
/// that tree.
///
/// Staging branch compile for a custom-app draft build to pin
/// (`oxyc publish --semantic-branch`). The POST asks GitHub where the
/// branch is and queues a compile of that commit; the status reads
/// Postgres. Both FleetOk. A branch GitHub does not have is refused
/// with the reason.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/compile/staging",
            post(compile_staging::enqueue_staging_compile),
        )
        .route(
            "/compile/staging/status",
            get(compile_staging::staging_compile_status),
        )
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree:
/// the two rows `oxy-app`'s `route_fleet` declared, method `*`, FleetOk. No
/// working copy is read: the POST compiles the commit GitHub has (a branch
/// only a working copy has is replayed to the Factory), the status is a
/// Postgres read.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        RouteRoleDecl {
            method: "*",
            path: "/compile/staging",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "*",
            path: "/compile/staging/status",
            role: RouteRole::FleetOk,
        },
    ];
    ROLES
}

/// The four task kinds previews queue, with their executors. Each was
/// registered in `oxy-app`'s driver registry until 2026-11-14; a node without
/// one fails the task as an unknown kind rather than running anything as
/// production.
pub fn tasks() -> Vec<TaskSeam> {
    vec![
        // The Airway change check. Reads the compile boundary and, at most, a
        // Reader on the workspace's Airhouse.
        TaskSeam {
            kind: analyze::PREVIEW_ANALYZE_KIND,
            build: |deps| Arc::new(analyze::PreviewAnalyzeExecutor::airhouse(deps.db.clone())),
        },
        // The TTL drop, queued by `maintenance`. Drops only schemas a registry
        // row claimed by the run vouches for, through a system Writer that
        // sends fixed DDL.
        TaskSeam {
            kind: drop::PREVIEW_SCHEMA_DROP_KIND,
            build: |deps| Arc::new(drop::PreviewSchemaDropExecutor::airhouse(deps.db.clone())),
        },
        // The compare of a finished transform build with live, queued by
        // `maintenance::enqueue_compares`. Reads only; its outcome is counts.
        TaskSeam {
            kind: compare::PREVIEW_COMPARE_KIND,
            build: |deps| Arc::new(compare::PreviewCompareExecutor::airhouse(deps.db.clone())),
        },
        // An Airway sample, queued by the run queue's step. Builds its own
        // preview platform from the run row; destination only in the
        // preview's schemas on a confined Writer.
        TaskSeam {
            kind: sample::PREVIEW_AIRWAY_SAMPLE_KIND,
            build: |deps| {
                Arc::new(sample::PreviewAirwaySampleExecutor::airhouse(
                    deps.db.clone(),
                ))
            },
        },
    ]
}

/// The TTL sweep: claim the registry rows of expired preview schemas and queue
/// their `preview_schema_drop` tasks, and queue compares of finished builds.
/// Only writes rows; runs regardless of `--no-workers`, idempotent across
/// replicas, as when `oxy-app`'s router started it.
pub fn background() -> Vec<BackgroundSeam> {
    vec![BackgroundSeam {
        name: "preview_maintenance",
        start: |_deps| {
            maintenance::spawn(maintenance::MaintenanceConfig::from_env());
            None
        },
    }]
}

#[cfg(test)]
#[path = "../../app/src/server/test_support.rs"]
#[allow(dead_code)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    /// Persisted contract: these strings are in `agentic_task_queue` rows.
    #[test]
    fn the_task_kinds_are_the_four_the_fleet_knew() {
        let kinds: Vec<&str> = tasks().iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                "preview_analyze",
                "preview_schema_drop",
                "preview_compare",
                "preview_airway_sample"
            ]
        );
    }

    /// Moved from `oxy-app`'s `role_manifest_tests` with the routes. Asked of
    /// the declarations, not of `classify`: an undeclared path classifies
    /// FleetOk by default, so a dropped declaration would still pass there.
    #[test]
    fn every_previews_route_is_declared_fleet_ok() {
        let declared: Vec<(&str, &str, RouteRole)> = route_roles()
            .iter()
            .map(|d| (d.method, d.path, d.role))
            .collect();
        let want: Vec<(&str, &str, RouteRole)> = [
            "/",
            "/refresh",
            "/checks",
            "/runs",
            "/runs/{run_id}",
            "/sources",
        ]
        .into_iter()
        .map(|p| ("*", p, RouteRole::FleetOk))
        .collect();
        assert_eq!(declared, want);
    }

    /// Moved from `oxy-app`'s `role_manifest_tests::
    /// compiling_a_branch_for_a_staging_build_is_fleet_ok` with the routes.
    /// Staging a branch for a custom-app build looks the branch up on GitHub
    /// and queues a compile of that commit; no working copy is read, so it
    /// must not need the ide. Declared, not defaulted: an undeclared path
    /// answers FleetOk too, so each path must be DECLARED, FleetOk for every
    /// method, and classify that way.
    #[test]
    fn compiling_a_branch_for_a_staging_build_is_fleet_ok() {
        use oxy_app::surface::roles::declared_role_in;
        let ws = "d9830be4-c6a4-4f89-11d3-9a0c0305e82c";
        let decls: Vec<_> = workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
            .collect();
        for path in ["/compile/staging", "/compile/staging/status"] {
            let declared: Vec<_> = workspace_route_roles()
                .iter()
                .filter(|d| d.path == path)
                .map(|d| (d.method, d.role))
                .collect();
            assert!(!declared.is_empty(), "{path} is declared");
            assert!(
                declared.iter().all(|d| *d == ("*", RouteRole::FleetOk)),
                "{path} is FleetOk for every method: {declared:?}"
            );
            for method in ["GET", "POST"] {
                let uri = format!("/api/{ws}{path}");
                assert_eq!(
                    declared_role_in(&decls, method, &uri),
                    Some(RouteRole::FleetOk),
                    "{method} {uri}"
                );
            }
        }
    }

    /// Both routes are mounted where the declarations say. TRACE, which neither
    /// takes, answers 405 from a mounted path and 404 from an unmounted one,
    /// without running a handler.
    #[tokio::test]
    async fn the_staging_compile_routes_are_mounted() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;
        let router = workspace_routes().with_state(oxy_app::surface::bare_app_state());
        for (uri, want) in [
            ("/compile/staging", StatusCode::METHOD_NOT_ALLOWED),
            ("/compile/staging/status", StatusCode::METHOD_NOT_ALLOWED),
            ("/compile/nothing-mounts-this", StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(got, want, "{uri}");
        }
    }
}
