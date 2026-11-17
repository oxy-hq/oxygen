// A binary is its own crate root, so the `recursion_limit` in lib.rs does not
// apply here. Laying out the futures reached from `oxy_app::cli::run` exceeds rustc's
// default query depth since SeaORM 2.0 deepened its query types.
#![recursion_limit = "256"]

// Dev-only dynamic linking (see `oxy-app-dylib` + `just dev-backend-dyn`).
// Forcing the dylib into the link (with `-C prefer-dynamic`) makes oxy-app's
// symbols — and its ~1.4 GB of static deps — resolve dynamically from
// liboxy_app_dylib.dylib instead of being re-linked into the binary every edit.
// `as _` because we import it purely for the link edge, not to use its items.
#[cfg(feature = "dev-dynamic")]
extern crate oxy_app_dylib as _;

#[cfg(test)]
mod catalog_tests;
mod commands;
mod core_routes;
mod custom_apps_seams;
#[cfg(test)]
mod healthcheck_tests;
mod logging;
#[cfg(test)]
mod recursion_limit_guard;
#[cfg(test)]
mod served_router_tests;

use std::process::exit;

use dotenv::dotenv;
use human_panic::Metadata;
use human_panic::setup_panic;
use oxy::sentry_config;
use oxy::theme::StyledText;
use oxy_app::observability_boot;
use oxy_app::server::router::{SurfaceSeam, SurfaceSeams};
use oxy_telemetry::otel::OtelConfig;
use std::env;

/// The long-lived server command this invocation runs, if any. Read before
/// clap runs because the subscriber must exist before anything logs, and
/// matched anywhere in argv rather than as "the first non-flag": a global
/// value-taking flag (`oxy --output text serve`) would otherwise make the
/// value look like the command.
///
/// Only these three get the OpenTelemetry layer. A one-shot command
/// (`oxy run`, `oxy validate`) has no reader for its trace ids, and in a shell
/// that happens to carry `OTEL_EXPORTER_OTLP_ENDPOINT` — `.env` is
/// auto-loaded — it would pay the exporter's flush at every exit.
///
/// Only these three get the product `SpanCollectorLayer` either, and for a
/// harder reason: they are the commands that call
/// `observability_boot::finalize()`, the only thing that drains its unbounded
/// channel. Anything else would hold every span it closes until it exits.
/// `commands::entry_point_tests` reads this list, so a
/// command added here without a `finalize` fails the build.
fn server_command(args: &[String]) -> Option<&str> {
    args.iter()
        .skip(1)
        .map(String::as_str)
        .find(|a| matches!(*a, "serve" | "start" | "worker"))
}

/// Raise the process's open-file-descriptor soft limit at startup.
///
/// macOS ships a default soft `RLIMIT_NOFILE` of 256. A busy oxy instance
/// (the warehouse + LLM HTTP clients, the embedded Postgres pool, and many
/// concurrent SSE streams from data-app dashboards) blows past that and the
/// server stops accepting connections with
/// `axum::serve::listener: accept error: Too many open files (os error 24)`.
/// We bump the soft limit toward the hard cap (clamped to a sane target that
/// stays under macOS's `kern.maxfilesperproc`) so the server has headroom
/// regardless of the shell/launcher it was started from. Best-effort: any
/// failure is logged and the process continues with the inherited limit.
#[cfg(unix)]
fn raise_fd_limit() {
    // 65536 is ample headroom for a single instance and stays well under the
    // macOS per-process kernel cap on default systems.
    const DESIRED: libc::rlim_t = 65_536;
    // SAFETY: plain libc rlimit syscalls on a zeroed struct; single-threaded
    // here (before the Tokio runtime is built).
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        let target = if lim.rlim_max == libc::RLIM_INFINITY {
            DESIRED
        } else {
            std::cmp::min(DESIRED, lim.rlim_max)
        };
        if lim.rlim_cur >= target {
            return; // already sufficient
        }
        let prev = lim.rlim_cur;
        lim.rlim_cur = target;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &lim) == 0 {
            tracing::debug!(from = prev, to = target, "raised RLIMIT_NOFILE soft limit");
        } else {
            tracing::warn!(
                current = prev,
                "could not raise RLIMIT_NOFILE; if you hit \"Too many open files\", \
                 raise it manually with `ulimit -n 65536` before starting oxy"
            );
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

type SeamRouter = axum::Router<oxy_app::server::router::AppState>;

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
        // The environment planes writes pin a semantic branch through a port
        // `oxy-app` implements (a surface may not depend on `oxy-app`). The
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
/// inside oxy-app can see across the crate line to stop them.
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
/// `oxy-api-airway` rides the workspace seam and declares its three routes
/// FleetOk: the report upload (an S3 write that must not need the ide) and the
/// two `/airway-pipelines` reads (the compile boundary and a static list).
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
/// it. Its anomaly handlers extract the `AgenticState` extension `oxy-app`
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
/// `oxy-app` layers over the workspace seam, and run-now takes the
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
/// which `oxy-app` refuses to boot without). On `workspace`: a
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
/// TTL sweep joins `background`; `oxy-app` refuses to start without them. (The
/// held runs' per-pass sweep is rows only and stays below, in `previews::queue`.)
///
/// `oxy-api-experiment` rides the workspace seam too (experiments and declared
/// worlds under `/{workspace_id}`), declaring write-back apply IdeOnly (it creates
/// a git worktree in the working copy). Its five task kinds join the custom-app
/// ones in `tasks`, and its tick — the one piece of singleton work the global
/// driver runs for it each pass — is the first `global_ticks` entry.
fn surface_seams() -> SurfaceSeams {
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
                .copied()
                .collect(),
        },
        // Routes only: the curated external tree carries no declarations.
        external: external_seam_routes(),
        // `/api/_catalog`, with the route table it serves bound in: the table
        // is generated above oxy-app (see `oxy-route-catalog`), and the
        // handler is this binary's (`core_routes`). Passed, not global: a
        // composition that dropped it would serve no catalog at all.
        catalog: SurfaceSeam {
            routes: core_routes::catalog_routes(oxy_route_catalog::catalog()),
            decls: core_routes::catalog_route_roles().to_vec(),
        },
        // Staff-console sections, each behind the capability it names.
        admin: oxy_api_tenancy::admin_sections()
            .into_iter()
            .chain(oxy_api_workspace_health::admin_sections())
            .chain(oxy_api_airway::admin_sections())
            .chain(oxy_api_identity::admin_sections())
            .chain(oxy_api_observability::admin_sections())
            .chain(oxy_api_threads::admin_sections())
            .chain(oxy_api_custom_apps::admin_sections())
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
        // executors and the sandbox maintenance job are still in `oxy-app` (the
        // second lift moves them); the sweeps and usage jobs are in
        // `oxy-custom-apps`, and the mount is `oxy-api-custom-apps`'s. `oxy-app`
        // no longer names any of it, and refuses to start if any of it is missing
        // here.
        tasks: custom_apps_seams::tasks()
            .into_iter()
            .chain(oxy_api_experiment::tasks())
            .chain(oxy_api_notifications::tasks())
            .chain(oxy_api_workspace_health::tasks())
            .chain(oxy_api_previews::tasks())
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
            .collect(),
        outer: Some(custom_apps_seams::outer),
        outer_decls: oxy_api_custom_apps::serve::serve_dispatch_roles().to_vec(),
        function_runtime: Some(custom_apps_seams::function_runtime()),
        post_migrate: Some(custom_apps_seams::post_migrate()),
        example_apps: Some(oxy_api_custom_apps::seed::example_apps()),
        notifications: Some(oxy_api_notifications::seam()),
    }
}

fn main() {
    dotenv().ok();
    let _sentry_guard = sentry_config::init_sentry(oxy_app::BUILD_SHA);
    if _sentry_guard.is_none() {
        setup_panic!(
            Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                .authors("Robert Yi <robert@oxygen-hq.com>") // temporarily using Robert email here, TODO: replace by support email
                .homepage("github.com/oxy-hq/oxygen")
                .support(
                    "- For support, please email robert@oxygen-hq.com or contact us directly via Github."
                )
        );
    }

    // Parse args early to check for flags
    let args: Vec<String> = env::args().collect();

    // Check if --enterprise flag is present (gates the observability UI/routes)
    let enterprise_enabled = args.iter().any(|a| a == "--enterprise");

    // Observability is opt-in everywhere — including `--local`. ClickHouse is
    // the sole backend, so there is no embedded store to default to: enabling
    // it implies a running ClickHouse (`oxy start` boots the container when
    // the var is set). With `--enterprise` but no backend, we warn and run
    // with observability disabled — no data is recorded and the UI surfaces a
    // "not configured" banner.
    let observability_enabled = env::var_os("OXY_OBSERVABILITY_BACKEND").is_some();
    if enterprise_enabled && !observability_enabled {
        eprintln!(
            "{}",
            "Observability disabled: OXY_OBSERVABILITY_BACKEND is not set. \
             Set it to clickhouse — with OXY_CLICKHOUSE_URL pointing at your \
             instance, or under `oxy start`, which boots one — to record traces."
                .text()
        );
    }

    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // DO NOT USE #[tokio::main]
    // https://docs.sentry.io/platforms/rust/
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    // A debug build's `async fn` poll frame reserves room for every future it
    // awaits, so a request's stack is the sum of its whole middleware chain's
    // frames. A ClickHouse `POST /{ws}/sql/query` measured 1.3 MB of it on macOS
    // before reaching the connector, and overflowed the default 2 MiB worker
    // stack on Linux in the custom-app canary. Release builds are far below
    // that and keep the default.
    #[cfg(debug_assertions)]
    runtime.thread_stack_size(8 * 1024 * 1024);
    runtime.build().unwrap().block_on(async {
        // Install the subscriber: Sentry + stderr/file + (for a server
        // command with OXY_OBSERVABILITY_BACKEND set) the
        // SpanCollectorLayer + (if an OTLP endpoint is configured) the
        // OpenTelemetry exporters. The observability *store* isn't wired
        // yet — the `oxy start` path boots its ClickHouse container and
        // only then are `OXY_CLICKHOUSE_*` set; `observability_boot::
        // finalize()` is called from `serve.rs` once that endpoint is
        // available, and from `worker.rs` at boot. The OTel resource needs
        // the fleet role now, before clap has parsed anything, so it is
        // read from OXY_ROLE + argv.
        let command = server_command(&args);
        let role =
            oxy_telemetry::resource::role_hint(command, env::var("OXY_ROLE").ok().as_deref());
        let mut otel = OtelConfig::from_env(role);
        if command.is_none() {
            otel.sdk_disabled = true;
        }
        // Only a command that drains the span channel may fill it — see
        // `server_command`.
        let collect_product_spans = observability_enabled && command.is_some();
        let telemetry_problems = logging::init(collect_product_spans, &otel, command.is_some());
        for problem in telemetry_problems {
            tracing::warn!(%problem, "platform telemetry degraded");
        }
        if otel.export_enabled() {
            tracing::info!(
                traces_endpoint = otel.traces_endpoint.as_deref().unwrap_or_default(),
                logs_endpoint = otel.logs_endpoint.as_deref().unwrap_or_default(),
                traces = otel.traces_exported(),
                logs = otel.logs_exported(),
                filter = %otel.filter,
                role = role.unwrap_or("none"),
                "OTLP export enabled"
            );
        }

        // Metrics, installed after the subscriber so its own diagnostics
        // are logged rather than dropped. Separate from `OtelConfig`
        // because the two signals have independent switches: the
        // Prometheus reader is always on (a pull endpoint costs nothing
        // until scraped) while OTLP push is opt-in, and a one-shot CLI
        // command gets neither.
        let mut metrics_config = oxy_telemetry::metrics::MetricsConfig::from_env();
        if command.is_none() {
            metrics_config.sdk_disabled = true;
        }
        for problem in
            oxy_telemetry::metrics::init(&metrics_config, oxy_telemetry::resource::build(role))
        {
            tracing::warn!(%problem, "platform metrics degraded");
        }
        if metrics_config.otlp_exported() {
            tracing::info!(
                endpoint = metrics_config.otlp_endpoint.as_deref().unwrap_or_default(),
                interval_secs = metrics_config.export_interval.as_secs(),
                "OTLP metrics export enabled"
            );
        }

        // Give the server enough file-descriptor headroom before it binds
        // listeners / boots the embedded Postgres — macOS defaults to a
        // soft NOFILE of 256, which busy instances exhaust (EMFILE).
        raise_fd_limit();

        // The parser is `oxy-cli-shim`'s and knows no command. This binary
        // owns the long-lived ones (`start`, `serve`, `worker`), listed first;
        // oxy-app owns most of the one-shot rest, and a context that has left
        // oxy-app brings its own (`oxy airway`, from `oxy-api-airway`).
        let owners = vec![
            oxy_cli_shim::Owner::of::<commands::ServerCommand>(|command, seams| {
                Box::pin(commands::run(command, seams))
            }),
            oxy_cli_shim::Owner::of::<oxy_app::cli::SubCommand>(|command, seams| {
                Box::pin(oxy_app::cli::run(command, seams))
            }),
            oxy_cli_shim::Owner::of::<oxy_api_airway::Command>(|command, _seams| {
                Box::pin(oxy_api_airway::run(command))
            }),
        ];
        let exit_code =
            match oxy_cli_shim::run(owners, surface_seams(), oxy_app::cli::prepare).await {
                Ok(_) => 0,
                Err(e) => {
                    tracing::error!(error = %e, "Application error");
                    sentry_config::capture_error_with_context(&*e, "CLI execution failed");
                    eprintln!("{}", format!("{e}").error());
                    1
                }
            };

        observability_boot::shutdown().await;

        // Metrics before the trace/log exporters: the meter provider's own
        // flush can emit internal logs, and those should still have
        // somewhere to go.
        match tokio::task::spawn_blocking(oxy_telemetry::metrics::shutdown).await {
            Ok(problems) => {
                for problem in problems {
                    eprintln!("oxy: {problem}");
                }
            }
            Err(e) => eprintln!("oxy: metrics shutdown did not complete: {e}"),
        }

        // Last: flush the OTLP exporters. Blocking, bounded, and off the
        // async thread so a slow collector cannot wedge the runtime.
        match tokio::task::spawn_blocking(oxy_telemetry::otel::shutdown).await {
            Ok(problems) => {
                for problem in problems {
                    eprintln!("oxy: {problem}");
                }
            }
            Err(e) => eprintln!("oxy: OTLP exporter shutdown did not complete: {e}"),
        }

        // Truly last: point fd 2 back at the real stderr and let the
        // capture thread write out whatever stray lines are still queued
        // (the `eprintln!`s just above included).
        oxy_telemetry::stderr_capture::finish(std::time::Duration::from_secs(2));

        if exit_code != 0 {
            exit(exit_code);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
            oxy_app::server::router::require_task_kinds(&seams.tasks),
            Ok(()),
            "a kind registered twice would shadow one executor with another"
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
        let doc = oxy_app::server::router::build_openapi_doc(surface_seams().openapi).await;
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
        let doc = oxy_app::server::router::build_openapi_doc(surface_seams().openapi).await;
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

        let doc = oxy_app::server::router::build_openapi_doc(surface_seams().openapi).await;
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
        let doc = oxy_app::server::router::build_openapi_doc(surface_seams().openapi).await;
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
        let doc = oxy_app::server::router::build_openapi_doc(surface_seams().openapi).await;
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
}
