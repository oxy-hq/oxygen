//! The long-lived commands: `start`, `serve` and `worker`. They are this
//! binary's own, registered with the `oxy-cli-shim` parser as the first
//! owner, ahead of `oxy-app`'s one-shot commands. They take from `oxy-app`
//! only what `oxy_app::host` lists.

pub mod args;
pub mod serve;
mod start;
mod worker;

#[cfg(test)]
mod entry_point_tests;

use oxy_app::host::router::SurfaceSeams;
use oxy_shared::errors::OxyError;

use args::{ServeArgs, StartArgs};

// The server commands, listed first by `--help`. A plain comment, not `///`:
// clap prints a doc comment on this enum as the root command's `about`.
#[derive(clap::Subcommand, Debug)]
pub enum ServerCommand {
    /// Start with Docker PostgreSQL (recommended)
    ///
    /// Launch PostgreSQL in Docker and start the Oxy web server.
    /// Uses postgres:18-alpine container for modern PostgreSQL features.
    /// Data persists in Docker volume 'oxy-postgres-data'.
    Start(StartArgs),
    /// Start the web server (requires OXY_DATABASE_URL)
    ///
    /// Launch the Oxy server. Requires OXY_DATABASE_URL environment variable
    /// to be set to a PostgreSQL connection string.
    /// For automatic PostgreSQL setup, use 'oxy start' instead.
    Serve(ServeArgs),
    /// Run an agentic worker process standalone (no HTTP server).
    ///
    /// Drains the durable task queue (`agentic_task_queue`) from a separate
    /// process so the worker fleet can scale independently of the HTTP
    /// frontend. Pair with `oxy serve --no-workers` for fleets where the
    /// HTTP server is HTTP-only. Requires OXY_DATABASE_URL.
    Worker(worker::WorkerArgs),
}

/// Run one of [`ServerCommand`] with the seams `main` composed.
///
/// A failure is returned rather than `exit(1)`-ed: `main` prints it, records it
/// to Sentry, and flushes the platform-telemetry exporters, so the failure that
/// crash-loops a pod is the last thing HyperDX shows for it.
pub async fn run(command: ServerCommand, seams: SurfaceSeams) -> oxy_cli_shim::RunResult {
    match command {
        ServerCommand::Start(args) => start::start_database_and_server(args, seams)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Failed to start: {e}")))?,
        ServerCommand::Serve(args) => serve::start_server_and_web_app(args, seams)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Server failed: {e}")))?,
        ServerCommand::Worker(args) => {
            worker::run_worker(args, seams.tasks, seams.global_ticks, seams.notifications)
                .await
                .map_err(|e| OxyError::RuntimeError(format!("Worker failed: {e}")))?
        }
    }
    Ok(())
}
