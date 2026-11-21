//! The eval context: `oxy test`, which runs the LLM-as-judge evaluations
//! defined in `.test.yml` files against their `.agentic.yml` targets.
//!
//! Its own owner in `oxy-cli-shim`, registered by `oxy-server` (rule S10 in
//! `internal-docs/domain-boundaries.md`, "the CLI is a shim; a command belongs
//! to its owner"). It moved out of `oxy-app` in two steps, top down, because
//! the engine builds its runs on `OxyProjectContext`, which only `oxy-app` can
//! name, so this crate sits above it and `oxy-app` can never call back:
//!
//! - **First (PR 1):** the command (`test_command`), its event handler,
//!   progress bars and launch helpers (`service`, was
//!   `oxy-app`'s `server::service::eval`), and the reporters (`reporters`, was
//!   `integrations::eval::reporters`).
//! - **Then (PR 2):** the engine (`builders`, was `oxy-app`'s
//!   `integrations::eval::builders`: the launcher, the target, the solvers and
//!   judges, the result types). It builds its runs on
//!   `oxy_app::surface::workspace_runtime::OxyProjectContext`.
//!
//! It mounts no route. It is named `api-*` because that is the layer it sits
//! in (above `oxy-app`, reaching it through `oxy_app::surface` only, held by
//! `surface_contract`), where the airway context's `oxy airway` lived until
//! it became `oxyc airway` (2026-11-21).
#![recursion_limit = "256"]

// The engine. `pub` as its types were when `oxy-app` re-exported them
// (`surface::eval`), so none of its constructors reads as dead here.
pub mod builders;
mod reporters;
mod service;
mod test_command;

// `oxy test`: its own owner in `oxy-cli-shim`, registered by `oxy-server`.
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

/// Run `oxy test …`. The error propagates to the shim, as it did from
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
