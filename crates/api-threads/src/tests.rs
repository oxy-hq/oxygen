//! The surface's own contract: what it mounts, what it declares, and that the
//! two agree. The served router's mount of it is checked where the router is
//! composed, in `oxy-server`'s `served_router_tests`.

use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxy_app::surface::roles::declared_role_in;
use tower::ServiceExt;

/// Every concrete workspace route, with path parameters filled.
const WORKSPACE_MOUNTED: &[&str] = &[
    "/threads",
    "/threads/bulk-delete",
    "/threads/65c121b2-0000-0000-0000-000000000000",
    "/threads/65c121b2-0000-0000-0000-000000000000/stop",
    "/agents",
    "/artifacts/65c121b2-0000-0000-0000-000000000000",
    "/charts/chart-1.json",
    "/logs",
    "/builder-availability",
    "/results/files/65c121b2-0000-0000-0000-000000000000.parquet",
];

async fn trace(router: &Router, uri: &str) -> StatusCode {
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

/// The routes came from `oxy-app`, where `route_fleet` wrote these rows. Here
/// the role is a declaration, so hold it: each row is DECLARED FleetOk exactly
/// once — not merely unclassified, which `classify` also answers FleetOk and
/// is what a dropped row would look like.
#[test]
fn every_workspace_route_is_declared_fleet_ok_exactly_once() {
    let declared = workspace_route_roles();
    assert_eq!(
        declared.len(),
        10,
        "four thread paths, the agent listing, artifacts, charts, logs, builder \
         availability and result files"
    );
    for decl in declared {
        let same = declared
            .iter()
            .filter(|d| d.method == decl.method && d.path == decl.path)
            .count();
        assert_eq!(
            same, 1,
            "{} {}: declared {same} times",
            decl.method, decl.path
        );
        assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
        assert_eq!(decl.method, "*", "{}", decl.path);
    }
}

/// The declarations hold against the role classifier the way the served router
/// uses them: prefixed by the workspace tree, then matched against a live URI.
/// Every mounted route must match a declaration, not the default.
#[test]
fn every_mounted_workspace_route_classifies_fleet_ok() {
    let decls: Vec<_> = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
        .collect();
    for path in WORKSPACE_MOUNTED {
        let uri = format!("/api/d9830be4-c6a4{path}");
        for method in ["GET", "POST", "DELETE"] {
            assert_eq!(
                declared_role_in(&decls, method, &uri),
                Some(RouteRole::FleetOk),
                "{method} {uri} must match a FleetOk declaration"
            );
        }
    }
}

/// Thread and run-history READS are persisted data, so they must serve from
/// any replica: pinning them to the singleton would make *viewing* a
/// conversation need the Factory, the HA bug the split exists to prevent.
/// Was the `/threads` half of `oxy-app`'s `persisted_data_reads_are_fleet_ok`
/// and the result-file rows of its runtime-artifact pin, asked of `classify`
/// there; asked of this crate's declarations here, so a dropped row fails
/// rather than taking the FleetOk default.
#[test]
fn persisted_data_reads_are_declared_fleet_ok() {
    let decls: Vec<_> = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
        .collect();
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        ("GET", format!("/api/{ws}/threads")),
        ("GET", format!("/api/{ws}/threads/t-1")),
        ("GET", format!("/api/{ws}/logs")),
        // result_files::{store,get}: mirror on write, fetch on local miss.
        ("GET", format!("/api/{ws}/results/files/file-123")),
        ("DELETE", format!("/api/{ws}/results/files/file-123")),
        // A chart is mirrored the same way.
        ("GET", format!("/api/{ws}/charts/chart-1.json")),
    ] {
        assert_eq!(
            declared_role_in(&decls, method, &path),
            Some(RouteRole::FleetOk),
            "{method} {path} is a persisted-data read and must serve from any replica"
        );
    }
}

/// A declaration with no route behind it (or a route moved without its
/// declaration) leaves the two lists disagreeing. TRACE, which no route here
/// takes, answers 405 from a mounted path and 404 from an unmounted one,
/// without running a handler.
#[tokio::test]
async fn every_workspace_route_is_mounted() {
    let router = workspace_routes().with_state(oxy_app::surface::bare_app_state());
    for uri in WORKSPACE_MOUNTED {
        assert_eq!(
            trace(&router, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri}: not mounted"
        );
    }
    assert_eq!(
        trace(&router, "/nothing-mounts-this").await,
        StatusCode::NOT_FOUND,
        "control"
    );
}

/// The nine operations `oxy-app`'s curated document listed, each still
/// carrying its method. A `routes!(..)` line compiles whether or not its
/// handler keeps a `#[utoipa::path]`, so only the document can show a loss.
#[test]
fn the_documented_operations_are_the_nine_the_curated_document_listed() {
    let doc = openapi();
    let paths = &doc.paths.paths;
    let has = |path: &str, method: &str| {
        paths.get(path).is_some_and(|item| match method {
            "get" => item.get.is_some(),
            "post" => item.post.is_some(),
            "delete" => item.delete.is_some(),
            _ => false,
        })
    };
    for (path, method) in [
        ("/{workspace_id}/agents", "get"),
        ("/{workspace_id}/threads", "get"),
        ("/{workspace_id}/threads", "post"),
        ("/{workspace_id}/threads", "delete"),
        ("/{workspace_id}/threads/{id}", "get"),
        ("/{workspace_id}/threads/{id}", "delete"),
        ("/{workspace_id}/threads/{id}/stop", "post"),
        ("/{workspace_id}/threads/bulk-delete", "post"),
        // Documented without its workspace prefix, as it always was.
        ("/logs", "get"),
    ] {
        assert!(has(path, method), "{method} {path} is not documented");
    }
}

/// Every concrete chat route, with path parameters filled.
const CHAT_MOUNTED: &[&str] = &[
    "/chat/channels",
    "/chat/channels/65c121b2-0000-0000-0000-000000000000/join",
    "/chat/channels/65c121b2-0000-0000-0000-000000000000/messages",
    "/chat/channels/65c121b2-0000-0000-0000-000000000000/read",
    "/chat/channels/65c121b2-0000-0000-0000-000000000000/stream",
];

/// The five chat rows `oxy-app`'s `route_fleet` wrote in `build_global_routes`,
/// each declared FleetOk exactly once — the SSE stream included: it is a
/// fan-out over Postgres, and pinning it to the singleton would kill chat on
/// every deploy.
#[test]
fn every_chat_route_is_declared_fleet_ok_exactly_once() {
    let declared = route_roles();
    assert_eq!(declared.len(), 5, "the five `/chat/channels*` paths");
    for decl in declared {
        let same = declared
            .iter()
            .filter(|d| d.method == decl.method && d.path == decl.path)
            .count();
        assert_eq!(
            same, 1,
            "{} {}: declared {same} times",
            decl.method, decl.path
        );
        assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
        assert_eq!(decl.method, "*", "{}", decl.path);
    }
    let decls: Vec<_> = declared
        .iter()
        .map(|d| (d.method, format!("/api{}", d.path), d.role))
        .collect();
    for path in CHAT_MOUNTED {
        for method in ["GET", "POST"] {
            assert_eq!(
                declared_role_in(&decls, method, &format!("/api{path}")),
                Some(RouteRole::FleetOk),
                "{method} /api{path} must match a FleetOk declaration"
            );
        }
    }
}

#[tokio::test]
async fn every_chat_route_is_mounted() {
    let router = routes().with_state(oxy_app::surface::bare_app_state());
    for uri in CHAT_MOUNTED {
        assert_eq!(
            trace(&router, uri).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri}: not mounted"
        );
    }
    assert_eq!(
        trace(&router, "/chat/nothing-mounts-this").await,
        StatusCode::NOT_FOUND,
        "control"
    );
}

/// The name `oxy-app`'s boot check (`CHAT_BACKGROUND_JOBS`) refuses to start
/// without; the two must agree, or every serve process refuses to boot.
#[test]
fn the_background_job_is_the_chat_delivery_listener() {
    let names: Vec<&str> = background().iter().map(|b| b.name).collect();
    assert_eq!(names, ["chat_delivery_listener"]);
}
