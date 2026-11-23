//! The eval context: LLM-as-judge evaluations defined in `.test.yml` files,
//! run against their `.agentic.yml` targets.
//!
//! It moved out of `oxy-app` in two steps, top down, because the engine builds
//! its runs on `OxyProjectContext`, which only `oxy-app` could name then, so this
//! crate sits above it and `oxy-app` can never call back:
//!
//! - **First (PR 1):** the `oxy test` command (`test_command`), its event
//!   handler, progress bars and launch helpers, and the reporters, each its
//!   own `oxy-cli-shim` owner, registered by `oxy-server` (rule S10 in
//!   `internal-docs/domain-boundaries.md`).
//! - **Then (PR 2):** the engine (`builders`, was `oxy-app`'s
//!   `integrations::eval::builders`: the launcher, the target, the solvers and
//!   judges, the result types).
//! - **Then (2026-11-21):** the routes `oxyc evals run` calls ([`runs`]):
//!   `POST /api/{workspace_id}/evals/runs` queues an `eval_run` task (its
//!   executor rides `SurfaceSeams::tasks`), `GET …/evals/runs/{id}` reads the
//!   results back. They read the suite from the compile boundary
//!   (`eval_test_definitions`), never a working copy, and run the engine on
//!   the worker fleet (`internal-docs/oxy-cli-to-oxyc.md`, step 12). The
//!   engine takes the project context and the test file from its caller there
//!   (`EvalLauncher::with_project`, `EvalInput::test`); `oxy test` keeps its
//!   local run over a checkout: it stays as dev tooling (it moves under
//!   `oxy dev`, step 13, decision 2026-11-22), because the BIRD harness
//!   (`evals/bird`) runs a local workspace no deployment holds.
//!
//! It no longer depends on `oxy-app`: since the R1 decompose it imports
//! from the crates below it (`oxy-app-core`, `oxy-workspace-runtime`,
//! `oxy-compile-runtime`, `oxy-server-authz`).
#![recursion_limit = "256"]

// The engine. `pub` as its types were when `oxy-app` re-exported them
// (`surface::eval`), so none of its constructors reads as dead here.
pub mod builders;
mod reporters;
pub mod runs;
mod service;
mod test_command;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_app_core::seams::TaskSeam;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The workspace seam's share, merged inside `/api/{workspace_id}`: the eval
/// runs ([`runs`]).
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/evals/runs", post(runs::handlers::create_run))
        .route("/evals/runs/{run_id}", get(runs::handlers::get_run))
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree:
/// FleetOk, as both read Postgres and enqueue, and touch no disk.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] =
    &[fleet("/evals/runs"), fleet("/evals/runs/{run_id}")];

/// The task kind this surface queues and runs: one eval run.
pub fn tasks() -> Vec<TaskSeam> {
    vec![TaskSeam {
        kind: runs::executor::EVAL_RUN_KIND,
        build: |deps| {
            Arc::new(runs::executor::EvalRunExecutor {
                db: deps.db.clone(),
            })
        },
    }]
}

/// `oxy test`'s arguments, for `oxy-server`'s one-release hidden alias of the
/// old top-level spelling (the command is `oxy dev test` now).
pub use test_command::TestArgs;

// `oxy dev test`: flattened into `oxy-server`'s `dev` group, registered only
// with its `dev-tools` feature (dev tooling; deployments use `oxyc evals`).
// The routes and the task kind above are mounted in every build.
// The variant's doc is the command's `--help` text, carried over verbatim. A
// plain comment, not a doc comment: clap's derive turns a doc on this enum into
// the `about` of the whole `oxy` command, as `oxy-server`'s `ServerCommand`
// notes too.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Run evaluation tests defined in .test.yml files
    ///
    /// Executes the cases in a `.test.yml` file against its `.agentic.yml`
    /// target, scores each run with an LLM judge, and reports accuracy and
    /// consistency. Omit the file to run every `.test.yml` in the project.
    Test(test_command::TestArgs),
}

/// Run `oxy dev test …`. The error propagates to the shim, as it did from
/// `oxy-app`'s dispatcher, which prints it and exits 1.
pub async fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Test(test_args) => {
            oxy::sentry_config::add_operation_context("test", test_args.file.as_deref());
            test_command::handle_test_command(test_args).await?;
        }
    }
    Ok(())
}
