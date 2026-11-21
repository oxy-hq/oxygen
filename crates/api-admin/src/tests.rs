//! The section, nest and seam tests for [`crate`]'s routers: what each
//! mounts, where, behind which capability. Out of `lib.rs` to keep it under
//! the file cap.

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

/// The sections `oxy-app`'s `admin::router` merged before the moves: the
/// audit log behind `PlatformAudit`, the grant table behind
/// `PlatformGrants`, the feature-flag console behind `PlatformOperate` and
/// the route-role table behind `PlatformOperate` (its own `route_layer` there,
/// so its own section here), each declaring nothing.
#[test]
fn the_section_is_the_one_oxy_app_merged() {
    let sections = admin_sections();
    let got: Vec<_> = sections
        .iter()
        .map(|s| (s.capability, s.decls.len()))
        .collect();
    assert_eq!(
        got,
        vec![
            (Action::PlatformAudit, 0),
            (Action::PlatformGrants, 0),
            (Action::PlatformOperate, 0),
            (Action::PlatformOperate, 0)
        ]
    );
}

/// Every audit route is mounted where the console calls it. TRACE, which
/// none takes, answers 405 from a mounted path and 404 from an unmounted
/// one, without running a handler or reaching a database.
#[tokio::test]
async fn every_audit_route_is_mounted() {
    let router = audit::router().with_state(oxy_app::surface::bare_app_state());
    let org = "00000000-0000-0000-0000-000000000001";
    for (uri, want) in [
        ("/audit".to_string(), StatusCode::METHOD_NOT_ALLOWED),
        (
            format!("/audit/verify/{org}"),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            format!("/audit/anchor/{org}"),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        ("/audit/".to_string(), StatusCode::NOT_FOUND),
        ("/audit/verify".to_string(), StatusCode::NOT_FOUND),
        ("/auditx".to_string(), StatusCode::NOT_FOUND),
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

/// Every grant-table route is mounted where the console calls it
/// (`/api/admin/app-admins`, `/api/admin/app-admins/{id}`). TRACE, which
/// none takes, answers 405 from a mounted path and 404 from an unmounted
/// one, without running a handler or reaching a database.
#[tokio::test]
async fn every_app_admins_route_is_mounted() {
    let router = app_admins::router().with_state(oxy_app::surface::bare_app_state());
    let id = "00000000-0000-0000-0000-000000000001";
    for (uri, want) in [
        ("/app-admins".to_string(), StatusCode::METHOD_NOT_ALLOWED),
        (format!("/app-admins/{id}"), StatusCode::METHOD_NOT_ALLOWED),
        ("/app-admins/".to_string(), StatusCode::NOT_FOUND),
        (format!("/app-admins/{id}/deeper"), StatusCode::NOT_FOUND),
        ("/app-adminsx".to_string(), StatusCode::NOT_FOUND),
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

/// Every feature-flag console route is mounted where the web app and
/// `oxyc flags` call it (`/api/admin/feature-flags`, `/evaluate`,
/// `/{key}`), and the org member's read is not on the staff tree. TRACE,
/// which none takes, answers 405 from a mounted path and 404 from an
/// unmounted one, without running a handler or reaching a database.
#[tokio::test]
async fn every_feature_flag_console_route_is_mounted() {
    let router = feature_flags::router().with_state(oxy_app::surface::bare_app_state());
    let org = "00000000-0000-0000-0000-000000000001";
    for (uri, want) in [
        ("/feature-flags".to_string(), StatusCode::METHOD_NOT_ALLOWED),
        (
            "/feature-flags/evaluate".to_string(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "/feature-flags/home_v3".to_string(),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "/feature-flags/home_v3/x".to_string(),
            StatusCode::NOT_FOUND,
        ),
        ("/feature-flagsx".to_string(), StatusCode::NOT_FOUND),
        (format!("/orgs/{org}/feature-flags"), StatusCode::NOT_FOUND),
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

/// The route-role table is mounted where the console calls it
/// (`/api/admin/routing-manifest`), in the section [`admin_sections`] hands
/// in last, and nowhere else in this crate's staff trees. TRACE, which it
/// does not take, answers 405 from a mounted path and 404 from an unmounted
/// one, without running a handler.
#[tokio::test]
async fn the_routing_manifest_is_mounted_in_its_own_section() {
    let state = oxy_app::surface::bare_app_state();
    let trace = |uri: &'static str| {
        Request::builder()
            .method(Method::TRACE)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    };
    let router = routing::router().with_state(state.clone());
    for (uri, want) in [
        ("/routing-manifest", StatusCode::METHOD_NOT_ALLOWED),
        ("/routing-manifest/x", StatusCode::NOT_FOUND),
        ("/routing-manifestx", StatusCode::NOT_FOUND),
    ] {
        let got = router.clone().oneshot(trace(uri)).await.unwrap().status();
        assert_eq!(got, want, "{uri}");
    }
    let mut mounting = Vec::new();
    for (i, section) in admin_sections().into_iter().enumerate() {
        let got = section
            .routes
            .with_state(state.clone())
            .oneshot(trace("/routing-manifest"))
            .await
            .unwrap()
            .status();
        if got != StatusCode::NOT_FOUND {
            mounting.push((i, section.capability));
        }
    }
    assert_eq!(mounting, vec![(3, Action::PlatformOperate)]);
}

/// The one row `oxy-app`'s `route_fleet("/feature-flags", ..)` declared
/// inside `/orgs/{org_id}`: declared, not merely unclassified (which also
/// reads FleetOk and is what a dropped row would look like), and matched
/// by the classifier on a live URI.
#[test]
fn the_org_flag_read_is_declared_fleet_ok() {
    use oxy_app::surface::roles::declared_role_in;
    let api: Vec<_> = route_roles()
        .iter()
        .map(|d| (d.method, format!("/api{}", d.path), d.role))
        .collect();
    assert_eq!(api.len(), 1);
    assert_eq!(
        declared_role_in(
            &api,
            "GET",
            "/api/orgs/00000000-0000-0000-0000-000000000000/feature-flags"
        ),
        Some(RouteRole::FleetOk)
    );
}

/// The api seam's router mounts the org member's flag read at
/// `/orgs/{org_id}/feature-flags` and never the staff half, which is
/// behind the console's door. TRACE answers 405 from a mounted path and
/// 404 from an unmounted one; `org_middleware` runs first on the mounted
/// one and, with no caller, refuses before any handler, so only "not 404"
/// is asserted there.
#[tokio::test]
async fn the_api_seam_carries_the_org_flag_read_and_not_the_console() {
    let router = routes().with_state(oxy_app::surface::bare_app_state());
    let org = "00000000-0000-0000-0000-000000000001";
    for (uri, mounted) in [
        (format!("/orgs/{org}/feature-flags"), true),
        (format!("/orgs/{org}/feature-flags/x"), false),
        ("/feature-flags".to_string(), false),
        ("/admin/feature-flags".to_string(), false),
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
        if mounted {
            assert_ne!(got, StatusCode::NOT_FOUND, "{uri}");
        } else {
            assert_eq!(got, StatusCode::NOT_FOUND, "{uri}");
        }
    }
}

/// Every assume-role route is mounted where the web app and `oxyc assume`
/// call it (`/api/assume`, `/api/assume/current`, `/api/assume/history`),
/// and the api seam's router is the assume router. TRACE, which none takes,
/// answers 405 from a mounted path and 404 from an unmounted one, without
/// running a handler or reaching a database.
#[tokio::test]
async fn every_assume_route_is_mounted() {
    let router = routes().with_state(oxy_app::surface::bare_app_state());
    for (uri, want) in [
        ("/assume", StatusCode::METHOD_NOT_ALLOWED),
        ("/assume/current", StatusCode::METHOD_NOT_ALLOWED),
        ("/assume/history", StatusCode::METHOD_NOT_ALLOWED),
        ("/assume/", StatusCode::NOT_FOUND),
        ("/assume/current/x", StatusCode::NOT_FOUND),
        ("/assumex", StatusCode::NOT_FOUND),
        // Never under `/admin`: that is the door acting closes.
        ("/admin/assume", StatusCode::NOT_FOUND),
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

/// No section or nest of this crate mounts assume-role: one would put the
/// exit behind the staff door and the refusal while acting.
#[tokio::test]
async fn assume_role_is_never_behind_the_staff_door() {
    let state = oxy_app::surface::bare_app_state();
    let trees = admin_sections()
        .into_iter()
        .map(|s| s.routes)
        .chain(admin_nests().into_iter().map(|n| n.routes));
    for tree in trees {
        for uri in ["/assume", "/assume/current", "/assume/history"] {
            let got = tree
                .clone()
                .with_state(state.clone())
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
            assert_eq!(got, StatusCode::NOT_FOUND, "{uri} on a staff tree");
        }
    }
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
