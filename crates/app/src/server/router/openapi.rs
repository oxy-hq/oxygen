//! The OpenAPI router used by Swagger UI. Kept separate from the runtime
//! router because its route set is curated (not every handler is exposed).

use axum::body::Body;
use axum::http::Request;
use oxy::config::constants::DEFAULT_API_KEY_HEADER;
use sentry::integrations::tower::NewSentryLayer;
use tower::ServiceBuilder;
use utoipa::openapi::ExternalDocs;
use utoipa::openapi::security::{
    ApiKey as OApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityRequirement, SecurityScheme,
};
use utoipa::openapi::server::Server;
use utoipa_axum::router::OpenApiRouter;

use super::{IdeState, build_cors_layer};

pub async fn openapi_router() -> OpenApiRouter<IdeState> {
    OpenApiRouter::new()
        // The health probes' operations are `oxy-server`'s (`core_routes`,
        // 2026-11-16), merged in through `SurfaceSeams::openapi`.
        // The legacy API-key operations are `oxy-api-identity`'s, merged in
        // through `SurfaceSeams::openapi`.
        // `GET /workspaces/{workspace_id}` and `GET /orgs/{org_id}/workspaces`
        // are `oxy-api-workspaces`' (2026-11-16), merged in through
        // `SurfaceSeams::openapi`.
        // The agent listing, the thread operations and the run logs are
        // `oxy-api-threads`' (2026-11-16), merged in through
        // `SurfaceSeams::openapi`.
        // Workflow + automation routes have been retired; the new workflow
        // surface lives under `/agentic-workflows` (see agentic-http).
        // ── the data plane ────────────────────────────────────────────────
        //
        // Added for `oxyc schema`: these are the endpoints an agent needs to
        // construct a correct request for, which is a different set from the
        // ones the web app happens to call. Deliberately NOT web-UI surfaces —
        // an agent never uploads a logo or reorders a list.
        // `POST /{workspace_id}/sql/query` is `oxy-api-semantic`'s
        // (2026-11-16), merged in through `SurfaceSeams::openapi`.
        // `/projects/{project_id}/query` and `/semantic-query` are
        // `oxy-api-custom-apps`' (`data_plane`), merged in through
        // `SurfaceSeams::openapi`; `oxy-server`'s tests assert them there.
        // Turning "the customer" into the ids every other endpoint wants.
        // `GET /user` is `oxy-api-identity`'s, merged in through
        // `SurfaceSeams::openapi`, and `GET /orgs/{org_id}/workspaces`
        // `oxy-api-workspaces`' (above).
        // Database routes
        .layer(build_cors_layer())
        .layer(ServiceBuilder::new().layer(NewSentryLayer::<Request<Body>>::new_from_top()))
}

/// Markdown rendered in the Swagger UI header, and carried as the spec's
/// `info.description`. Documents both the HTTP API and the CLI tools
/// (`oxyc login`, `oxyc api`) that consume it; the body lives in `apidoc.md` so
/// the long markdown stays out of the code.
const APIDOC_DESCRIPTION: &str = include_str!("../../cli/commands/apidoc.md");

/// The finished OpenAPI document: [`openapi_router`]'s operations plus the
/// title, description, security schemes and server prefix that make it a
/// usable spec rather than a bare path list.
///
/// Two consumers, deliberately one builder: `oxy serve` hands it to Swagger UI
/// at `/apidoc`, and `oxyc openapi` prints it offline. A caller who can
/// only reach the binary gets exactly the document the running server serves.
///
/// Scope: `openapi_router`'s route set is **curated**, not the whole surface —
/// it is where the request/response *schemas* live. `oxyc routes` is the
/// complete endpoint list.
/// `extracted` are the operations extracted surface crates document
/// (`SurfaceSeams::openapi`) — `GET /orgs` among them, from `oxy-api-tenancy`.
/// Merged before the document-wide settings below, so they apply to all.
pub async fn build_openapi_doc(
    extracted: Vec<utoipa::openapi::OpenApi>,
) -> utoipa::openapi::OpenApi {
    let mut doc = openapi_router().await.into_openapi();
    for part in extracted {
        doc.merge(part);
    }

    doc.info.title = "Oxy API".to_string();
    doc.info.description = Some(APIDOC_DESCRIPTION.to_string());
    doc.info.contact = None;
    doc.info.license = None;

    let mut external_docs = ExternalDocs::new("https://oxygen-hq.com/docs");
    external_docs.description = Some("Oxy documentation".to_string());
    doc.external_docs = Some(external_docs);

    let mut components = doc.components.take().unwrap_or_default();
    // API key scheme — for service-to-service calls (X-API-Key header).
    components.security_schemes.insert(
        "ApiKey".to_string(),
        SecurityScheme::ApiKey(OApiKey::Header(ApiKeyValue::new(
            DEFAULT_API_KEY_HEADER.to_string(),
        ))),
    );
    // Bearer scheme — the JWT issued by `oxyc login` (and returned by the
    // magic-link flow). Pass via `Authorization: Bearer <token>`.
    components.security_schemes.insert(
        "BearerAuth".to_string(),
        SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
    );
    doc.components = Some(components);

    // Endpoints accept either scheme; clients only need to supply one.
    doc.security = Some(vec![
        SecurityRequirement::new("ApiKey", Vec::<String>::new()),
        SecurityRequirement::new("BearerAuth", Vec::<String>::new()),
    ]);
    doc.servers = Some(vec![Server::new("/api")]);
    doc
}

// `openapi_coverage_tests` (`the_agent_data_plane_is_documented`,
// `the_sql_request_body_carries_its_fields`) moved to `oxy-server`'s `main.rs`
// tests (Wave D2, 2026-11-16), where they read the served document: their one
// operation, `POST /{workspace_id}/sql/query`, is `oxy-api-semantic`'s, merged
// in through `SurfaceSeams::openapi`, so this crate's document alone no longer
// holds it. `every_security_requirement_names_a_registered_scheme` moved there
// the same way earlier the same day.
