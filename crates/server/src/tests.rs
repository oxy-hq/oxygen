//! The crate root's own unit tests, lifted out of an inline `mod tests` in
//! `main.rs` on 2026-11-22 (decompose UP2) so the crate root stays inside its
//! size budget as modules move up into it. The test paths are unchanged; the
//! composition they test is `lib.rs`'s since decompose LT.

use super::*;

/// The command tree `main` parses, hidden commands included.
fn root_commands() -> Vec<String> {
    oxy_cli_shim::subcommand_names(&cli_owners())
}

/// The release build (`--no-default-features --features
/// custom-app-functions`, `.github/actions/build-oxy-cli`) has no dev
/// tooling: no `dev`, no top-level `seed` or `test`. CI compiles this test
/// (the release-shape clippy line in `ci.yaml`, `--profile test`).
#[cfg(not(feature = "dev-tools"))]
#[test]
fn a_build_without_dev_tools_cannot_seed() {
    let names = root_commands();
    for gone in ["dev", "seed", "test"] {
        assert!(
            !names.iter().any(|n| n == gone),
            "`oxy {gone}` in {names:?}"
        );
    }
    assert!(names.iter().any(|n| n == "compile"), "{names:?}");
}

/// A default (debug) build has the `dev` group and, for one release, the
/// old top-level `seed` and `test`.
#[cfg(feature = "dev-tools")]
#[test]
fn a_dev_tools_build_has_the_dev_group_and_the_old_spellings() {
    let names = root_commands();
    for name in ["dev", "seed", "test", "compile", "serve"] {
        assert!(
            names.iter().any(|n| n == name),
            "no `oxy {name}` in {names:?}"
        );
    }
}

/// Composing the seams panics on a path collision between sibling crates,
/// which would otherwise surface only as a boot panic.
#[test]
fn the_seams_compose_without_conflict() {
    let _ = api_seam_routes();
    let _ = workspace_seam_routes();
    let _ = public_seam_routes();
}

/// `oxy-app` serves whatever table it is handed and an empty one is valid
/// to it, so the composition root is where "the binary ships the real
/// table" is checked.
#[test]
fn the_binary_ships_the_generated_route_table() {
    let catalog = oxy_route_catalog::catalog();
    assert!(
        catalog.routes.len() > 400,
        "{} routes",
        catalog.routes.len()
    );
    assert!(catalog.routes.iter().any(|r| r.path == "/api/_catalog"));
}

/// The experiment and simulation task kinds reached the fleet from inside
/// `oxy-app`'s registry until the context moved to `oxy-api-experiment`; now
/// only this composition hands them over, so only here can "a queued
/// `experiment_estimate` has an executor" be checked. A kind missing from the
/// seams sits in the queue forever and no run ever reports; the tick missing
/// means no run ever advances.
#[test]
fn the_composition_runs_every_experiment_kind() {
    let seams = surface_seams();
    let kinds: Vec<&str> = seams.tasks.iter().map(|t| t.kind).collect();
    for kind in [
        "simulation_run",
        "experiment_price",
        "experiment_check",
        "experiment_estimate",
        "experiment_interim",
    ] {
        assert!(kinds.contains(&kind), "no executor for `{kind}`: {kinds:?}");
    }
    assert!(
        seams
            .global_ticks
            .iter()
            .any(|t| t.name == "experiment_tick"),
        "the experiment tick is not handed to the global driver"
    );
    assert_eq!(
        crate::router::require_task_kinds(&seams.tasks),
        Ok(()),
        "a kind registered twice would shadow one executor with another"
    );
}

/// The weekly custom-app usage report's pass reached the global driver
/// from inside `oxy-app` (`router::recovery` called it inline) until the
/// report's surface moved to `oxy-api-billing`; now only this composition
/// hands it over, once per loop of the driver as it ran. Missing, no
/// report is ever written and nothing else fails; at `AfterSchedules`, a
/// cloud node that claimed no workspace would not run it.
#[test]
fn the_composition_runs_the_usage_report_on_every_loop() {
    let seams = surface_seams();
    let ticks: Vec<_> = seams
        .global_ticks
        .iter()
        .map(|t| (t.name, t.pass))
        .collect();
    assert!(
        ticks.contains(&("usage_report", crate::router::TickPass::EveryLoop)),
        "the usage report's pass is not handed to the driver's loop: {ticks:?}"
    );
}

/// The document built from the seams boot hands over (`surface_seams()`),
/// not a hand-built list of them.
///
/// `GET /orgs` is one of the lookups an agent needs a schema for (see
/// `oxy-app`'s `the_agent_data_plane_is_documented`). Its handler moved to
/// `oxy-api-tenancy`, so only the document the composition root assembles
/// can show it — a dropped `openapi:` entry would lose it silently.
#[tokio::test]
async fn the_served_openapi_document_includes_the_surfaces() {
    let doc = crate::router::build_openapi_doc(surface_seams().openapi).await;
    // `GET /user`, the lookup that turns a credential into a user, is
    // `oxy-api-identity`'s since 2026-11-15.
    let user = doc
        .paths
        .paths
        .get("/user")
        .expect("GET /user is missing from the OpenAPI document");
    assert!(user.get.is_some(), "/user is documented but carries no get");
    let orgs = doc
        .paths
        .paths
        .get("/orgs")
        .expect("GET /orgs is missing from the OpenAPI document");
    assert!(orgs.get.is_some(), "/orgs is documented but carries no get");
    // The health probes, the composition root's own since 2026-11-16
    // (`core_routes::openapi`; were in `oxy-app`'s `openapi_router`), with
    // the response schemas the two bodied ones name.
    for path in ["/health", "/ready", "/live", "/version"] {
        let item = doc
            .paths
            .paths
            .get(path)
            .unwrap_or_else(|| panic!("{path} is missing from the OpenAPI document"));
        let get = item
            .get
            .as_ref()
            .unwrap_or_else(|| panic!("{path} is documented but carries no get"));
        assert_eq!(
            get.tags.as_deref(),
            Some(&["Health".to_string()][..]),
            "{path}"
        );
    }
    let schemas = &doc.components.as_ref().expect("components").schemas;
    for schema in [
        "HealthCheckResponse",
        "DatabaseStatus",
        "VersionResponse",
        "BuildInfo",
    ] {
        assert!(
            schemas.contains_key(schema),
            "{schema} is not in components"
        );
    }
    // The thread and agent operations and the run logs, `oxy-api-threads`'
    // since 2026-11-16 (were in `oxy-app`'s `openapi_router`), with the
    // request and response schemas they name.
    for (path, methods) in [
        ("/{workspace_id}/agents", &["get"][..]),
        ("/{workspace_id}/threads", &["get", "post", "delete"][..]),
        ("/{workspace_id}/threads/{id}", &["get", "delete"][..]),
        ("/{workspace_id}/threads/{id}/stop", &["post"][..]),
        ("/{workspace_id}/threads/bulk-delete", &["post"][..]),
        ("/logs", &["get"][..]),
    ] {
        let item = doc
            .paths
            .paths
            .get(path)
            .unwrap_or_else(|| panic!("{path} is missing from the OpenAPI document"));
        for method in methods {
            let op = match *method {
                "get" => item.get.as_ref(),
                "post" => item.post.as_ref(),
                "delete" => item.delete.as_ref(),
                _ => unreachable!(),
            };
            assert!(op.is_some(), "{path} is documented but carries no {method}");
        }
    }
    for schema in [
        "ThreadItem",
        "ThreadsResponse",
        "CreateThreadRequest",
        "BulkDeleteThreadsRequest",
        "LogItem",
        "LogsResponse",
    ] {
        assert!(
            schemas.contains_key(schema),
            "{schema} is not in components"
        );
    }
    // The workspace lookups, `oxy-api-workspaces`' since 2026-11-16 (were
    // in `oxy-app`'s `openapi_router`; `GET /orgs/{org_id}/workspaces`
    // was a row of its `the_agent_data_plane_is_documented`: the second
    // step of turning a customer into the ids every endpoint takes), with
    // the response schemas they name.
    for path in ["/orgs/{org_id}/workspaces", "/workspaces/{workspace_id}"] {
        let item = doc
            .paths
            .paths
            .get(path)
            .unwrap_or_else(|| panic!("{path} is missing from the OpenAPI document"));
        assert!(
            item.get.is_some(),
            "{path} is documented but carries no get"
        );
    }
    for schema in ["WorkspaceSummary", "WorkspaceDetailsResponse"] {
        assert!(
            schemas.contains_key(schema),
            "{schema} is not in components"
        );
    }
    // The custom-app data plane, which `oxyc schema` answers from: its
    // handlers moved to `oxy-api-custom-apps` (`data_plane`).
    for path in [
        "/projects/{project_id}/query",
        "/projects/{project_id}/semantic-query",
    ] {
        let item = doc
            .paths
            .paths
            .get(path)
            .unwrap_or_else(|| panic!("{path} is missing from the OpenAPI document"));
        assert!(
            item.post.is_some(),
            "{path} is documented but carries no post"
        );
    }
}

/// The data-plane operations `oxyc schema` exists to serve.
///
/// A green build proves nothing here: `routes!(handler)` compiles fine
/// whether or not the handler carries a `#[utoipa::path]`, and a handler
/// whose annotation was dropped simply vanishes from the document. This is
/// the only thing that would notice.
///
/// These are the endpoints an AGENT needs a schema for — SQL, semantic
/// query, and the two lookups that turn a customer into the ids everything
/// else takes. Web-UI-only surfaces are deliberately not in this list and
/// should not be added to it.
///
/// Moved here from `oxy-app`'s `router::openapi` (Wave D2, 2026-11-16),
/// where it read `build_openapi_doc(Vec::new())`: its one row, the SQL
/// execute, is `oxy-api-semantic`'s now, so only the served document holds
/// it. (`GET /user`, `GET /orgs`, `GET /orgs/{org_id}/workspaces` and the
/// custom-app data plane are asserted in
/// `the_served_openapi_document_includes_the_surfaces`.)
const AGENT_PATHS: &[(&str, &str)] = &[("/{workspace_id}/sql/query", "post")];

/// The operation for `method`, or `None` — read off the typed `PathItem`
/// rather than by grepping serialized JSON, which is how the first version
/// of this test managed to pass on `rendered.contains("get")` matching the
/// word "get" anywhere in a description.
fn operation<'a>(
    item: &'a utoipa::openapi::path::PathItem,
    method: &str,
) -> Option<&'a utoipa::openapi::path::Operation> {
    match method {
        "get" => item.get.as_ref(),
        "post" => item.post.as_ref(),
        "put" => item.put.as_ref(),
        "patch" => item.patch.as_ref(),
        "delete" => item.delete.as_ref(),
        other => panic!("unhandled method in AGENT_PATHS: {other}"),
    }
}

#[tokio::test]
async fn the_agent_data_plane_is_documented() {
    let doc = crate::router::build_openapi_doc(surface_seams().openapi).await;
    for (path, method) in AGENT_PATHS {
        let item = doc.paths.paths.get(*path).unwrap_or_else(|| {
            panic!(
                "{path} is missing from the OpenAPI document — `oxyc schema {path}` \
                 would answer 'not documented'. Either the #[utoipa::path] annotation \
                 was dropped from its handler, or the routes!(..) line was."
            )
        });
        assert!(
            operation(item, method).is_some(),
            "{path} is documented but carries no {method} operation"
        );
    }
}

/// A schema with no properties is worse than none: it reads as
/// "documented, and the body is empty". `SQLParams` is the one an agent
/// copies first.
///
/// Resolved through the operation's own `$ref` rather than by grepping the
/// whole components blob — the earlier version asserted only that the
/// strings "sql" and "database" appeared *somewhere* in the document, which
/// they do in half a dozen unrelated descriptions.
///
/// Moved here from `oxy-app`'s `router::openapi` with the one above (Wave
/// D2): the served document is the only one that still holds the operation.
#[tokio::test]
async fn the_sql_request_body_carries_its_fields() {
    use utoipa::openapi::{RefOr, Schema};

    let doc = crate::router::build_openapi_doc(surface_seams().openapi).await;
    let item = doc
        .paths
        .paths
        .get("/{workspace_id}/sql/query")
        .expect("the SQL endpoint is documented");
    let body = operation(item, "post")
        .and_then(|op| op.request_body.as_ref())
        .expect("the SQL endpoint declares a request body");

    let reference = body
        .content
        .get("application/json")
        .and_then(|c| match &c.schema {
            Some(RefOr::Ref(r)) => Some(r.ref_location.clone()),
            _ => None,
        })
        .expect("the request body is a $ref into components");
    let name = reference
        .rsplit('/')
        .next()
        .expect("a $ref ends in a component name")
        .to_string();

    let schema = doc
        .components
        .as_ref()
        .and_then(|c| c.schemas.get(&name))
        .unwrap_or_else(|| panic!("{name} is referenced but not in components/schemas"));

    let RefOr::T(Schema::Object(object)) = schema else {
        panic!("{name} did not resolve to an object schema");
    };
    for field in ["sql", "database"] {
        assert!(
            object.properties.contains_key(field),
            "{name} lost its `{field}` property — an agent copying this body would omit it"
        );
    }
    assert!(
        object.required.iter().any(|r| r == "sql"),
        "{name} no longer marks `sql` required"
    );
}

/// Every operation on a path item, paired with its method name.
fn operations(
    item: &utoipa::openapi::path::PathItem,
) -> Vec<(&'static str, &utoipa::openapi::path::Operation)> {
    [
        ("get", item.get.as_ref()),
        ("put", item.put.as_ref()),
        ("post", item.post.as_ref()),
        ("delete", item.delete.as_ref()),
        ("options", item.options.as_ref()),
        ("head", item.head.as_ref()),
        ("patch", item.patch.as_ref()),
        ("trace", item.trace.as_ref()),
    ]
    .into_iter()
    .filter_map(|(name, op)| op.map(|o| (name, o)))
    .collect()
}

/// Every `security(..)` name an operation cites must be a scheme this
/// document actually registers.
///
/// Six annotations shipped citing `"Bearer"` while the registered scheme is
/// `"BearerAuth"` — a dangling reference that generates a valid-looking
/// document in which no client can work out how to authenticate. Nothing
/// fails on it: utoipa does not resolve the name, and the JSON is still
/// well-formed.
#[tokio::test]
async fn every_security_requirement_names_a_registered_scheme() {
    // The served document, the seams' operations included (was
    // `oxy-app`'s `build_openapi_doc(Vec::new())`, until the thread and
    // agent operations left for `oxy-api-threads` on 2026-11-16).
    let doc = crate::router::build_openapi_doc(surface_seams().openapi).await;
    let registered: Vec<String> = doc
        .components
        .as_ref()
        .map(|c| c.security_schemes.keys().cloned().collect())
        .unwrap_or_default();
    assert!(
        !registered.is_empty(),
        "the document registers no security schemes at all"
    );

    let mut checked = 0usize;
    for (path, item) in &doc.paths.paths {
        for (method, op) in operations(item) {
            for requirement in op.security.iter().flatten() {
                // `SecurityRequirement`'s map is private, so the scheme
                // names are read back off its serialized form — which is
                // also exactly what a client sees.
                let as_json: serde_json::Value =
                    serde_json::to_value(requirement).expect("serialize security requirement");
                let Some(map) = as_json.as_object() else {
                    continue;
                };
                for name in map.keys() {
                    checked += 1;
                    assert!(
                        registered.contains(name),
                        "{method} {path} cites security scheme {name:?}, which is not \
                         registered. Registered: {registered:?}"
                    );
                }
            }
        }
    }
    // A loop that inspected nothing would pass silently, which is exactly
    // how the dangling `"Bearer"` survived six annotations in the first
    // place.
    assert!(
        checked > 5,
        "only {checked} security requirements inspected — the walk found almost nothing"
    );
}

/// Two Rust types are both called `SemanticQueryResponse` — the custom-app
/// data plane's (`oxy-api-custom-apps`) and the semantic explorer's
/// (`oxy-app`) — and utoipa names a component after the ident alone — so without an
/// explicit `#[schema(as = ...)]` on each they collapse into one schema and
/// whichever registers last wins, handing `oxyc schema` the wrong body
/// shape for the other endpoint.
///
/// Checked by SHAPE rather than by name, so renaming either component
/// keeps the test meaningful: the SQL response is an untagged enum (no
/// `properties`), the semantic one is an object with `columns` and `rows`.
#[tokio::test]
async fn the_two_query_response_schemas_stayed_distinct() {
    use utoipa::openapi::{RefOr, Schema};

    // Only the served document holds both, so only it can show a collision.
    let doc = crate::router::build_openapi_doc(surface_seams().openapi).await;
    let components = doc.components.as_ref().expect("components");

    let object_with = |name: &str| -> bool {
        matches!(
            components.schemas.get(name),
            Some(RefOr::T(Schema::Object(o))) if o.properties.contains_key("columns")
        )
    };

    assert!(
        object_with("SemanticQueryResult"),
        "SemanticQueryResult is missing or is not the {{columns, rows}} object — the two \
         SemanticQueryResponse types have probably collided again"
    );
    assert!(
        components.schemas.contains_key("SqlQueryResponse"),
        "SqlQueryResponse is missing — the SQL endpoint's response schema was lost"
    );
    assert!(
        !object_with("SqlQueryResponse"),
        "SqlQueryResponse now looks like the semantic {{columns, rows}} object, so the two \
         collided"
    );
}
