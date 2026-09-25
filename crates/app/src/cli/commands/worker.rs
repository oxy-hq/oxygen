//! `oxy worker` — run an agentic worker process standalone.
//!
//! Counterpart to `oxy serve --no-workers`: HTTP frontends accept requests
//! and write to `agentic_task_queue`; one or more worker processes drain
//! the queue via the durable transport's `SELECT ... FOR UPDATE SKIP
//! LOCKED` claim path. The subcommand only adds the long-running process
//! that owns the orchestrator + driver loops — no HTTP surface beyond the
//! optional k8s probe port.
//!
//! **The worker drives runs (Phase 1).** It runs the same loops `oxy serve`
//! does — `server::router::recovery`'s latency worker and periodic
//! stranded-run tick — so scheduler-seeded and orphaned work executes on a
//! worker pod rather than on the node serving HTTP. It is an *additional*
//! driver: the request-time direct-drive still executes interactive submits
//! in-process on the node that accepted them, and removing that is Phase 2.
//! What the worker needs from its deployment to be useful (workspace paths on
//! disk, in particular) is in `internal-docs/worker-fleet.md`.
//!
//! See `internal-docs/worker-fleet.md` for full deployment guidance.
//!
//! ## Database connection
//!
//! Two auth modes are supported (controlled by `OXY_DATABASE_AUTH_MODE`):
//!
//! - **`password`** (default) — set `OXY_DATABASE_URL` to the full
//!   PostgreSQL connection string.
//! - **`iam`** — AWS RDS IAM auth; `OXY_DATABASE_URL` is **not** required.
//!   Set `OXY_DATABASE_HOST`, `OXY_DATABASE_NAME`, `OXY_DATABASE_USER`,
//!   `OXY_DATABASE_REGION`, and optionally `OXY_DATABASE_PORT` /
//!   `OXY_DATABASE_SSL_MODE` instead.
//!
//! Other env vars: `OXY_WORKER_MAX_INFLIGHT`,
//! `OXY_WORKER_RECOVERY_INTERVAL_SECS`, `OXY_WORKER_HEALTH_PORT`.

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use oxy::database::client::{DatabaseAuthMode, IamConfig};
use oxy::theme::StyledText;
use oxy_shared::errors::OxyError;
use tokio::signal;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::cli::commands::serve;
// Aliased: this module already has a `spawn_recovery` (the reaper pre-pass
// loop below), and the two are genuinely different jobs — one reaps stale
// queue rows, the other drives runs.
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::router::recovery::{StartupPass, spawn_recovery as spawn_run_drivers};
use crate::server::worker_health::{self, WorkerHealthState};
use crate::server::worker_runtime::WorkerRuntime;

/// Default cadence for the standalone-worker recovery loop. Same default the
/// in-process driver uses (`OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS`).
const DEFAULT_RECOVERY_INTERVAL_SECS: u64 = 30;

// The shutdown budget documented in `internal-docs/worker-fleet.md` (the
// `terminationGracePeriodSeconds` bullet's `reachable when` column, both
// "graceful shutdown takes…" troubleshooting rows) and in `metrics()`'s doc
// comment rest on ONE property: at the default recovery interval, `tick_once`
// short-circuits before touching the DB, so the 30s recovery drain is not
// reachable.
//
// It used to cover the 10s `release_queue_claims` too, because the transport
// `tick_once` built was the only thing in the process that minted
// `PROCESS_WORKER_ID`. That stopped being true when the worker started driving
// runs: the driver loops spawned in `run_worker` build a `DurableTransport` on
// the first run they pick up, at any interval. So the release is now
// unconditional and only the drain is gated — which is what the doc's table
// says, and why this assert still matters for one of the two terms rather than
// both.
//
// That property is `DEFAULT_RECOVERY_INTERVAL_SECS >= REAPER_INTERVAL`, and it
// currently holds at exact equality (30 >= 30) between two constants in
// DIFFERENT CRATES whose values were chosen for unrelated reasons — this one to
// match `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS`, that one to pace the reaper.
// Raise `REAPER_INTERVAL` to 60, or take this default to 15 for the reason its
// own comment gives, and every one of those documents becomes silently wrong
// with nothing failing.
//
// So it is checked rather than coincidental.
//
// Compared in NANOS rather than `as_secs()`, so the assert tests the same
// predicate the runtime does: `tick_once` compares whole `Duration`s
// (`interval >= REAPER_INTERVAL`). `as_secs()` floors, so a `REAPER_INTERVAL`
// of 30_500 ms would satisfy `30 >= 30` here while the runtime comparison went
// false — the assert would pass, the short-circuit would stop firing, and the
// docs would go silently wrong. That is the exact failure this exists to catch,
// so it must not be reachable through the assert's own units.
//
// If this fires, the fix is to update the docs named above, not to bend the
// constant back.
const _: () = assert!(
    Duration::from_secs(DEFAULT_RECOVERY_INTERVAL_SECS).as_nanos()
        >= agentic_runtime::background::REAPER_INTERVAL.as_nanos(),
    "DEFAULT_RECOVERY_INTERVAL_SECS must stay >= REAPER_INTERVAL. Update these \
     four before changing it: the terminationGracePeriodSeconds bullet's \
     `reachable when` table and both `graceful shutdown takes...` \
     troubleshooting rows in internal-docs/worker-fleet.md, and the metrics() \
     doc comment in crates/app/src/server/worker_metrics.rs. They all assume \
     tick_once short-circuits at the default interval"
);

/// The worst-case graceful shutdown the k8s recipe in
/// `internal-docs/worker-fleet.md` sizes `terminationGracePeriodSeconds`
/// against — all three terms, i.e. `--health-port` set AND a recovery interval
/// below `REAPER_INTERVAL`. The release runs after `drain_background` rather
/// than inside it, so the terms add.
///
/// Exists so the number the doc publishes is derived from the bounds rather
/// than transcribed from them. The gate above is checked; without this the
/// three MAGNITUDES were not, and changing any one of them would have left the
/// table, the `50` recommendation and the copy-paste manifest wrong with
/// nothing failing — the same enforcement-by-hope the gate assert replaced.
/// Summed in NANOS for the same reason the gate assert above compares nanos:
/// `as_secs()` floors, so summing truncated terms and then truncating the sum
/// would let any sub-second change slip through invisibly —
/// `HEALTH_JOIN_TIMEOUT = 5_500ms` still gives `5 + 30 + 10 = 45` while the
/// real bound is 45.5s. A derivation that floors is a transcription with extra
/// steps, which is exactly what this const exists not to be.
const WORST_CASE_SHUTDOWN: Duration = Duration::from_nanos(
    (RECOVERY_DRAIN_TIMEOUT.as_nanos()
        + HEALTH_JOIN_TIMEOUT.as_nanos()
        + RELEASE_CLAIMS_TIMEOUT.as_nanos()) as u64,
);

// If this fires, `internal-docs/worker-fleet.md` publishes a stale worst case
// and a `terminationGracePeriodSeconds` that no longer covers it. Fix the doc
// and the recommended TGP, not this line.
const _: () = assert!(
    WORST_CASE_SHUTDOWN.as_nanos() == Duration::from_secs(45).as_nanos(),
    "shutdown worst case changed. FOUR sites publish these magnitudes: the 45s \
     row and the 50s recommendation in internal-docs/worker-fleet.md's \
     terminationGracePeriodSeconds table; the metrics() doc comment in \
     crates/app/src/server/worker_metrics.rs; and the two troubleshooting rows \
     in worker-fleet.md that quote the drain log lines VERBATIM as grep \
     strings - those strings derive their numbers from these consts, so \
     changing one silently makes the runbook's grep miss. Update all four, \
     then update this assert"
);
const RECOVERY_INTERVAL_ENV: &str = "OXY_WORKER_RECOVERY_INTERVAL_SECS";
const HEALTH_PORT_ENV: &str = "OXY_WORKER_HEALTH_PORT";

/// Arguments for the `oxy worker` command.
#[derive(Parser, Debug, Clone)]
pub struct WorkerArgs {
    /// Enable enterprise features when running migrations.
    ///
    /// Matches the same `--enterprise` flag on `oxy serve`; tracked separately
    /// so migration code can branch without re-reading the serve args.
    #[clap(long, default_value_t = false)]
    pub enterprise: bool,

    /// Skip running migrations at startup.
    ///
    /// Useful when scaling a fleet behind a leader that already ran them, or
    /// when migrations are run as a separate `oxy migrate` job in CI/CD.
    /// Default OFF — the first standalone worker on a fresh deployment still
    /// runs migrations exactly like `oxy serve` does.
    #[clap(long, default_value_t = false)]
    pub skip_migrations: bool,

    /// Override the reaper pre-pass interval in seconds.
    ///
    /// Falls back to `OXY_WORKER_RECOVERY_INTERVAL_SECS`, then to 30s. At the
    /// default it is a no-op: the pass only fires below the background
    /// reaper's own cadence, so this is an incident-triage knob
    /// (`--recovery-interval-secs 5`), not the one that paces run driving.
    ///
    /// **Run driving is paced separately**, by
    /// `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS` (periodic stranded-run tick,
    /// default 30s) and `OXY_LATENCY_WORKER_INTERVAL_MS` (freshly-seeded
    /// Global runs, default 1s) — the same two knobs that pace the loops
    /// inside `oxy serve`, because it is the same code.
    #[clap(long)]
    pub recovery_interval_secs: Option<u64>,

    /// Bind a tiny server on this TCP port exposing `/healthz`, `/readyz` and
    /// `/metrics`. No Oxy application routes are mounted here.
    ///
    /// Leaving it unset costs more than the k8s probes: `/metrics` is where an
    /// HPA reads outstanding work (`oxy_queue_depth_queued` +
    /// `oxy_queue_depth_claimed`) against `oxy_worker_capacity` to size the
    /// fleet, so an unset port removes the scrape target too.
    ///
    /// Aggregation differs per metric — DB-sourced, process-local and
    /// per-replica gauges each take a different function, and the two halves of
    /// the HPA query are not the same one. Getting it wrong silently mis-sizes
    /// the fleet or silences an alert. The module header of
    /// `server/worker_metrics.rs` is the authority; read it before writing a
    /// query.
    ///
    /// Falls back to `OXY_WORKER_HEALTH_PORT`. Default OFF (no port bound).
    #[clap(long)]
    pub health_port: Option<u16>,
}

/// Entry point for `oxy worker`.
///
/// Bootstraps the orchestrator, logs config, then blocks on SIGINT/SIGTERM.
#[tracing::instrument(skip_all, fields(worker_id = tracing::field::Empty, version = tracing::field::Empty))]
pub async fn run_worker(args: WorkerArgs) -> Result<(), OxyError> {
    // The worker is a separate PROCESS, so `start_server_and_web_app`'s
    // registration does not reach it — and background work is where `notify`
    // will mostly be called from. Registering here keeps the boot log honest
    // about push in the process that will actually be sending it.
    if !crate::server::api::notifications::web_push::register_if_configured() {
        tracing::info!("no push transport configured — notifications are inbox-only");
    }

    // Same first line as `start_server_and_web_app`, and for a sharper reason
    // here. `current_process_role()` falls back to `Role::All` when nothing set
    // the `OnceLock`, so a standalone `oxy worker` declared itself the node that
    // owns the workspace files:
    //
    //   - `OxyProjectContext::context_root` branches on `Serve | Worker` and
    //     took the filesystem arm instead, globbing an absent working copy —
    //     the "no databases configured" failure its own comment names.
    //   - `process_owns_workspace_files()` was true, so the workspace
    //     middleware published the `WorkingCopy` extension on a pod with none.
    //   - `process_is_fs_writable()` was true, so a missing root read as
    //     "materializing" (503) rather than as the normal state of a worker.
    //
    // Only a deployment that already sets `OXY_ROLE=worker` changes behaviour —
    // which is the deployment that was being mis-described.
    // Defaults to `Worker`, not `All`: running this command IS the declaration,
    // and `All` is the value that claims the workspace filesystem. A chart that
    // never sets `OXY_ROLE` would otherwise get all three wrong branches below
    // anyway. An explicit `OXY_ROLE` still wins.
    crate::server::role_manifest::init_process_role_from_env_with_default(
        crate::server::role_manifest::Role::Worker,
    );

    require_database_url()?;

    // NOTE: do not add a second `init_process_role_from_env()` here. This PR
    // originally did, before main's #2822 added the `_with_default(Worker)`
    // call above, and the merge kept both. The second one is not an idempotent
    // no-op: `PROCESS_ROLE` is a `OnceLock` so the role stays `Worker`, but
    // `set_process_owns_workspace_files` is an `AtomicBool::store`, and with
    // `OXY_ROLE` unset the bare call recomputes `Role::All` and stores `true`
    // over the `false` the first call just wrote. The process then answers
    // `process_can_compile() == false` (reads the OnceLock) and
    // `process_owns_workspace_files() == true` (reads the atomic) — the exact
    // split-brain #2822 exists to remove, and worse than either end alone
    // because the two disagree. It also logs two contradictory
    // "role manifest initialised" lines.

    if args.skip_migrations || serve::skip_migrations_requested() {
        tracing::info!("worker: skipping migrations (--skip-migrations or OXY_SKIP_MIGRATIONS)");
    } else {
        tracing::info!("worker: running database migrations");
        serve::run_database_migrations(args.enterprise).await?;
        tracing::info!("worker: migrations complete");
    }

    // airway's process-wide `GlobalConfig`, from the singleton
    // `airway_deployment_config` row — installed once here rather than at the
    // top of each airway run, so it also covers any connector this process
    // builds outside a run. A standalone worker drains `TaskSpec::Airway` off
    // the queue, so it is one of the three entry points that can build a
    // source connector; see `crate::airway_boot` for the full roster. Never
    // fails boot: a malformed row is a warning here and a legible run failure
    // where the operator can see it.
    crate::airway_boot::install_deployment_tier_from_env().await;

    // Wire the product observability store, as `start_server_and_web_app`
    // does. `main` installs the `SpanCollectorLayer` for this command too
    // whenever `OXY_OBSERVABILITY_BACKEND` is set, and parks its receiver
    // until someone calls `finalize`. Nobody did here, so on every worker pod:
    //
    //   - the span channel was never drained. It is unbounded, and the
    //     latency worker closes a `driver_tick` span every poll (300ms in
    //     dev and prod), so it grew for the life of the pod: ~10 MiB/h of
    //     RSS from 0.5.145 on, until the 1 GiB limit OOM-killed it.
    //   - the custom-app sinks were never installed, so `record_event` and
    //     `record_logs` were no-ops: no `custom_app_events` / `custom_app_logs`
    //     row for any function invocation the worker ran.
    //   - the global store stayed unset, so agentic runs driven here recorded
    //     no metric usage either.
    //
    // Before `WorkerRuntime::start` and the run drivers, so the sinks exist
    // before the first invocation this process can run. `OXY_CLICKHOUSE_*`
    // comes from the pod spec, so unlike `oxy start` nothing has to boot first.
    crate::observability_boot::finalize().await;

    // Initialize the feature-flag cache — which wires the `oltp` kill-switch
    // bridge, starts the refresh, and reads the flags. Without it the worker's
    // OLTP resolutions (Airway landing into `raw_*`, the analyst for agentic
    // runs) ran with the switch permanently permissive.
    //
    // Non-fatal here — unlike serve. init wires the hook and spawns the refresh
    // BEFORE the fallible load, so a failure leaves `oltp` reading its unloaded
    // default (OFF = disabled = fail-closed) and the refresh self-heals on the
    // next tick. The worker enforces no paywall, so `billing`'s fail-open
    // default cannot bite here. ONE call, no arm that can skip the hook:
    // `--skip-migrations` makes this the process's first DB touch, and the
    // earlier `match establish_connection()` had an `Err` arm that skipped init
    // entirely and left the hook unwired (permissive) for the process's life.
    if let Err(e) = crate::server::feature_flags::cache::init().await {
        tracing::warn!(
            ?e,
            "worker: feature flag load failed; oltp reads disabled until refresh"
        );
    }

    let max_inflight = read_max_inflight();
    let recovery_interval = resolve_recovery_interval(args.recovery_interval_secs);
    let health_port = resolve_health_port(args.health_port);
    let db_host = mask_db_url();
    let version = env!("CARGO_PKG_VERSION");
    let commit = option_env!("GITHUB_SHA").unwrap_or("unknown");
    let worker_id = compute_worker_id();

    // Record on the current tracing span so child spans inherit the
    // identifier without re-passing it everywhere.
    let span = tracing::Span::current();
    span.record("worker_id", worker_id.as_str());
    span.record("version", version);

    tracing::info!(
        worker_id = worker_id.as_str(),
        worker_version = version,
        commit = commit,
        max_inflight,
        recovery_interval_secs = recovery_interval.as_secs(),
        health_port = ?health_port,
        db_host = %db_host,
        "worker: starting standalone worker process"
    );
    println!(
        "{} {} {}",
        "Oxy worker starting".text(),
        format!("(v{version}, commit {commit})").secondary(),
        format!("[id={worker_id} max_inflight={max_inflight}]").tertiary(),
    );

    let shutdown = CancellationToken::new();
    let runtime = WorkerRuntime::start(shutdown.clone(), worker_id.clone()).await?;

    let health_state = WorkerHealthState::new(version, worker_id.clone());
    let recovery_alive = health_state.recovery_alive_handle();
    let router_connected = health_state.router_connected_handle();
    // Router is connected once `WorkerRuntime::start` returns; record it.
    router_connected.store(true, std::sync::atomic::Ordering::Relaxed);

    // Reaper pre-pass loop. Narrow by design — it only runs `run_reaper` at a
    // faster-than-default cadence, and short-circuits entirely at the default
    // interval (see `tick_once`). The actual run-driving is `spawn_run_drivers`
    // just below; this loop stays separate because `/readyz` and the shutdown
    // drain are keyed on it.
    recovery_alive.store(true, std::sync::atomic::Ordering::Relaxed);
    let recovery_handle = spawn_recovery(&runtime, shutdown.clone(), recovery_interval);

    // Phase 1 of moving execution off the app node: the worker now runs the
    // same per-workspace driving loops `oxy serve` does — the latency worker
    // (`queued scope_owned = false` rows, sub-second) and the periodic
    // stranded-run tick — instead of relying on an HTTP node to drive
    // everything. It is an ADDITIONAL driver, not a replacement: the
    // request-time direct-drive (`spawn_airway_run_drive`) is untouched and
    // still executes interactive submits on the node that accepted them.
    // Dropping it is Phase 2, and that ordering is not stylistic — the
    // previous attempt inverted it (made runs Global, removed direct-drive,
    // with nothing draining the queue) and had to be reverted in 54355b198.
    //
    // `StartupPass::Skip` is the one thing that is NOT a copy of the serve
    // path, and it is load-bearing; see `StartupPass` for the full argument.
    // Short version: the one-shot pass selects on the driver lease alone, and
    // direct-drive never takes that lease, so a worker restart would re-drive
    // whatever the app node happens to be running.
    //
    // `ServeMode::Cloud` unconditionally: a worker fleet only exists in a
    // deployment that has a `workspaces` table to enumerate. Local development
    // runs the production path (`oxy serve --enterprise`), which is Cloud mode
    // too, so this is also what a local split-fleet box wants.
    //
    // Turn it off with `OXY_INPROC_GLOBAL_WORKER=0` — the same switch that
    // governs the loops inside `oxy serve`, and the kill switch if a fleet
    // needs to be put back to reaper-only behaviour without a rollback.
    spawn_run_drivers(
        runtime.agentic_state(shutdown.clone()),
        oxy_app_core::serve_mode::ServeMode::Cloud,
        // No Layer-1 preagg cache on this pod. `#3013` threaded the serve
        // node's cache into background work so queued runs resolve rollups on
        // the same tier their request-driven twins do; a standalone worker
        // builds no `AppState`, so it has none to pass.
        //
        // Correct but NOT free, and worth naming rather than discovering: a
        // run driven here compiles to warehouse SQL where the same run on an
        // `oxy serve` node would have hit a pre-aggregation. That is a
        // performance difference on exactly the work this change relocates.
        // Giving the worker its own cache is a follow-up, not a blocker.
        //
        // Not "identical either way", which is what this said first and is
        // stronger than the code supports: `RollupFreshness::ServeStale` is the
        // default posture, so a preagg-backed run on `oxy serve` can legitimately
        // answer from a rollup that is BEHIND the warehouse. The worker's
        // uncached answer is the fresher one. Same query, same semantics,
        // possibly different staleness — which is fine, and worth stating
        // accurately because "identical" is what someone will quote later.
        PreaggCacheCtx {
            cache: None,
            renewal_threshold_secs: None,
        },
        StartupPass::Skip,
    );

    // Optional health probe + Prometheus metrics server (off unless
    // --health-port / OXY_WORKER_HEALTH_PORT is set). Metrics share the
    // same listener as the probes — one tiny axum router with three
    // routes. HPA scrapes outstanding work (`oxy_queue_depth_queued` +
    // `oxy_queue_depth_claimed`) against `oxy_worker_capacity`.
    let health_handle = match health_port {
        Some(port) => {
            let metrics_state = crate::server::worker_metrics::MetricsState {
                worker_id: std::sync::Arc::new(worker_id.clone()),
                version,
                db: std::sync::Arc::new(
                    oxy::database::client::establish_connection()
                        .await
                        .map_err(|e| {
                            OxyError::RuntimeError(format!(
                                "worker metrics: DB connect failed: {e}"
                            ))
                        })?,
                ),
                capacity: crate::server::worker_metrics::ConcurrencyCaps::from_env(),
            };
            Some(
                worker_health::spawn(port, health_state, Some(metrics_state), shutdown.clone())
                    .await?,
            )
        }
        None => None,
    };

    // Block until a shutdown signal arrives, then cascade.
    wait_for_shutdown_signal().await;
    tracing::info!(
        worker_id = worker_id.as_str(),
        "worker: shutdown signal received, cancelling outstanding tasks"
    );
    println!("{}", "Oxy worker shutting down".text());

    // Close the claim path before anything else. The release below flips rows
    // `claimed -> queued`, which fires the queue's NOTIFY trigger and wakes
    // this process's own workers; without this gate they re-claim what we just
    // released and the process exits holding it with the budget charged.
    agentic_runtime::transport::begin_shutdown();

    shutdown.cancel();
    recovery_alive.store(false, std::sync::atomic::Ordering::Relaxed);

    drain_background(recovery_handle, health_handle).await;

    // Give back every durable-queue claim this process holds so a successor
    // can pick the work up immediately, budget-neutral. Placed after the
    // background drain (the recovery loop and health server are joined there)
    // and before `drop(runtime)`, which drops the DB connection the release
    // needs.
    //
    // `drain_background` is not a barrier on task *execution* — it waits on
    // the recovery loop and the health server, neither of which runs queued
    // tasks. The guarantee that matters is the `begin_shutdown` above: no new
    // claims from here on. A task still finishing when its row returns to
    // `queued` is handled by the driver-lease CAS and the heartbeat's
    // ownership predicate, not by ordering here.
    //
    // Deliberately NOT `worker_id` (that's `compute_worker_id()`, a
    // display-only `{host}@{pid}` string used for logs/health/metrics).
    // Actual queue claims are written by `DurableTransport` under the process
    // worker identity (`{env}·{host}·{short}`) — the two ids differ, and
    // releasing against the wrong one silently matches zero rows.
    release_queue_claims(&runtime.db).await;

    // Dropping `runtime` releases router + background-job handles.
    drop(runtime);
    tracing::info!(
        worker_id = worker_id.as_str(),
        "worker: graceful shutdown complete"
    );
    println!("{}", "Oxy worker stopped".success());
    Ok(())
}

/// Spawn the periodic recovery loop. Returns the join handle so the caller
/// can drain it on shutdown.
fn spawn_recovery(
    runtime: &WorkerRuntime,
    shutdown: CancellationToken,
    interval: Duration,
) -> JoinHandle<()> {
    let db = runtime.db.clone();
    let router = runtime.router.clone();
    tokio::spawn(async move {
        run_recovery_loop(db, router, shutdown, interval).await;
    })
}

/// Bound on joining the recovery loop during shutdown. Term 1 of 3 in the
/// budget `internal-docs/worker-fleet.md` publishes as
/// `terminationGracePeriodSeconds`; see `WORST_CASE_SHUTDOWN` near the top of
/// this file, whose assert pins the sum of all three.
const RECOVERY_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound on joining the optional health server during shutdown. Term 2 of 3.
/// Only present when `--health-port` is set.
const HEALTH_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Wait for the recovery loop (and optional health server) to drain after
/// the umbrella token is cancelled. We bound the wait so a stuck task can't
/// keep the process alive forever.
async fn drain_background(recovery_handle: JoinHandle<()>, health_handle: Option<JoinHandle<()>>) {
    let drain_deadline = RECOVERY_DRAIN_TIMEOUT;
    match tokio::time::timeout(drain_deadline, recovery_handle).await {
        Ok(Ok(())) => tracing::info!("worker: recovery loop drained cleanly"),
        Ok(Err(e)) => tracing::warn!(error = ?e, "worker: recovery loop join failed"),
        Err(_) => tracing::warn!(
            "worker: recovery loop did not drain within {}s; abandoning",
            drain_deadline.as_secs()
        ),
    }
    if let Some(handle) = health_handle {
        match tokio::time::timeout(HEALTH_JOIN_TIMEOUT, handle).await {
            Ok(Ok(())) => tracing::info!("worker: health server drained cleanly"),
            Ok(Err(e)) => tracing::warn!(error = ?e, "worker: health server join failed"),
            Err(_) => tracing::warn!(
                "worker: health server did not drain within {}s; abandoning",
                HEALTH_JOIN_TIMEOUT.as_secs()
            ),
        }
    }
}

/// How long [`release_queue_claims`] waits for the database before giving up.
///
/// Matches `oxy serve`'s `SHUTDOWN_HOOK_TIMEOUT` and for the same reason: a
/// *wedged* database (a down one fails fast) must never be why this pod
/// outlives its Kubernetes termination grace period and gets SIGKILLed —
/// which would skip the release entirely, the opposite of what it is for.
/// The shared pool IS bounded:
/// `establish_connection` sets `.connect_timeout(CONNECT_TIMEOUT)` (10s) and
/// `.acquire_timeout(ACQUIRE_TIMEOUT)` (30s) — `platform/src/db/client.rs`.
/// So without this constant the wait would be 30s per acquire, not unbounded.
/// 30s is still too long, which is why the bound stays: it is triple the 10s
/// this constant promises the termination grace period, and the mark + drain
/// pair compounds it (next paragraph). The justification is the budget, not
/// the absence of any other cap.
///
/// **One budget for the whole sequence, not one per call.** `recovery.rs`'s
/// `spawn_shutdown_hook` gets this for free — its mark-then-drain sequence
/// sits inside the *outer* `SHUTDOWN_HOOK_TIMEOUT` that already wraps the
/// whole hook body. `release_queue_claims` has no such outer bound (it is
/// awaited directly from `run_worker`, not spawned-and-timed the way the HTTP
/// hook is), so it wraps the mark + drain pair in a single
/// `RELEASE_CLAIMS_TIMEOUT` itself. Timing each call separately would let
/// worst-case shutdown take up to 20s — double what this constant documents
/// and promises the Kubernetes termination grace period.
///
/// **Term 3 of 3** in the shutdown budget `internal-docs/worker-fleet.md`
/// publishes; `WORST_CASE_SHUTDOWN` near the top of this file asserts the sum,
/// so changing this value fails the build until the doc is updated too.
const RELEASE_CLAIMS_TIMEOUT: Duration = Duration::from_secs(10);

/// Release every durable-queue claim this process holds back to the queue,
/// budget-neutral (`claim_count` is decremented, not charged) — the
/// standalone-worker counterpart to `oxy serve`'s `spawn_shutdown_hook`. A
/// rolling deploy evicts exactly this process, so a bounced task must not
/// exhaust `max_claims` and dead-letter despite never failing.
///
/// **Live as of the run-driving change.** This was forward-looking wiring
/// while `oxy worker` claimed nothing; now the driver loops spawned in
/// [`run_worker`] build a `DurableTransport` and claim on the first run they
/// pick up, so the mark + drain below has real rows to move on every eviction.
/// It also no longer depends on the recovery interval: a claim can exist at any
/// interval, so the `reachable when` cell for this row in
/// `internal-docs/worker-fleet.md` is `always`.
///
/// Best-effort and bounded: an error or a timeout degrades to the pre-existing
/// behaviour (the reaper reclaims the claim once its visibility timeout
/// expires), never a failed or hung shutdown.
async fn release_queue_claims(db: &sea_orm::DatabaseConnection) {
    // Lazily minted, so asking for the id unconditionally would forge a fresh
    // one in a process that never built a `DurableTransport` and issue a
    // guaranteed-zero-row UPDATE against a database that may be exactly what
    // is making shutdown slow. `None` means this process never claimed.
    let Some(worker_id) = agentic_runtime::transport::process_worker_id_if_initialized() else {
        return;
    };

    // Tracks how far the sequence got, so a timeout can report "marked but
    // not drained" rather than a generic "something timed out" — an operator
    // reading the log needs to know whether the orphaned-root marking landed
    // before the DB went unresponsive.
    let mut marked_roots = false;

    let sequence = async {
        // Order matters: `mark_released_roots_global` matches on this
        // worker's `worker_id` + `queue_status = 'claimed'`, and the drain
        // below clears both. Run it first so an orphaned workflow/airway root
        // becomes visible to the global claim path instead of waiting for a
        // process restart — see `mark_released_roots_global`'s doc comment
        // for why this is gated to `workflow`/`airway` and roots only.
        match agentic_runtime::crud::mark_released_roots_global(db, worker_id).await {
            Ok(0) => {}
            Ok(marked) => tracing::info!(
                target: "worker.recovery",
                marked,
                worker_id,
                "graceful shutdown: made orphaned roots globally recoverable"
            ),
            Err(e) => tracing::warn!(
                target: "worker.recovery",
                error = %e,
                worker_id,
                "graceful shutdown: failed to mark roots global"
            ),
        }
        marked_roots = true;

        // Drained rather than released once: a claim in flight when the
        // shutdown gate closed can land after the first pass. Bounded at 3
        // passes, stopping on the first empty one.
        match agentic_runtime::crud::drain_claims_for_worker(db, worker_id).await {
            Ok(0) => {}
            Ok(released) => tracing::info!(
                target: "worker.recovery",
                released,
                worker_id,
                "graceful shutdown: released claims back to the queue"
            ),
            Err(e) => tracing::warn!(
                target: "worker.recovery",
                error = %e,
                worker_id,
                "graceful shutdown: failed to release claims; the reaper will \
                 reclaim them after the visibility timeout"
            ),
        }
    };

    if tokio::time::timeout(RELEASE_CLAIMS_TIMEOUT, sequence)
        .await
        .is_err()
    {
        if marked_roots {
            tracing::warn!(
                target: "worker.recovery",
                timeout_secs = RELEASE_CLAIMS_TIMEOUT.as_secs(),
                worker_id,
                "graceful shutdown: timed out draining claims after marking \
                 roots global; undrained claims will be reclaimed by the \
                 reaper after the visibility timeout"
            );
        } else {
            tracing::warn!(
                target: "worker.recovery",
                timeout_secs = RELEASE_CLAIMS_TIMEOUT.as_secs(),
                worker_id,
                "graceful shutdown: timed out before marking roots global; \
                 neither the mark nor the drain ran to completion — orphaned \
                 roots stay scoped and claims stay held until the next \
                 recovery pass or the reaper's visibility timeout"
            );
        }
    }
}

/// Periodic reaper tick. **Not** the run driver — that is
/// `spawn_run_drivers` in [`run_worker`], which spawns
/// `server::router::recovery`'s loops. This one exists only to run
/// `run_reaper` faster than the background reaper's own cadence when an
/// operator asks for it, and no-ops otherwise; see [`tick_once`].
///
/// Kept as a separate loop rather than folded into the driver because
/// `/readyz` (`recovery_alive`) and the bounded shutdown drain
/// ([`RECOVERY_DRAIN_TIMEOUT`]) are both keyed on its join handle.
#[tracing::instrument(skip_all, fields(interval_secs = interval.as_secs()))]
async fn run_recovery_loop(
    db: sea_orm::DatabaseConnection,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    shutdown: CancellationToken,
    interval: Duration,
) {
    tracing::info!(
        target: "worker.recovery",
        interval_secs = interval.as_secs(),
        "recovery loop started"
    );
    // Run one tick up-front so /readyz becoming true means recovery has
    // already executed at least once, not "is scheduled to". Without this,
    // a 30s default interval means /readyz turns true at T+0 but the first
    // reaper pass doesn't fire until T+30.
    tokio::select! {
        _ = shutdown.cancelled() => {
            tracing::info!(target: "worker.recovery", "recovery loop: shutdown before first tick");
            return;
        }
        _ = tick_once(&db, &router, interval) => {}
    }
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!(target: "worker.recovery", "recovery loop: shutdown");
                return;
            }
            _ = tokio::time::sleep(interval) => {
                tick_once(&db, &router, interval).await;
            }
        }
    }
}

/// One reaper pass, conditionally.
///
/// The pass is skipped when the recovery interval is >= the background
/// reaper's own interval — running both at the same cadence is pure churn
/// against `agentic_task_queue`. Short `--recovery-interval-secs` still gets
/// the benefit (e.g. `--recovery-interval-secs 5` for incident triage). At the
/// default interval this function therefore returns before touching the DB,
/// which is the property the shutdown budget documented at the top of this
/// file used to rest on for BOTH its terms.
///
/// It now rests on it for one. Driving runs means claiming queue rows, so
/// `release_queue_claims` has real work to do on every shutdown regardless of
/// this interval — the `reachable when` column in
/// `internal-docs/worker-fleet.md`'s `terminationGracePeriodSeconds` table
/// says `always` for that row now. The recovery-drain term is still gated
/// here, because this early return is still "return from `tick_once`" rather
/// than "skip the reaper pre-pass".
async fn tick_once(
    db: &sea_orm::DatabaseConnection,
    router: &Arc<dyn agentic_runtime::router::TaskRouter>,
    interval: Duration,
) {
    if interval >= agentic_runtime::background::REAPER_INTERVAL {
        // Background reaper already runs at >= the same cadence; calling
        // it again here just duplicates writes against agentic_task_queue.
        return;
    }
    let transport =
        agentic_runtime::transport::DurableTransport::with_router(db.clone(), router.clone(), None);
    let reaped = transport.run_reaper().await.total();
    if reaped > 0 {
        tracing::info!(
            target: "worker.recovery",
            count = reaped,
            "reaper pre-pass: freed stale queue entries"
        );
    }
}

/// Validate that the environment has everything needed to open the database
/// connection.  The check is auth-mode-aware:
///
/// - `password` mode (default): `OXY_DATABASE_URL` must be set.
/// - `iam` mode: delegates to [`IamConfig::from_env`] which validates all
///   required vars including port format and ssl-mode values.
///
/// Using the canonical platform validators keeps this guard in sync with
/// the actual connection layer automatically.
fn require_database_url() -> Result<(), OxyError> {
    match DatabaseAuthMode::from_env()? {
        DatabaseAuthMode::Iam => IamConfig::from_env().map(|_| ()),
        DatabaseAuthMode::Password => {
            if std::env::var("OXY_DATABASE_URL").is_err() {
                return Err(OxyError::RuntimeError(
                    "OXY_DATABASE_URL environment variable is required.\n\n\
                     Set it to the same PostgreSQL connection string your `oxy serve`\n\
                     frontend uses — the worker process and the HTTP server must share\n\
                     one queue (and therefore one Postgres database).\n\n\
                     Alternatively, set OXY_DATABASE_AUTH_MODE=iam and supply the\n\
                     individual OXY_DATABASE_HOST / NAME / USER / REGION vars."
                        .to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn read_max_inflight() -> usize {
    std::env::var(agentic_runtime::worker::MAX_INFLIGHT_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(agentic_runtime::worker::DEFAULT_MAX_INFLIGHT)
}

fn resolve_recovery_interval(arg: Option<u64>) -> Duration {
    let secs = arg
        .or_else(|| {
            std::env::var(RECOVERY_INTERVAL_ENV)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
        })
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_RECOVERY_INTERVAL_SECS);
    Duration::from_secs(secs)
}

/// Resolve the health probe port: CLI flag wins, then env var, then None.
/// Unparsable env values quietly fall through to `None` instead of
/// crashing the worker on startup — health probes are optional.
fn resolve_health_port(arg: Option<u16>) -> Option<u16> {
    if let Some(port) = arg {
        return Some(port);
    }
    std::env::var(HEALTH_PORT_ENV)
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
}

/// Stable per-process worker identifier used as a structured log field on
/// the startup banner, shutdown logs, and `#[tracing::instrument]` spans.
///
/// Format: `{hostname}@{pid}`. We do not pull in the `hostname` crate; the
/// `HOSTNAME` env var is set on virtually every container runtime (k8s,
/// docker, systemd) and the fallback `"unknown"` is fine for the local
/// dev case. This is an instance identifier, not PII.
fn compute_worker_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
    format!("{host}@{}", std::process::id())
}

/// Return a safe-to-log database identifier.
///
/// - `password` mode: strips the password from `OXY_DATABASE_URL`.
/// - `iam` mode: formats `host/database` from the individual IAM vars since
///   there is no URL to parse.
fn mask_db_url() -> String {
    if DatabaseAuthMode::from_env().ok() == Some(DatabaseAuthMode::Iam) {
        let host = std::env::var("OXY_DATABASE_HOST").unwrap_or_else(|_| "<unset>".to_string());
        let db = std::env::var("OXY_DATABASE_NAME").unwrap_or_else(|_| "<unset>".to_string());
        return format!("{host}/{db} (IAM)");
    }
    mask_db_url_str(&std::env::var("OXY_DATABASE_URL").unwrap_or_default())
}

/// Internal helper: pure function over a URL string so tests don't have to
/// mutate process env.
fn mask_db_url_str(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            // We never log the password — overwrite if present.
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        }
        Err(_) => "<invalid OXY_DATABASE_URL>".to_string(),
    }
}

async fn wait_for_shutdown_signal() {
    // Force-exit watcher: any second SIGINT arriving while we're draining
    // (or even before the first signal fires) kills the process. Spawning
    // this *before* the wait — rather than inside the same function as a
    // child task that leaks across drains — keeps shutdown reentrant and
    // makes the test-only signal path easier to reason about.
    let _force_exit: JoinHandle<()> = tokio::spawn(async {
        // Wait for the first Ctrl+C to arm the watcher, then wait again for
        // a second one. The first wait deliberately races with the primary
        // shutdown handler below; whichever fires first, the watcher only
        // exits the process on the *second* Ctrl+C.
        if signal::ctrl_c().await.is_err() {
            return;
        }
        if signal::ctrl_c().await.is_err() {
            return;
        }
        tracing::warn!("worker: second shutdown signal, forcing exit");
        std::process::exit(1);
    });

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("worker: SIGINT received");
        }
        _ = terminate => {
            tracing::info!("worker: SIGTERM received");
        }
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
