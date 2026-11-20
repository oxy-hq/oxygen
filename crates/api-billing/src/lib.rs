//! Billing's HTTP surface (`internal-docs/domain-boundaries.md` § The admin
//! split).
//!
//! Started in the admin split's PR 8b with the weekly custom-app usage
//! report's surface ([`usage_report`]: the console's routes, who gets it,
//! the delivery and the weekly pass), out of `oxy-app`'s
//! `api::admin::usage_report`. The report's domain half is `oxy-usage-report`,
//! below this crate. Named for billing because the staff billing console
//! (`admin::billing`) and the tenant billing routes (`api::billing`) follow it
//! here in PR 9: the report is what a deployment's usage costs, read by the
//! staff who bill for it.
//!
//! Mounted by the `oxy-server` composition root through two seams:
//!
//! - `SurfaceSeams::admin` ([`admin_sections`]): two sections merged inside
//!   `/api/admin`, ahead of the refusal while acting, behind the capabilities
//!   `admin::router` gave them in-tree. `oxy-app` applies the staff door, the
//!   refusal and the capability guard; this crate only names the capability.
//! - `SurfaceSeams::global_ticks` ([`global_ticks`]): the weekly pass, at
//!   `TickPass::EveryLoop`, which runs it where `router::recovery` called it
//!   inline: once per iteration of the global driver's loop, on every process
//!   that runs the driver, whatever that node claimed.
//!
//! Every route reads or writes Postgres only (the report, preference and
//! delivery rows, the staff grants), and sends mail; nothing reads a working
//! copy, so the console's FleetOk wildcard places them and they declare
//! nothing.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod usage_report;

use oxy_app::surface::seams::{GlobalTickSeam, TickPass};
use oxy_app::surface::{Action, AdminSection};

/// The staff console's sections this crate hands `oxy-app`, merged inside
/// `/api/admin` before the refusal while acting, each behind the capability
/// `admin::router` gave it in-tree:
///
/// - the report itself and the caller's own switch (`/usage-report`,
///   `/usage-report/email-preference`, `/usage-report/send-to-me`) behind
///   `PlatformOperate`, the capability [`usage_report::delivery::AUDIENCE`]
///   mails, so the mail's link always opens;
/// - who gets it, and switching it off for another person
///   (`/usage-report/recipients*`) behind `PlatformGrants`: the staff list,
///   so the capability that administers staff access.
///
/// Sections, not nests, because that is how they were mounted: a
/// `route_layer` on a router merged inside `admin::router`, which is exactly
/// what `extracted_sections` builds. Neither declares anything: the console's
/// `/{*rest}` FleetOk wildcard places them, as it did. The handlers narrow a
/// bounded grant to the orgs it reaches themselves (`staff::scope`).
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        AdminSection {
            capability: Action::PlatformOperate,
            routes: usage_report::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformGrants,
            routes: usage_report::recipients::router(),
            decls: Vec::new(),
        },
    ]
}

/// The weekly custom-app usage report's pass ([`usage_report::job`]), run by
/// the global driver once per iteration of its loop (`TickPass::EveryLoop`):
/// where `router::recovery` called `job::tick` inline before the move, after
/// the pass and the previews sweep, on every process that runs the driver
/// (an `all`/`ide` serve with `OXY_INPROC_GLOBAL_WORKER` on, each `oxy
/// worker`), in both modes and whatever that node claimed.
///
/// Not `TickPass::AfterSchedules`: in the cloud pass that runs only on a node
/// that claimed a workspace, and the report belongs to none — a deployment
/// with no workspace to claim would never write one. The job throttles itself
/// to once per ten minutes per process and claims its work by unique rows, so
/// every driver running it writes one report and sends one mail between them.
/// It reports nothing advanced: it logs its own outcome, as it did.
pub fn global_ticks() -> Vec<GlobalTickSeam> {
    vec![GlobalTickSeam {
        name: "usage_report",
        pass: TickPass::EveryLoop,
        tick: |db, _now| {
            Box::pin(async move {
                usage_report::job::tick(db).await;
                0
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Every path the console calls, by the section that must mount it.
    const REPORT: &[&str] = &[
        "/usage-report",
        "/usage-report/email-preference",
        "/usage-report/send-to-me",
    ];
    const RECIPIENTS: &[&str] = &[
        "/usage-report/recipients",
        "/usage-report/recipients/someone@example.com",
    ];

    /// TRACE, which no route takes: 405 from a mounted path, 404 from an
    /// unmounted one, without running a handler or reaching a database.
    async fn trace(router: &axum::Router, uri: &str) -> StatusCode {
        router
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
            .status()
    }

    /// The two sections `oxy-app`'s `admin::router` merged before the move:
    /// the report behind `PlatformOperate` and its recipients behind
    /// `PlatformGrants`, each mounting its own paths and not the other's, and
    /// neither declaring a placement. A swapped capability, or a route moved
    /// into the other section, would hand the staff list to whoever reads the
    /// report (or the reverse), and every route would still answer.
    #[tokio::test]
    async fn each_section_mounts_its_own_routes_behind_the_capability_it_had() {
        let sections = admin_sections();
        let got: Vec<_> = sections
            .iter()
            .map(|s| (s.capability, s.decls.len()))
            .collect();
        assert_eq!(
            got,
            vec![(Action::PlatformOperate, 0), (Action::PlatformGrants, 0)]
        );
        for (section, own, other) in [
            (&sections[0], REPORT, RECIPIENTS),
            (&sections[1], RECIPIENTS, REPORT),
        ] {
            let router = section
                .routes
                .clone()
                .with_state(oxy_app::surface::bare_app_state());
            for path in own {
                assert_eq!(
                    trace(&router, path).await,
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{:?} must mount {path}",
                    section.capability
                );
            }
            for path in other {
                assert_eq!(
                    trace(&router, path).await,
                    StatusCode::NOT_FOUND,
                    "{:?} must not mount {path}",
                    section.capability
                );
            }
            for path in [
                "/usage-report/",
                "/usage-reportx",
                "/usage-report/recipients/a/b",
            ] {
                assert_eq!(trace(&router, path).await, StatusCode::NOT_FOUND, "{path}");
            }
        }
    }

    /// The pass runs on every iteration of the driver's loop, as the inline
    /// call in `router::recovery` did, and not only on a node that claimed a
    /// workspace.
    #[test]
    fn the_weekly_pass_runs_on_every_loop_of_the_driver() {
        let ticks: Vec<_> = global_ticks().iter().map(|t| (t.name, t.pass)).collect();
        assert_eq!(ticks, vec![("usage_report", TickPass::EveryLoop)]);
    }
}
