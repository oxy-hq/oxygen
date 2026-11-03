//! CLI argument types for Oxy commands

use clap::Parser;

/// Arguments for the `oxy serve` command (web server only, no Docker)
#[derive(Parser, Debug, Clone)]
pub struct ServeArgs {
    /// Port number for the web application server
    ///
    /// Specify which port to bind the Oxy web interface.
    /// Reads `OXY_HTTP_PORT` when the flag is omitted; default is 3000.
    ///
    /// NAME MATTERS — do NOT use `OXY_PORT` or `OXY_<role>_PORT`. Kubernetes
    /// injects Docker-link service-discovery vars `<SVCNAME>_PORT=tcp://<ip>:<port>`
    /// into every pod, for every Service in the namespace. So `OXY_<X>_PORT` is
    /// silently overwritten with `tcp://...` whenever a Service named `oxy-<x>`
    /// exists, and clap then can't parse it as a u16 — crashlooping the pod.
    ///   - `OXY_PORT` collided with the prod Service `oxy` (incident 2026-06-29,
    ///     release 0.5.90).
    ///   - `OXY_SERVE_PORT` would collide with a future `oxy-serve` fleet Service
    ///     (split-fleet design, internal-docs/multi-instance-fleet.md) — avoided.
    /// `http` is not a fleet role (serve/ide/worker), so `oxy-http` is not a
    /// plausible Service name. The durable, name-independent fix is
    /// `enableServiceLinks: false` on the pod spec (oxy-app Helm chart). See
    /// DEVELOPMENT.md "Kubernetes env-var collision".
    #[clap(long, env = "OXY_HTTP_PORT", default_value_t = 3000)]
    pub port: u16,
    /// Host address to bind the web application server
    ///
    /// Specify which host address to bind the Oxy web interface.
    /// Default is 0.0.0.0 to listen on all interfaces.
    #[clap(long, default_value = "0.0.0.0")]
    pub host: String,
    /// Force HTTP/2 only mode (disable HTTP/1.1)
    ///
    /// When enabled, the server will only accept HTTP/2 connections over TLS.
    /// HTTP/1.1 requests will be rejected. Default supports both protocols.
    #[clap(long, default_value_t = false)]
    pub http2_only: bool,
    /// TLS certificate file for HTTPS (local development)
    #[clap(long, default_value = "localhost+2.pem")]
    pub tls_cert: String,
    /// TLS private key file for HTTPS (local development)
    #[clap(long, default_value = "localhost+2-key.pem")]
    pub tls_key: String,

    /// Port for the internal API server (no authentication required)
    ///
    /// The internal port serves the same API routes without authentication.
    /// Binds to 127.0.0.1 by default for security. Set to 0 to disable.
    /// Reads `OXY_HTTP_INTERNAL_PORT` when the flag is omitted; default is 3001.
    /// (Not `OXY_INTERNAL_PORT` — same Kubernetes service-link reason as `port`.)
    #[clap(long, env = "OXY_HTTP_INTERNAL_PORT", default_value_t = 3001)]
    pub internal_port: u16,

    /// Host address to bind the internal API server
    ///
    /// Default is 127.0.0.1 (localhost only) for security since the internal
    /// port has no authentication. Use 0.0.0.0 for Docker/container deployments
    /// where the port needs to be accessible within the container network.
    #[clap(long, default_value = "127.0.0.1")]
    pub internal_host: String,

    /// Enable enterprise features (observability, analytics)
    #[clap(long, default_value_t = false)]
    pub enterprise: bool,

    /// Run in local mode: single workspace rooted at the current directory,
    /// no orgs, guest authentication. Ignores configured auth provider env vars.
    ///
    /// WARNING: local mode disables authentication entirely. Do not expose
    /// a `--local` instance on a non-loopback interface without a reverse
    /// proxy that restricts access.
    #[clap(long, default_value_t = false)]
    pub local: bool,

    /// Disable in-process agentic workers: HTTP only, no startup recovery and
    /// no periodic global driver loop.
    ///
    /// Use when running a separate `oxy worker` fleet — the workers handle all
    /// task execution, recovery, and scheduler ticks; `oxy serve` only accepts
    /// HTTP requests and writes new tasks to the queue.
    ///
    /// Usually unnecessary: `OXY_ROLE=serve` already derives this. The flag (and
    /// `OXY_DISABLE_INPROCESS_WORKERS`) remain explicit overrides; the CLI flag
    /// wins over both.
    #[clap(long, default_value_t = false)]
    pub no_workers: bool,
}

impl ServeArgs {
    /// True when in-process worker plumbing (startup recovery, periodic global
    /// driver loop, task execution) should be skipped because a separate
    /// `oxy worker` fleet handles it.
    ///
    /// Resolution order: `--no-workers` flag → `OXY_DISABLE_INPROCESS_WORKERS`
    /// env (honored in BOTH directions, so `=0` force-enables) → derived from
    /// `OXY_ROLE`. A stateless `serve` replica derives OFF (offload to the
    /// worker fleet); every other role (`all` / `ide` / `worker`) runs them
    /// in-process. This is why a serve node no longer needs `--no-workers` set.
    pub fn workers_disabled(&self) -> bool {
        // Explicit CLI flag always wins.
        if self.no_workers {
            return true;
        }
        // Explicit env override wins next, in BOTH directions, so
        // `OXY_DISABLE_INPROCESS_WORKERS=0` can force workers ON for a role whose
        // derived default is OFF.
        if let Ok(v) = std::env::var("OXY_DISABLE_INPROCESS_WORKERS") {
            return matches!(v.as_str(), "1" | "true" | "yes" | "on");
        }
        // Otherwise derive from the process role: only the stateless serve
        // replica offloads everything to the worker fleet. Shared invariant so a
        // single `OXY_ROLE=all` instance always runs its own in-process workers.
        !crate::server::role_manifest::role_runs_inprocess_workers(
            crate::server::role_manifest::current_process_role(),
        )
    }
}

/// Arguments for the `oxy start` command (Docker containers + web server)
#[derive(Parser, Debug)]
pub struct StartArgs {
    /// Server configuration options (includes --enterprise flag)
    #[clap(flatten)]
    pub serve: ServeArgs,

    /// Clean start: remove existing Docker containers and volumes before starting
    ///
    /// When enabled, removes all Oxy-managed Docker containers and their
    /// associated volumes to start with a fresh state. This is useful for
    /// troubleshooting or resetting the local environment.
    #[clap(long, default_value_t = false)]
    pub clean: bool,

    /// Bring up the databases and exit, without starting the server.
    ///
    /// For scripts that need a database and then want to run their own
    /// sequence of commands — `oxy start` otherwise blocks in the server, so a
    /// caller had to either background it or manage Postgres itself. The demo
    /// did the latter, which is how it ended up with a second Postgres and its
    /// own compose file to keep in sync.
    #[clap(long, default_value_t = false)]
    pub db_only: bool,

    /// Do not start the local Airhouse.
    ///
    /// `oxy start` runs one beside Postgres, where workspace data lives as it
    /// does in the cloud. Skipped as well when `AIRHOUSE_BASE_URL` is set: that
    /// names the Airhouse to use.
    #[clap(long, default_value_t = false)]
    pub no_airhouse: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn local_flag_defaults_to_false() {
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(!args.local);
    }

    #[test]
    fn local_flag_is_parsed() {
        let args = ServeArgs::parse_from(["oxy", "--local"]);
        assert!(args.local);
    }

    #[test]
    fn no_workers_defaults_to_false() {
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(!args.no_workers);
    }

    #[test]
    fn no_workers_flag_is_parsed() {
        let args = ServeArgs::parse_from(["oxy", "--no-workers"]);
        assert!(args.no_workers);
        assert!(args.workers_disabled());
    }

    #[test]
    fn workers_disabled_falls_back_to_env() {
        // Lib-binary tests run intra-binary in parallel under nextest, so
        // any test that mutates env state needs to serialize on a shared
        // mutex. Today only this test touches OXY_DISABLE_INPROCESS_WORKERS,
        // but the lock is here so a future test added to this binary
        // doesn't race against this one. Matches the ENV_LOCK pattern in
        // crates/app/src/cli/commands/worker_tests.rs.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("OXY_DISABLE_INPROCESS_WORKERS").ok();
        // SAFETY: ENV_LOCK serializes env mutation within this binary.
        unsafe { std::env::set_var("OXY_DISABLE_INPROCESS_WORKERS", "1") };
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(args.workers_disabled());
        // Restore prior env state to avoid leaking across tests.
        match prev {
            Some(v) => unsafe { std::env::set_var("OXY_DISABLE_INPROCESS_WORKERS", v) },
            None => unsafe { std::env::remove_var("OXY_DISABLE_INPROCESS_WORKERS") },
        }
    }

    // The next three tests set OXY_ROLE + the override env and call
    // init_process_role_from_env (a process-wide OnceLock). They rely on
    // nextest running each test in its OWN process — the same isolation the
    // role_middleware tests use — so the role set here never leaks. Under
    // `cargo test` (shared process) they would race; CLAUDE.md mandates nextest.

    #[test]
    fn workers_disabled_derives_from_serve_role() {
        // serve = stateless replica → offload all execution to the worker fleet.
        // SAFETY: nextest isolates this test in its own single-threaded process.
        unsafe {
            std::env::remove_var("OXY_DISABLE_INPROCESS_WORKERS");
            std::env::set_var("OXY_ROLE", "serve");
        }
        crate::server::role_manifest::init_process_role_from_env();
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(
            args.workers_disabled(),
            "serve role with no override should derive workers-disabled"
        );
    }

    #[test]
    fn workers_enabled_derives_from_non_serve_role() {
        // SAFETY: nextest isolates this test in its own single-threaded process.
        unsafe {
            std::env::remove_var("OXY_DISABLE_INPROCESS_WORKERS");
            std::env::set_var("OXY_ROLE", "all");
        }
        crate::server::role_manifest::init_process_role_from_env();
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(
            !args.workers_disabled(),
            "all role with no override should derive workers-enabled"
        );
    }

    #[test]
    fn env_override_forces_workers_on_for_serve_role() {
        // serve would derive OFF, but an explicit `=0` forces workers ON.
        // SAFETY: nextest isolates this test in its own single-threaded process.
        unsafe {
            std::env::set_var("OXY_ROLE", "serve");
            std::env::set_var("OXY_DISABLE_INPROCESS_WORKERS", "0");
        }
        crate::server::role_manifest::init_process_role_from_env();
        let args = ServeArgs::parse_from(["oxy"]);
        assert!(
            !args.workers_disabled(),
            "explicit OXY_DISABLE_INPROCESS_WORKERS=0 must force workers ON even for serve"
        );
    }
}
