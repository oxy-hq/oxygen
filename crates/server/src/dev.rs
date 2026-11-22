//! `oxy dev …`: the hidden group of dev tooling — `seed` and the dev `compile`
//! (oxy-app's) and `test` (oxy-api-eval's). They write straight into a
//! database or a local checkout, so they are compiled only with the
//! `dev-tools` feature: on by default (debug builds, `just up`, CI's agentic
//! job, the fleet image), off in the release build
//! (`.github/actions/build-oxy-cli`), so a shipped binary cannot seed.
//! `internal-docs/oxy-cli-to-oxyc.md`, "Dev tooling (class d)".
//!
//! The old top-level `oxy seed` and `oxy test` still run for one release, in
//! dev-tools builds only, and print the new spelling first (on stderr).

use oxy_app::host::router::SurfaceSeams;
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

// What `oxy dev` lists: oxy-app's `seed` and `compile`, then `test`.
#[derive(clap::Subcommand, Debug)]
pub enum DevCommand {
    #[command(flatten)]
    App(oxy_app::cli::DevCommand),
    #[command(flatten)]
    Eval(oxy_api_eval::Command),
}

pub async fn run(group: DevGroup, seams: SurfaceSeams) -> RunResult {
    let DevGroup::Dev(command) = group;
    match command {
        DevCommand::App(command) => oxy_app::cli::run_dev(command, seams).await,
        DevCommand::Eval(command) => oxy_api_eval::run(command).await,
    }
}

// The old top-level `oxy test`, kept for one release. Hidden; `oxy seed`'s
// alias is oxy-app's (its `SubCommand::Seed`).
#[derive(clap::Subcommand, Debug)]
pub enum Renamed {
    /// Renamed to `oxy dev test` (kept for one release)
    #[command(hide = true)]
    Test(oxy_api_eval::TestArgs),
}

pub async fn run_renamed(command: Renamed) -> RunResult {
    match command {
        Renamed::Test(args) => {
            eprintln!("{}", oxy_app::cli::renamed_hint("test"));
            oxy_api_eval::run(oxy_api_eval::Command::Test(args)).await
        }
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

    /// The old spellings still parse in a dev-tools build, hidden, for one
    /// release; the top-level `compile` is not renamed (the template's CI gate
    /// runs it on a release binary).
    #[test]
    fn the_old_spellings_are_hidden_aliases() {
        assert!(Renamed::has_subcommand("test"));
        assert!(oxy_app::cli::SubCommand::has_subcommand("seed"));
        assert!(oxy_app::cli::SubCommand::has_subcommand("compile"));
        let root = Renamed::augment_subcommands(oxy_app::cli::SubCommand::augment_subcommands(
            clap::Command::new("oxy"),
        ));
        for name in ["seed", "test"] {
            let sub = root.find_subcommand(name).expect(name);
            assert!(sub.is_hide_set(), "`oxy {name}` shows in the root --help");
        }
    }
}
