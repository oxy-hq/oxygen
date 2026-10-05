// A binary is its own crate root, so the `recursion_limit` in lib.rs does not
// apply here. Laying out the futures reached from `cli()` exceeds rustc's
// default query depth since SeaORM 2.0 deepened its query types.
#![recursion_limit = "256"]

// Dev-only dynamic linking (see `oxy-app-dylib` + `just dev-backend-dyn`).
// Forcing the dylib into the link (with `-C prefer-dynamic`) makes oxy-app's
// symbols — and its ~1.4 GB of static deps — resolve dynamically from
// liboxy_app_dylib.dylib instead of being re-linked into the binary every edit.
// `as _` because we import it purely for the link edge, not to use its items.
#[cfg(feature = "dev-dynamic")]
extern crate oxy_app_dylib as _;

mod logging;
#[cfg(test)]
mod served_router_tests;

use std::process::exit;

use dotenv::dotenv;
use human_panic::Metadata;
use human_panic::setup_panic;
use oxy::sentry_config;
use oxy::theme::StyledText;
use oxy_app::cli::commands::cli;
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
/// `oxy-app`'s `observability_boot::entry_point_tests` reads this list, so a
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
        .merge(oxy_api_tenancy::partner_console::routes())
        .merge(oxy_api_tenancy::onboarding::routes())
        .merge(oxy_api_documents::routes())
        .merge(oxy_api_frontline::routes())
}

/// The sibling crates merged inside the `/{workspace_id}` nest.
fn workspace_seam_routes() -> SeamRouter {
    oxy_api_tenancy::onboarding::workspace_routes().merge(oxy_api_source_upload::routes())
}

/// Every surface this composition root mounts, by seam. `cli` forwards it into
/// `serve`'s `api_router`. A function rather than inline in `main` for the
/// reason `api_seam_routes` is one: `served_router_tests` builds the router from
/// the SAME value boot hands over, so "cloud mode serves `/orgs`" is checked
/// against the real composition and not a copy of it.
///
/// Roles travel WITH the routes. `oxy-api-github` mounts a Postgres-only
/// surface, so the FleetOk default is the truth and it declares nothing.
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
/// `oxy-api-source-upload` rides the workspace seam and declares its one route
/// FleetOk: an S3 write that must not need the ide.
fn surface_seams() -> SurfaceSeams {
    SurfaceSeams {
        api: SurfaceSeam {
            routes: api_seam_routes(),
            decls: oxy_api_tenancy::onboarding::route_roles()
                .iter()
                .chain(oxy_api_tenancy::partner_console::route_roles())
                .chain(oxy_api_documents::route_roles())
                .chain(oxy_api_frontline::route_roles())
                .copied()
                .collect(),
        },
        workspace: SurfaceSeam {
            routes: workspace_seam_routes(),
            decls: oxy_api_tenancy::onboarding::workspace_route_roles()
                .iter()
                .chain(oxy_api_source_upload::route_roles())
                .copied()
                .collect(),
        },
        public: SurfaceSeam {
            routes: oxy_api_frontline::public_routes(),
            decls: oxy_api_frontline::public_route_roles().to_vec(),
        },
        // The route table `/api/_catalog` serves, generated above oxy-app (see
        // `oxy-route-catalog`). Passed, not global: a composition that dropped
        // it would serve an empty catalog.
        catalog: oxy_route_catalog::catalog(),
        // Staff-console sections, each behind the capability it names.
        admin: oxy_api_tenancy::admin_sections(),
        // Operations the surfaces document, merged into the served OpenAPI
        // document (`oxyc schema` reads it).
        openapi: vec![oxy_api_tenancy::openapi()],
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
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
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

            let exit_code = match cli(surface_seams()).await {
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

    /// `GET /orgs` is one of the lookups an agent needs a schema for (see
    /// `oxy-app`'s `the_agent_data_plane_is_documented`). Its handler moved to
    /// `oxy-api-tenancy`, so only the document the composition root assembles
    /// can show it — a dropped `openapi:` entry would lose it silently.
    #[tokio::test]
    async fn the_served_openapi_document_includes_the_surfaces() {
        let doc =
            oxy_app::server::router::build_openapi_doc(vec![oxy_api_tenancy::openapi()]).await;
        let orgs = doc
            .paths
            .paths
            .get("/orgs")
            .expect("GET /orgs is missing from the OpenAPI document");
        assert!(orgs.get.is_some(), "/orgs is documented but carries no get");
    }
}
