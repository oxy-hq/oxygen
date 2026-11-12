//! Airway's HTTP surface (Pending 5 in `internal-docs/domain-boundaries.md`).
//!
//! Three routes, all merged INSIDE the `/{workspace_id}` nest by `oxy-server`
//! through the workspace seam, so they inherit `workspace_middleware` exactly
//! as they did when they lived in oxy-app's `build_workspace_routes`:
//!
//! - `POST /source-uploads/reports` — report uploads for file-based sources,
//!   into the shared landing zone ([`source_upload`]);
//! - `GET /airway-pipelines` — the workspace's pipelines, from the compile
//!   boundary ([`pipelines`]);
//! - `GET /airway-pipelines/sp-api/marketplaces` — the `sp_api` connector's
//!   marketplaces, for the New Pipeline wizard ([`pipelines`]).
//!
//! Beside them, the rest of the airway context that used to live in `oxy-app`:
//!
//! - [`admin_sections`] — the staff console's `/admin/airway` section (the
//!   per-source-kind admission policy and the deployment-wide operational
//!   tier), mounted through the admin seam behind `PlatformOperate`;
//! - [`boot`] — installing airway's deployment tier at process start, which
//!   `oxy serve`, `oxy worker` and every `oxy airway` command do first;
//! - [`Command`] — the `oxy airway` subcommands, registered with
//!   `oxy-cli-shim` as their own owner.
//!
//! The run surface (`/agentic-airway`) stays in `agentic-http`.

// Laying out `upload_report`'s future exceeds rustc's default query depth of
// 128 in the `--release` build that produces the image — and only there: a
// dev-profile build stays under it, so PR CI passes. rustc's own suggestion.
#![recursion_limit = "256"]

mod admin;
pub mod boot;
mod cli;
mod pipelines;
mod source_upload;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use source_upload::{MAX_REPORT_BYTES, upload_report};

/// Public only because the module docs link to it: nothing outside this crate
/// reads it, and a private const referenced solely from doc links is dead code.
pub use source_upload::ZONE_VAR;

/// Every route this crate mounts, and which pod may serve it. Paths are
/// relative to the `/{workspace_id}` nest the seam merges into.
///
/// FleetOk, declared rather than defaulted: the upload writes to S3 and reads
/// nothing node-local, so pinning it to the singleton would cost HA for no
/// reason. It is deliberately NOT under `/agentic-airway`, which is an
/// `IdeOnly` `{*rest}` wildcard so a live run stays on the instance holding
/// the working copy — the carve-out is the upload, not the surface.
///
/// The two pipeline reads are FleetOk for the reason they always were: the
/// listing reads the compile boundary (`WorkspaceManagerReadOnly`, so the
/// compiler refuses a working-copy read), and the marketplace list is static.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "POST",
            path: "/source-uploads/reports",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/airway-pipelines",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/airway-pipelines/sp-api/marketplaces",
            role: RouteRole::FleetOk,
        },
    ]
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/airway-pipelines", get(pipelines::list_pipelines))
        // Static connector metadata for the New Pipeline wizard's marketplace
        // picker. Served rather than duplicated in the frontend so the list
        // has one source of truth (`source_factory::NA_MARKETPLACES`).
        .route(
            "/airway-pipelines/sp-api/marketplaces",
            get(pipelines::list_sp_api_marketplaces),
        )
        .merge(upload_routes())
}

/// The upload, with the body limit it needs and the reads do not.
fn upload_routes() -> Router<AppState> {
    Router::new()
        .route("/source-uploads/reports", post(upload_report))
        // Without this, axum 0.8's 2 MiB default governs and the handler's own
        // ceiling is unreachable — a larger report fails inside `field.bytes()`
        // as a 400, never the 413 the handler writes. At `Router` level rather
        // than on the `MethodRouter` for the same reason `oxy-api-tenancy`'s `onboarding` module
        // gives: the latter can interact unexpectedly with outer CORS preflight
        // handling on axum 0.8.
        // `MAX_REPORT_BYTES` plus slack, because this bounds the whole
        // multipart body while the constant bounds ONE FILE: boundaries, field
        // names, `pipeline_ref`, `workflow_id` and the period all ride along.
        // Set equal, a file a few hundred bytes under the ceiling passed the
        // client-side check and the handler's own check and still died here —
        // answered by this layer's terse 400, never the handler's 413 that
        // names the size. The handler stays the authority on the file itself.
        .layer(DefaultBodyLimit::max(MAX_REPORT_BYTES + 64 * 1024))
}

/// The staff console's airway section, mounted under `/api/admin` through the
/// admin seam behind `PlatformOperate`, the capability it had in `oxy-app`:
/// deployment-wide operational config, beside `workspace_health`, `routing`
/// and `metrics`. No route declarations: the console is FleetOk wholesale.
///
/// The per-workspace half carries its own scope fence, because the capability
/// layer cannot: `Resource::platform()` has no org, so a bounded grant passes
/// every capability gate. `admin::handlers` fences both override writes and
/// filters the overrides `get_config` returns; `admin::preview` narrows the
/// cross-tenant scan with `scope_org_filter` and reports the withheld rest as a
/// count.
///
/// KNOWN LIMITATION, accepted rather than overlooked: the **global row stays
/// fleet-wide**. A grant bounded to two orgs can still `PUT
/// /airway/config/{kind}` and change the policy every tenant resolves against.
/// It belongs to no org, so there is nothing to fence it on. The three ways to
/// close it were each worse than the gap: a third owner-only gate on the field
/// operators most need; refusing the global write for any bounded grant (the
/// surface then silently does half of what its UI shows); or a per-org row,
/// which contradicts what the row is (airway resolves admission per source
/// kind, fleet-wide, in `agentic_pipeline::airway_config::resolve_admission`).
/// It is acceptable because `PlatformOperate` is already the capability for
/// fleet-wide operational config, a scoped grant is issued to staff, not
/// tenants, and the save-confirmation gate previews the fleet-wide blast
/// radius. If that stops holding, the per-org row is the direction to take.
pub fn admin_sections() -> Vec<oxy_app::surface::AdminSection> {
    vec![oxy_app::surface::AdminSection {
        capability: oxy_app::surface::Action::PlatformOperate,
        routes: admin_routes(),
        decls: Vec::new(),
    }]
}

/// Relative to `/admin`. Its own function so the route catalog can seed it,
/// and the chain is written here rather than delegated to `admin::router()`:
/// the catalog's walker resolves a call by module suffix, and `admin::router`
/// also names `oxy-app`'s whole staff console, so it walked the wrong one.
pub fn admin_routes() -> Router<AppState> {
    use admin::{deployment, handlers, preview};
    use axum::routing::put;

    Router::new().nest(
        "/airway",
        Router::new()
            .route("/config", get(handlers::get_config))
            .route(
                "/config/{source_kind}",
                put(handlers::put_global_config).delete(handlers::delete_global_config),
            )
            .route(
                "/config/{source_kind}/workspaces/{workspace_id}",
                put(handlers::put_workspace_override).delete(handlers::delete_workspace_override),
            )
            // Distinct path depth from the `PUT`/`DELETE` route above, so
            // axum matches them separately.
            .route(
                "/config/{source_kind}/preview",
                get(preview::preview_policy),
            )
            // The operational tier. A sibling of `/config`, not a child:
            // `/config/{source_kind}` would swallow it as a source kind, and
            // it is deployment-wide rather than per kind in any case.
            .route(
                "/deployment-config",
                get(deployment::get_deployment_config)
                    .put(deployment::put_deployment_config)
                    .delete(deployment::delete_deployment_config),
            ),
    )
}

// `oxy airway`: its own owner in `oxy-cli-shim`, registered by `oxy-server`.
// The variant's doc is the command's `--help` text, carried over verbatim. A
// plain comment, not a doc comment: clap's derive turns a doc on this enum into
// the `about` of the whole `oxy` command, as `oxy-server`'s `ServerCommand`
// notes too.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Run airway ELT pipelines.
    ///
    /// Execute a `.airway.yml` pipeline (extract → normalize → load)
    /// and stream progress events. Requires OXY_DATABASE_URL to be set.
    Airway(cli::AirwayArgs),
}

/// Run `oxy airway …`. The error propagates to the shim, as it did from
/// `oxy-app`'s dispatcher.
pub async fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Airway(args) => cli::handle_airway_command(args).await?,
    }
    Ok(())
}

/// `oxy-app`'s in-source DB harness, shared by path (see `oxy-workspace-health`):
/// the admin section's and the boot's tests write the same singleton
/// `airway_deployment_config` row and contend on its one lock key.
///
/// Kept below every builder: `oxy-route-catalog`'s walker stops reading a file
/// at its first `#[cfg(test)]` module, so one placed above `routes()` hides it.
#[cfg(test)]
#[path = "../../app/src/server/test_support.rs"]
#[allow(dead_code)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    /// axum 0.8 checks for route conflicts eagerly, so reproduce the seam's
    /// merge point against a stand-in of the `/{workspace_id}` tree — a
    /// collision then fails here instead of as a boot panic in prod.
    #[test]
    fn merges_into_the_workspace_tree_without_conflict() {
        use axum::routing::get;
        async fn h() {}

        let _: Router<AppState> = Router::new()
            .route("/details", get(h))
            .route("/agentic-airway/{*rest}", get(h))
            .merge(routes());
    }
}
