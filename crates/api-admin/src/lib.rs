//! The staff console's own crate: the cross-context staff machinery that no
//! tenant context owns (`internal-docs/domain-boundaries.md` § The admin
//! split, S1).
//!
//! Started in the admin split's PR 7 with the durable task fleet's operator
//! console ([`internal_jobs`], `/api/admin/internal-jobs/*`), out of
//! `oxy-app`'s `api::admin::internal_jobs*`. The audit log (`audit`), the
//! grant table's routes (`app_admins`), the `/api/assume` routes and `routing`
//! follow it in later PRs, and the console shell (`admin::router`: the door,
//! the capability layers, `extracted_sections`) last, with the router
//! assembly.
//!
//! Only the staff console goes here. A section about a tenant context goes to
//! that context's surface, never here by audience: orgs went to
//! `oxy-api-tenancy`, the user directory to `oxy-api-identity`, the compile
//! console to `oxy-api-previews`.
//!
//! Mounted by the `oxy-server` composition root through one seam,
//! `SurfaceSeams::admin_nests` ([`admin_nests`]), chained ahead of
//! `oxy-api-previews`' compile console so the route-role table keeps the
//! order it had in `oxy-app` (internal jobs, then compiles). Every route reads
//! or writes Postgres only (the task queue, the run and workspace tables), so
//! the nest is FleetOk.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod internal_jobs;
mod internal_jobs_dead_letter;
mod internal_jobs_failure_groups;
mod internal_jobs_fleet;
mod internal_jobs_reach;
mod internal_jobs_rows;

use oxy_app::surface::roles::RouteRole;
use oxy_app::surface::{Action, AdminNest};

/// The staff console's sibling trees this crate hands `oxy-app`: today the
/// task fleet's operator console, nested at `/api/admin/internal-jobs` behind
/// the staff door, the refusal while acting and `PlatformOperate`.
///
/// Internal jobs is a nest rather than a section because its routes are flat
/// (no `/internal-jobs/` prefix on each), so the guards answer every path it
/// mounts, a wrong method included (a path the tree does not match is the
/// outer router's 404). Operating the durable task fleet is Oxy's own
/// machinery, not tenant data — `PlatformOperate` — and the handlers narrow by
/// grant scope themselves (`internal_jobs_reach`). `oxy-app` applies the
/// guards; this crate only names the capability.
pub fn admin_nests() -> Vec<AdminNest> {
    vec![AdminNest {
        prefix: "/internal-jobs",
        capability: Action::PlatformOperate,
        routes: internal_jobs::router(),
        role: RouteRole::FleetOk,
        why: "job admin reads the task queue tables",
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// The nest `oxy-app`'s `admin::nests` built before the move: the same
    /// prefix, capability, placement and reason.
    #[test]
    fn the_nest_is_the_one_oxy_app_mounted() {
        let nests: Vec<_> = admin_nests()
            .iter()
            .map(|n| (n.prefix, n.capability, n.role, n.why))
            .collect();
        assert_eq!(
            nests,
            vec![(
                "/internal-jobs",
                Action::PlatformOperate,
                RouteRole::FleetOk,
                "job admin reads the task queue tables"
            )]
        );
    }

    /// Every route the console serves is mounted where the web app calls it.
    /// TRACE, which none takes, answers 405 from a mounted path and 404 from
    /// an unmounted one, without running a handler or reaching a database.
    #[tokio::test]
    async fn every_internal_jobs_route_is_mounted() {
        let router = internal_jobs::router().with_state(oxy_app::surface::bare_app_state());
        let task = "task-0";
        for (uri, want) in [
            ("/queue-stats".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (
                "/recent-failures".to_string(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                "/failure-groups".to_string(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("/dead-letter".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (
                format!("/dead-letter/{task}/reenqueue"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                format!("/dead-letter/{task}"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("/workers".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/scheduled".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/run-reaper".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/run-retention".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/nothing-mounts-this".to_string(), StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
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
