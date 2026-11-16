use oxy_app::surface::roles::declared_role_in;
use oxy_shared::fleet_role::RouteRole;

use super::*;

/// Every workspace route this crate mounts, as `oxy-app`'s
/// `build_workspace_routes` mounted it with `route_fleet` before 2026-11-16.
const MOUNTED: &[&str] = &[
    "/sql/{pathb64}",
    "/sql/query",
    "/semantic/objects",
    "/context-graph/sources",
    "/revision",
    "/revision/files",
    "/revision/files/content",
    "/semantic/topic/{file_path_b64}",
    "/semantic/view/{file_path_b64}",
    "/semantic/preagg-status",
    "/semantic/preagg-rebuild",
    "/semantic/compile",
    "/semantic",
];

/// The routes came from `oxy-app`, where `route_fleet`'s type proved the role
/// and wrote the declaration `* path FleetOk`. Here the role is a declaration,
/// so hold it: each is declared exactly once, method `*`, FleetOk — not merely
/// unclassified, which also reads FleetOk and is what a dropped row would look
/// like. A read pinned to the singleton would stop the semantic explorer, the
/// Context Graph and the file browser whenever the ide restarts.
#[test]
fn every_route_is_declared_fleet_ok_as_route_fleet_declared_it() {
    let declared = workspace_route_roles();
    assert_eq!(
        declared.len(),
        MOUNTED.len(),
        "one declaration per mounted route: {declared:?}"
    );
    for path in MOUNTED {
        let found: Vec<_> = declared.iter().filter(|d| d.path == *path).collect();
        assert_eq!(found.len(), 1, "{path}: declared {} times", found.len());
        assert_eq!(found[0].method, "*", "{path}");
        assert_eq!(found[0].role, RouteRole::FleetOk, "{path}");
    }
}

/// The classifier matches a live URI against these declarations rather than
/// falling to its default, including `/semantic` beside its longer siblings
/// and the two parameterised reads.
#[test]
fn the_classifier_matches_live_uris_against_the_declarations() {
    let decls: Vec<_> = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
        .collect();
    let ws = "d9830be4-c6a4-4c1e-9d55-6f0c3b1d2e7a";
    for (method, rel) in [
        ("POST", "/sql/cXVlcmllcy9hLnNxbA=="),
        ("POST", "/sql/query"),
        ("GET", "/semantic/objects"),
        ("GET", "/context-graph/sources"),
        ("GET", "/revision"),
        ("GET", "/revision/files"),
        ("GET", "/revision/files/content"),
        ("GET", "/semantic/topic/c2FsZXMudG9waWMueW1s"),
        ("GET", "/semantic/view/b3JkZXJzLnZpZXcueW1s"),
        ("GET", "/semantic/preagg-status"),
        ("POST", "/semantic/preagg-rebuild"),
        ("POST", "/semantic/compile"),
        ("POST", "/semantic"),
    ] {
        let uri = format!("/api/{ws}{rel}");
        assert_eq!(
            declared_role_in(&decls, method, &uri),
            Some(RouteRole::FleetOk),
            "{method} {uri} must match a FleetOk declaration"
        );
    }
}

/// The operation this crate hands the served document: the SQL execute, with
/// the request body an agent copies.
#[test]
fn the_openapi_part_documents_the_sql_execute() {
    let doc = openapi();
    let item = doc
        .paths
        .paths
        .get("/{workspace_id}/sql/query")
        .expect("POST /{workspace_id}/sql/query is documented");
    assert!(
        item.post.is_some(),
        "documented, but with no post operation"
    );
    let schemas = &doc.components.as_ref().expect("components").schemas;
    for schema in ["SQLParams", "SqlQueryResponse", "SqlErrorResponse"] {
        assert!(
            schemas.contains_key(schema),
            "{schema} is not in components"
        );
    }
}
