//! `oxy-server`'s library: everything the `oxy` binary runs, behind a thin
//! `main.rs` that keeps only the process entry (argv, the runtime, telemetry
//! bring-up and teardown). A library target, not more code in the binary, so
//! a test outside this crate can build what the binary builds: an integration
//! test cannot import a bin (decompose LT, 2026-11-23).
//!
//! Its crate ident is `oxy_server`, so the modules here log as
//! `oxy_server::…`. In the binary they logged as `oxy::…`, the same ident as
//! the core library's (`telemetry::sentry_filter_lt_pins_tests` pins the two
//! alike).

// Laying out the futures reached from `cli::run` exceeds rustc's
// default query depth since SeaORM 2.0 deepened its query types
// (`recursion_limit_guard`).
#![recursion_limit = "256"]

mod admission;
// The admin console shell (`/admin`, every surface's `AdminSection` merged),
// the router's dependencies (`router`, `role_manifest`, `role_middleware`) and
// `serve`'s boot refusal (`auth_provider_check`): up from `oxy-app` in
// decompose LIFT (2026-11-23). Public, as they were there: tests outside this
// crate build the real router (`token_auth`, the surfaces' route roles, the
// custom-app and experiment suites).
pub mod admin;
mod app_platform;
pub mod auth_provider_check;
// Which build this process is (the image tag, and what `build.rs` stamps): up
// from `oxy-app`'s `server::build_identity` in decompose BID (2026-11-25).
// Outside `core_routes/` on purpose: `oxy-route-catalog`'s build script watches
// that directory, and the health probes are not its only reader.
pub mod build_identity;
#[cfg(test)]
mod catalog_tests;
// The one-shot commands (`compile`, `migrate`, `migrate-agents`, the stubs) and
// the process's `prepare`, which `main.rs` hands the shim: up from `oxy-app`'s
// `cli` in decompose CLI (2026-11-25).
pub mod cli;
mod commands;
mod core_routes;
mod custom_apps_seams;
// `pub` for the demo seed (`dev::seed`, decompose SD): the surfaces'
// integration tests build their fixtures on it.
#[cfg(feature = "dev-tools")]
pub mod dev;
mod drivers;
#[cfg(test)]
mod healthcheck_tests;
mod logging;
// Core's managed-connection port, installed by `cli::prepare` (decompose CLI).
mod managed_connection_resolver;
mod metrics_server;
mod observability_boot;
#[cfg(test)]
mod recursion_limit_guard;
pub mod role_manifest;
pub mod role_middleware;
pub mod router;
#[cfg(test)]
mod served_router_tests;
#[cfg(test)]
mod shared_state_boot_tests;
mod shared_state_check;
mod static_assets;
mod worker_health;
mod worker_metrics;
mod worker_runtime;

use router::{SurfaceSeam, SurfaceSeams};

/// What `main.rs` calls besides the composition below: the subscriber it
/// installs before anything logs, and the span store's drain at exit.
pub use logging::init as init_logging;
pub use observability_boot::shutdown as shutdown_observability;

/// The commit this binary was built from, short form — `"dev"` for a local
/// build, since `build.rs` has only what CI puts in `GITHUB_SHA`.
///
/// Lives here rather than in a build script on `oxy` so that the platform
/// library stays build-script-free: a `rustc-env` on `oxy` would put a
/// per-commit input at the root of the graph and rebuild `oxy` plus its nine
/// direct dependents on every CI commit, where this crate is the composition
/// root and already pays that cost (internal-docs/rust-build-performance.md).
/// Up from `oxy-app` with `build.rs` in decompose BID (2026-11-25).
pub const BUILD_SHA: &str = env!("GIT_HASH");

type SeamRouter = axum::Router<router::AppState>;

/// The sibling crates merged at the protected-tree root. A function rather
/// than inline in `main` so the test below builds the SAME composition boot
/// does: axum rejects a colliding path eagerly on `.merge`, and each crate's
/// own probe only merges against a stand-in, never against its siblings.
fn api_seam_routes() -> SeamRouter {
    oxy_api_github::routes()
        .merge(oxy_api_tenancy::routes())
        // The partner console publishes a client's app through the custom-app
        // surface's `AppLifecycle`, which this root hands it (a surface may not
        // depend on another).
        .merge(
            oxy_api_tenancy::partner_console::routes()
                .layer(axum::Extension(custom_apps_seams::app_lifecycle())),
        )
        .merge(oxy_api_tenancy::onboarding::routes())
        .merge(oxy_api_documents::routes())
        .merge(oxy_api_frontline::routes())
        .merge(oxy_api_identity::routes())
        .merge(oxy_api_operating_graph::routes())
        .merge(oxy_api_notifications::routes())
        .merge(oxy_api_slack::routes())
        .merge(oxy_api_threads::routes())
        .merge(oxy_api_workspaces::routes())
        // Assume-role (`/assume*`), `oxy-api-admin`'s since the admin split
        // (2026-11-20): at this root, NOT behind the staff door, because acting
        // closes `/admin` and the exit cannot live behind the door it locks.
        // Undeclared (FleetOk), as `oxy-app`'s global tree merged it. With it
        // since 2026-11-21, the org member's flag read
        // (`/orgs/{org_id}/feature-flags`): `org_middleware` re-applied and no
        // subscription guard, as `build_org_routes` mounted it, declared FleetOk.
        .merge(oxy_api_admin::routes())
        // The tenant's billing (`/orgs/{org_id}/billing*`), `oxy-api-billing`'s
        // since the admin split's PR 9: `org_middleware` re-applied and no
        // subscription guard, as `build_org_routes` mounted it, so a paywalled
        // org still reaches it.
        .merge(oxy_api_billing::routes())
        // The environment planes writes pin a semantic branch through a port
        // this crate implements (a surface may not depend on another). The
        // staff console's "Test notifications" reads an app's kinds through a
        // port the notifications surface implements (a surface may not depend
        // on another).
        .merge(
            oxy_api_custom_apps::routes()
                .layer(axum::Extension(custom_apps_seams::semantic_pinner()))
                .layer(axum::Extension(oxy_api_notifications::live_kinds())),
        )
}

/// The sibling crates merged beside the public tree, outside the auth stack.
fn public_seam_routes() -> SeamRouter {
    core_routes::public_routes()
        .merge(oxy_api_frontline::public_routes())
        .merge(oxy_api_identity::public_routes())
        .merge(oxy_api_custom_apps::public_routes())
        .merge(oxy_api_slack::public_routes())
        .merge(oxy_api_world_model::public_routes())
        .merge(oxy_api_github::public_routes())
        // Stripe's webhook (`POST /webhooks/stripe`), `oxy-api-billing`'s since
        // the admin split's PR 9: signature-verified over the raw body.
        .merge(oxy_api_billing::public_routes())
        // The OAuth callbacks (`/quickbooks/oauth/callback`,
        // `/oauth/{provider}/callback`), `oxy-api-workspace-config`'s since
        // 2026-11-21: the vendor redirects a bare browser here, and the
        // single-use state nonce is the guard.
        .merge(oxy_api_workspace_config::public_routes())
}

/// The sibling crates merged inside the `/{workspace_id}` nest.
fn workspace_seam_routes() -> SeamRouter {
    oxy_api_tenancy::onboarding::workspace_routes()
        .merge(oxy_api_tenancy::workspace_routes())
        .merge(oxy_api_airway::routes())
        .merge(oxy_api_custom_apps::workspace_routes())
        .merge(oxy_api_experiment::workspace_routes())
        .merge(oxy_api_world_model::workspace_routes())
        .merge(oxy_api_metric_monitoring::workspace_routes())
        .merge(oxy_api_observability::workspace_routes())
        .merge(oxy_api_automations::workspace_routes())
        .merge(oxy_api_workspace_config::workspace_routes())
        .merge(oxy_api_previews::workspace_routes())
        .merge(oxy_api_identity::workspace_routes())
        .merge(oxy_api_threads::workspace_routes())
        .merge(oxy_api_workspaces::workspace_routes())
        .merge(oxy_api_semantic::workspace_routes())
        .merge(oxy_api_eval::workspace_routes())
}

/// The sibling crates merged into the curated `/external/api/{workspace_id}`
/// tree standalone apps call.
fn external_seam_routes() -> SeamRouter {
    oxy_api_world_model::external_routes()
        .merge(oxy_api_metric_monitoring::external_routes())
        .merge(oxy_api_semantic::external_routes())
}

/// Every surface this composition root mounts, by seam. `cli` forwards it into
/// `serve`'s `api_router`. A function rather than inline in `main` for the
/// reason `api_seam_routes` is one: `served_router_tests` builds the router from
/// the SAME value boot hands over, so "cloud mode serves `/orgs`" is checked
/// against the real composition and not a copy of it.
///
/// Roles travel WITH the routes. `oxy-api-github` mounts a Postgres-only
/// surface on the api seam, so the FleetOk default is the truth and it declares
/// nothing there. Its GitHub App webhook (`POST /webhooks/github`, out of
/// `oxy-app`'s public tree on 2026-11-15) rides `public` — GitHub carries no
/// user credential, so the handler verifies its HMAC instead — and is declared
/// `*` FleetOk, as `route_fleet` declared it (Postgres and the compile queue).
/// Tenancy's onboarding clones a repository and scaffolds `config.yml` onto
/// node-local disk, and its partner console's create-org scaffolds the new
/// org's Default workspace — neither can take that default, and no type gate
/// inside the binary can see across the crate line to stop them.
/// `oxy-api-documents` is Postgres + presigned S3 everywhere but
/// `POST /documents/ask`, which resolves an agent config out of the working
/// copy, so it declares that one route.
///
/// `oxy-api-frontline` is Postgres-only too, but it declares its FleetOk
/// explicitly: its routes left `route_fleet`, whose type gate stated that, and
/// it is the one surface on the PUBLIC seam — PIN sign-in and the kiosk binding
/// sit outside the auth stack by design.
///
/// Each seam is named for which side of the auth stack it lands on (see
/// `SurfaceSeams`), so a route cannot change sides by argument order: `public`
/// is the only one outside auth.
///
/// `oxy-api-custom-apps` is the second surface on the public seam: its health,
/// logs and client-error reads authenticate inline, before they look the app up,
/// and read the build store and ClickHouse, which every replica has. Its
/// `/customer-apps/{*path}` route rides `outer`, with its declarations in
/// `outer_decls` (absolute, because that router sits beside `/api`). On
/// `admin` (the admin split, PR 2): the staff console's CI publish tokens,
/// `/app-publish-tokens*`, behind `PlatformApps`, declaring nothing (the
/// console's `/{*rest}` FleetOk wildcard places them, as before).
///
/// `oxy-api-notifications` rides the api seam at the `/api` root, where
/// `build_global_routes` had its routes, and declares every one FleetOk
/// (Postgres and the org's store only). Its two queue kinds join `tasks`, and its
/// boot hooks — the inline transport, the run observers, push, the settings
/// reconcile — ride `notifications`, without which `serve` and `worker` refuse
/// to start.
///
/// `oxy-api-airway` rides the workspace seam and declares its five routes
/// FleetOk: the report upload (an S3 write that must not need the ide), the
/// two `/airway-pipelines` reads (the compile boundary and a static list), and
/// the two single-flight lease routes (one SELECT, one DELETE in Postgres).
///
/// `oxy-api-slack` fills two seams, as frontline does: Slack's webhooks, the
/// OAuth callback and the account-link page ride `public` (no user credential:
/// each webhook is signature-verified in its handler), and the org's install
/// and installation routes ride `api` with the org middlewares re-applied. All
/// seven are FleetOk (Postgres and Slack's API).
///
/// `oxy-api-world-model` fills two seams: the live feed, foot traffic,
/// competitors, and (since 2026-11-15, out of `oxy-app`'s `api/video.rs`) the
/// UniFi camera list and the weather tiles and readings ride `workspace` (all
/// FleetOk: Postgres and the vendors' APIs), and the same seven plus the LLM
/// proxy ride `external`, the curated `/external/api` tree standalone apps call
/// with an API key. Its tailer, reaper and camera bridge join `background`.
/// Its customer-app twins and (since 2026-11-15, out of `oxy-app`'s public
/// tree) the Toast order webhook ride `public`, the webhook declared `*`
/// FleetOk as `route_fleet` declared it (the compile boundary and Postgres).
///
/// `oxy-api-metric-monitoring` (2026-11-15) fills two seams too: the metric
/// tree, `/semantic/monitors` and the anomaly inbox ride `workspace` (all
/// seventeen FleetOk: the compile boundary, Postgres and the warehouses), and
/// the inbox again rides `external`, where the SDK's `client.anomalies.*` calls
/// it. Its anomaly handlers extract the `AgenticState` extension this crate
/// layers over both seams.
///
/// `oxy-api-observability` (2026-11-15) fills `workspace` only: the
/// tenant-facing observability console's `/traces`, `/metrics` and
/// `/execution-analytics` nests (six declarations, all FleetOk: the
/// ClickHouse product store every replica reaches alike).
///
/// `oxy-api-automations` (2026-11-15) fills `workspace` only: the
/// compile-boundary automation listing (`/procedures`, `/automations`) and
/// `/agentic-schedules` (eight declarations, all FleetOk: the compile boundary
/// and Postgres). Its schedule handlers extract the `AgenticState` extension
/// this crate layers over the workspace seam, and run-now takes the
/// `PlatformContext` the workspace middleware inserts.
///
/// `oxy-api-workspace-config` (2026-11-15) fills `workspace` only: the
/// workspace's own configured resources, `/databases` (the list, a database's
/// schema, the onboarding inspection) and `/secrets` (nine declarations, all
/// FleetOk: the in-memory config, the warehouses and Postgres). Each write
/// takes `WorkspaceAdmin` in its handler, as before.
///
/// `oxy-api-identity` fills six seams, each where its routes sat in
/// `oxy-app`'s router: the caller's tokens, the session's halves of `oxyc
/// login`, `/logout`, the mobile sign-in approval and an org's API access ride
/// `api` (the org half re-applies `org_middleware` + the subscription guard);
/// signing in, `GET /user`, the CLI's halves, the ticket redeem, the OIDC
/// exchange and leak reports ride `public`; the legacy `/api-keys` and the
/// workspace's `/api-tokens` ride `workspace`; the two token sections ride
/// `admin`; the legacy key operations and `GET /user` ride `openapi`; the
/// token sweep rides `global_ticks`. Every route is FleetOk (Postgres only).
///
/// `oxy-api-operating-graph` rides `api` in two halves, each where it sat in
/// `build_global_routes`: `/work` at the root (no org gate: a frontline worker
/// holds no org membership) and the locations, positions and assignments under
/// `/orgs/{org_id}` with `org_middleware` + the subscription guard re-applied.
/// All nine paths are declared FleetOk (Postgres only).
///
/// `oxy-api-threads` (2026-11-16) fills `api`, `workspace`, `openapi`,
/// `background` and `admin`. Org chat rides `api` at the root, where
/// `build_global_routes` had it (five paths, all FleetOk, the SSE stream
/// included: a fan-out over Postgres), and its delivery listener — the
/// `LISTEN` connection that wakes this replica's streams for a message
/// another replica took — rides `background` (`chat_delivery_listener`,
/// which `serve` refuses to boot without). On `workspace`: a
/// workspace's threads, the run logs, the agent listing and builder
/// availability, and a run's artifacts, chart files and result files (ten
/// declarations, all FleetOk: Postgres and the S3-mirrored runtime
/// artifacts), with the nine thread and agent operations the curated
/// document has always listed. On `admin` (the admin split): the staff
/// console's Explorer, `/explorer/threads` and `/explorer/runs` (two
/// declarations, FleetOk: Postgres), behind `PlatformExplorer`.
///
/// `oxy-api-workspaces` (2026-11-16) fills `api`, `workspace` and
/// `openapi`. On `api`: an org's workspace list, delete and rename under
/// `/orgs/{org_id}`, with `org_middleware` + the subscription guard
/// re-applied as `build_org_routes` wrapped them (three paths, FleetOk, cloud
/// only like the global tree). On `workspace`: a workspace's `/meta`, its
/// members and role overrides, the Oxy-staff lockdown and the app
/// integrations (five declarations, FleetOk), with the two operations the
/// curated document has always listed (`GET /workspaces/{workspace_id}`, `GET
/// /orgs/{org_id}/workspaces`).
///
/// `oxy-api-semantic` (2026-11-16) fills `workspace`, `external` and
/// `openapi`. On `workspace`: the Dev Portal's SQL execute (`/sql/{pathb64}`,
/// `/sql/query`), the semantic explorer (`/semantic`, `/semantic/compile`, a
/// topic's and a view's details), `/semantic/objects`, `/context-graph/sources`,
/// the compile boundary's revision reads (`/revision`, `/revision/files`,
/// `/revision/files/content`) and the pre-aggregation status and rebuild
/// (thirteen declarations, `*` FleetOk as `route_fleet` wrote them: the compile
/// boundary first, the warehouses, Postgres and the blob bucket). On
/// `external`: `/sql/query`, `/semantic` and `/semantic/compile`, the three
/// the curated API-key tree has always served. On `openapi`: `POST
/// /{workspace_id}/sql/query`, the data-plane operation `oxyc schema` answers.
///
/// `oxy-api-previews` (2026-11-14) fills its own seam, `previews`: the
/// `/{workspace_id}/previews` routes, beside the workspace tree behind the
/// access check alone, all FleetOk. Its four task kinds join `tasks` and the
/// TTL sweep joins `background`; `serve` refuses to start without them. (The
/// held runs' per-pass sweep is rows only and stays below, in `previews::queue`.)
///
/// `oxy-api-experiment` rides the workspace seam too (experiments and declared
/// worlds under `/{workspace_id}`), declaring write-back apply IdeOnly (it creates
/// a git worktree in the working copy). Its five task kinds join the custom-app
/// ones in `tasks`, and its tick — the one piece of singleton work the global
/// driver runs for it each pass — is the first `global_ticks` entry.
pub fn surface_seams() -> SurfaceSeams {
    SurfaceSeams {
        api: SurfaceSeam {
            routes: api_seam_routes(),
            decls: oxy_api_tenancy::onboarding::route_roles()
                .iter()
                .chain(oxy_api_tenancy::route_roles())
                .chain(oxy_api_tenancy::partner_console::route_roles())
                .chain(oxy_api_documents::route_roles())
                .chain(oxy_api_frontline::route_roles())
                .chain(oxy_api_identity::route_roles())
                .chain(oxy_api_operating_graph::route_roles())
                .chain(oxy_api_notifications::route_roles())
                .chain(oxy_api_custom_apps::route_roles())
                .chain(oxy_api_slack::route_roles())
                .chain(oxy_api_threads::route_roles())
                .chain(oxy_api_workspaces::route_roles())
                .chain(oxy_api_billing::route_roles())
                .chain(oxy_api_admin::route_roles())
                .copied()
                .collect(),
        },
        workspace: SurfaceSeam {
            routes: workspace_seam_routes(),
            decls: oxy_api_tenancy::onboarding::workspace_route_roles()
                .iter()
                .chain(oxy_api_tenancy::workspace_route_roles())
                .chain(oxy_api_airway::route_roles())
                .chain(oxy_api_custom_apps::workspace_route_roles())
                .chain(oxy_api_experiment::workspace_route_roles())
                .chain(oxy_api_world_model::workspace_route_roles())
                .chain(oxy_api_metric_monitoring::workspace_route_roles())
                .chain(oxy_api_observability::workspace_route_roles())
                .chain(oxy_api_automations::workspace_route_roles())
                .chain(oxy_api_workspace_config::workspace_route_roles())
                .chain(oxy_api_previews::workspace_route_roles())
                .chain(oxy_api_identity::workspace_route_roles())
                .chain(oxy_api_threads::workspace_route_roles())
                .chain(oxy_api_workspaces::workspace_route_roles())
                .chain(oxy_api_semantic::workspace_route_roles())
                .chain(oxy_api_eval::workspace_route_roles())
                .copied()
                .collect(),
        },
        // Workspace previews, beside the workspace tree behind the access
        // check alone.
        previews: SurfaceSeam {
            routes: oxy_api_previews::routes(),
            decls: oxy_api_previews::route_roles().to_vec(),
        },
        public: SurfaceSeam {
            routes: public_seam_routes(),
            decls: core_routes::public_route_roles()
                .iter()
                .chain(oxy_api_frontline::public_route_roles())
                .chain(oxy_api_identity::public_route_roles())
                .chain(oxy_api_custom_apps::public_route_roles())
                .chain(oxy_api_slack::public_route_roles())
                .chain(oxy_api_world_model::public_route_roles())
                .chain(oxy_api_github::public_route_roles())
                .chain(oxy_api_billing::public_route_roles())
                .chain(oxy_api_workspace_config::public_route_roles())
                .copied()
                .collect(),
        },
        // Routes only: the curated external tree carries no declarations.
        external: external_seam_routes(),
        // `/api/_catalog`, with the route table it serves bound in: the table
        // is generated by `oxy-route-catalog`'s build script, and the
        // handler is this binary's (`core_routes`). Passed, not global: a
        // composition that dropped it would serve no catalog at all.
        catalog: SurfaceSeam {
            routes: core_routes::catalog_routes(oxy_route_catalog::catalog()),
            decls: core_routes::catalog_route_roles().to_vec(),
        },
        // Staff-console sections, each behind the capability it names.
        // `oxy-api-billing`'s: the weekly custom-app usage report
        // (`/admin/usage-report*`, `PlatformOperate`) and who gets it
        // (`/admin/usage-report/recipients*`, `PlatformGrants`) since admin
        // split PR 8b, and the Billing queue (`/admin/orgs`,
        // `/admin/billing/prices`, `/admin/orgs/{org_id}/billing/*`,
        // `PlatformOwnerOnly`: the strict owner guard) since PR 9.
        //
        // `oxy-api-billing`'s FIRST: its Billing queue mounts `GET /orgs`, and
        // tenancy's org console `POST /orgs`. axum merges the two into one
        // method router whose wrong-verb 405 is the later router's, under that
        // router's guard; in-tree `billing` merged before every extracted
        // section, so a wrong verb on `/admin/orgs` answered behind tenancy's
        // `PlatformOrgs`, not the owner guard. Its other two sections share no
        // path, so their place does not matter.
        admin: oxy_api_billing::admin_sections()
            .into_iter()
            .chain(oxy_api_tenancy::admin_sections())
            .chain(oxy_api_workspace_health::admin_sections())
            .chain(oxy_api_airway::admin_sections())
            .chain(oxy_api_identity::admin_sections())
            .chain(oxy_api_observability::admin_sections())
            .chain(oxy_api_threads::admin_sections())
            .chain(oxy_api_custom_apps::admin_sections())
            // The platform audit log (`/admin/audit*`, `PlatformAudit`) and
            // the grant table (`/admin/app-admins*`, `PlatformGrants`),
            // `oxy-api-admin`'s since 2026-11-20, and the feature-flag console
            // (`/admin/feature-flags*`, `PlatformOperate`) since 2026-11-21.
            .chain(oxy_api_admin::admin_sections())
            .collect(),
        // Staff-console trees nested beside `/admin`, each behind the staff
        // door, the refusal while acting and the capability it names, in
        // the order `oxy-app` nested them: the task fleet's console
        // (`/admin/internal-jobs`, `oxy-api-admin`'s), then the compile
        // console (`/admin/compiles`, `oxy-api-previews`').
        admin_nests: oxy_api_admin::admin_nests()
            .into_iter()
            .chain(oxy_api_previews::admin_nests())
            .collect(),
        // Operations the surfaces document, merged into the served OpenAPI
        // document (`oxyc schema` reads it).
        openapi: vec![
            core_routes::openapi(),
            oxy_api_tenancy::openapi(),
            oxy_api_identity::openapi(),
            oxy_api_custom_apps::openapi(),
            oxy_api_threads::openapi(),
            oxy_api_workspaces::openapi(),
            oxy_api_semantic::openapi(),
        ],
        // The custom-app task kinds a worker runs, the background jobs a serve
        // process starts, and the outer `/customer-apps/{*path}` mount. The task
        // executors and the sandbox maintenance job are `oxy-api-custom-apps`'
        // (they were `oxy-app`'s until its second lift); the sweeps and usage
        // jobs are in `oxy-custom-apps`, and the mount is `oxy-api-custom-apps`'s.
        // The drivers name none of it, and refuse to start if any of it is
        // missing here.
        tasks: custom_apps_seams::tasks()
            .into_iter()
            .chain(oxy_api_experiment::tasks())
            .chain(oxy_api_notifications::tasks())
            .chain(oxy_api_workspace_health::tasks())
            // Insights delivery, queued by a monitor scan's `notify:` block
            // (in-tree in `oxy-app` until 2026-11-21).
            .chain(oxy_api_metric_monitoring::tasks())
            .chain(oxy_api_previews::tasks())
            .chain(oxy_api_eval::tasks())
            .collect(),
        // Chat's delivery listener first: `new_agentic_state` started it
        // ahead of every other listener, before it moved (2026-11-16).
        background: oxy_api_threads::background()
            .into_iter()
            .chain(custom_apps_seams::background())
            .chain(oxy_api_world_model::background())
            .chain(oxy_api_previews::background())
            .collect(),
        // The jobs whose answer is global rather than per replica: started by
        // every process that runs singleton work (each `oxy worker`, an
        // `all`/`ide` serve), each claiming its interval in `fleet_job_claims`.
        fleet_jobs: custom_apps_seams::fleet_jobs(),
        global_ticks: oxy_api_experiment::global_ticks()
            .into_iter()
            .chain(oxy_api_identity::global_ticks())
            // The weekly custom-app usage report, once per loop of the
            // driver (`TickPass::EveryLoop`), where `oxy-app` called it inline.
            .chain(oxy_api_billing::global_ticks())
            .collect(),
        outer: Some(custom_apps_seams::outer),
        outer_decls: oxy_api_custom_apps::serve::serve_dispatch_roles().to_vec(),
        function_runtime: Some(custom_apps_seams::function_runtime()),
        // The functions preflight needs the V8 runtime; a V8-off build skips it.
        #[cfg(feature = "custom-app-functions")]
        post_migrate: Some(custom_apps_seams::post_migrate()),
        #[cfg(not(feature = "custom-app-functions"))]
        post_migrate: None,
        example_apps: Some(oxy_api_custom_apps::seed::example_apps()),
        notifications: Some(oxy_api_notifications::seam()),
    }
}

/// Who owns which `oxy` subcommand. The parser is `oxy-cli-shim`'s and knows
/// no command. This binary owns the long-lived ones (`start`, `serve`,
/// `worker`), listed first; [`cli`] owns the one-shot rest. The hidden
/// `oxy dev` group (`seed`, dev `compile`, and `test` from `oxy-api-eval`) and
/// the one-release `oxy seed` / `oxy test` aliases exist only with the `dev-tools` feature,
/// which the release build leaves off (`dev.rs`). `oxy airway` was
/// `oxy-api-airway`'s until it moved to `oxyc airway` on 2026-11-21.
pub fn cli_owners() -> Vec<oxy_cli_shim::Owner<SurfaceSeams>> {
    let mut owners = vec![oxy_cli_shim::Owner::of::<commands::ServerCommand>(
        |command, seams| Box::pin(commands::run(command, seams)),
    )];
    #[cfg(feature = "dev-tools")]
    owners.extend([
        oxy_cli_shim::Owner::of::<dev::DevGroup>(|command, seams| {
            Box::pin(dev::run(command, seams))
        }),
        oxy_cli_shim::Owner::of::<dev::Renamed>(|command, seams| {
            Box::pin(dev::run_renamed(command, seams))
        }),
    ]);
    owners.push(oxy_cli_shim::Owner::of::<cli::SubCommand>(
        |command, seams| Box::pin(cli::run(command, seams)),
    ));
    owners
}

#[cfg(test)]
mod tests;
