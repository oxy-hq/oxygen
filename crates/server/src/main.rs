//! The `oxy` binary's entry: argv, the Tokio runtime, and the process-wide
//! setup and teardown around one call into `oxy-server`'s library, which holds
//! everything else (`lib.rs`; decompose LT, 2026-11-23). Lines logged here
//! keep the target `oxy`, the binary's crate ident.

// The binary is its own crate root, so the `recursion_limit` in `lib.rs` does
// not apply here, and `main` still lays out the futures reached from
// `oxy_server::cli::run` (through `oxy_cli_shim::run`), which exceed rustc's
// default query depth since SeaORM 2.0 deepened its query types.
#![recursion_limit = "256"]

use std::process::exit;

use dotenv::dotenv;
use human_panic::Metadata;
use human_panic::setup_panic;
use oxy::theme::StyledText;
use oxy_server::{cli_owners, init_logging, shutdown_observability, surface_seams};
use oxy_telemetry::otel::OtelConfig;
use oxy_telemetry::sentry_config;
use std::env;

/// The long-lived server command this invocation runs, if any. Read before
/// clap runs because the subscriber must exist before anything logs, and
/// matched anywhere in argv rather than as "the first non-flag", so a flag
/// placed before the command cannot hide it.
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

fn main() {
    dotenv().ok();
    let _sentry_guard = sentry_config::init_sentry(oxy_server::BUILD_SHA);
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
        let telemetry_problems = init_logging(collect_product_spans, &otel, command.is_some());
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

        let exit_code = match oxy_cli_shim::run(
            cli_owners(),
            surface_seams(),
            oxy_server::cli::prepare,
        )
        .await
        {
            Ok(_) => 0,
            Err(e) => {
                tracing::error!(error = %e, "Application error");
                sentry_config::capture_error_with_context(&*e, "CLI execution failed");
                eprintln!("{}", format!("{e}").error());
                1
            }
        };

        shutdown_observability().await;

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
