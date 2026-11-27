//! `oxy dev …`: the hidden group of dev tooling — `seed` (this crate's,
//! [`seed`], up from oxy-app in decompose SD), the dev `compile` (`crate::cli`'s)
//! and `test` (oxy-api-eval's). They write straight into a database or a local
//! checkout, so they are compiled only with the `dev-tools` feature: on by
//! default (debug builds, `just up`, CI's agentic job, the fleet image), off in
//! the release build (`.github/actions/build-oxy-cli`), so a shipped binary
//! cannot seed. `internal-docs/oxy-cli-to-oxyc.md`, "Dev tooling (class d)".
//!
//! The old top-level `oxy seed` and `oxy test` ran for one release as hidden
//! aliases; 0.5.162 shipped them and they were deleted 2026-11-26.

pub mod seed;

use crate::router::SurfaceSeams;
use oxy_cli_shim::RunResult;

// The `dev` group, its own owner in `oxy-cli-shim`. A plain comment, not `///`:
// clap prints a doc comment on this enum as the root command's `about`.
#[derive(clap::Subcommand, Debug)]
pub enum DevGroup {
    /// Dev tooling: seed a local database, compile a local workspace, run evals
    ///
    /// Not in release binaries. `seed` writes demo rows straight into a local
    /// database; `compile` compiles a checkout into it; `test` runs the
    /// `.test.yml` evals against a local workspace.
    #[command(subcommand, hide = true)]
    Dev(DevCommand),
}

// What `oxy dev` lists: `seed`, `crate::cli`'s `compile`, then `test`.
#[derive(clap::Subcommand, Debug)]
pub enum DevCommand {
    /// Seed the demo workspace, partners, tenants and platform grants into a local database
    ///
    /// Writes the rows routes and tokens later depend on, loads the demo's
    /// Airhouse data, and compiles + promotes every seeded workspace. Refuses a
    /// database that does not look local.
    Seed(seed::SeedArgs),
    #[command(flatten)]
    App(crate::cli::DevCommand),
    #[command(flatten)]
    Eval(oxy_api_eval::Command),
}

pub async fn run(group: DevGroup, seams: SurfaceSeams) -> RunResult {
    let DevGroup::Dev(command) = group;
    match command {
        DevCommand::Seed(seed_args) => {
            ::oxy_telemetry::sentry_config::add_operation_context("seed", None);
            seed::handle_seed_command(seed_args, seams.example_apps).await?;
            Ok(())
        }
        DevCommand::App(command) => crate::cli::run_dev(command).await,
        DevCommand::Eval(command) => oxy_api_eval::run(command).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser, Subcommand};

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        dev: DevGroup,
    }

    /// `oxy dev` lists `seed`, `compile`, `test` in that order, and stays out of
    /// the root `--help`.
    #[test]
    fn the_dev_group_lists_seed_compile_test_and_is_hidden() {
        let cmd = Cli::command();
        let dev = cmd.find_subcommand("dev").expect("a dev group");
        assert!(dev.is_hide_set(), "`oxy dev` shows in the root --help");
        let names: Vec<&str> = dev.get_subcommands().map(|c| c.get_name()).collect();
        assert_eq!(names, ["seed", "compile", "test"]);
    }

    #[test]
    fn the_dev_spellings_parse() {
        for argv in [
            &[
                "oxy",
                "dev",
                "seed",
                "--workspace-path",
                "./examples",
                "--llm-keys",
            ][..],
            &["oxy", "dev", "seed", "--org-workspace", "acme"],
            &[
                "oxy",
                "dev",
                "compile",
                "--promote",
                "--skip-migrations",
                "--json",
            ],
            &["oxy", "dev", "test", "x.test.yml", "--format", "json"],
        ] {
            Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        }
    }

    /// The old top-level spellings are gone (their one-release aliases were
    /// deleted 2026-11-26); the top-level `compile` was never renamed (the
    /// template's CI gate runs it on a release binary).
    #[test]
    fn the_old_spellings_are_not_top_level() {
        for name in ["seed", "test"] {
            assert!(
                !crate::cli::SubCommand::has_subcommand(name),
                "`oxy {name}` is a top-level command again"
            );
        }
        assert!(crate::cli::SubCommand::has_subcommand("compile"));
    }
}
