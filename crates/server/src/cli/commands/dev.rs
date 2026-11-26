//! The CLI's half of `oxy dev …`: the dev `compile`. Compiled only with the
//! `dev-tools` feature, on by default (debug builds, `just up`, CI's agentic
//! job, the fleet image) and off in the release build
//! (`.github/actions/build-oxy-cli`). `crate::dev` composes the `dev` group
//! from its `seed` (up from here in decompose SD, 2026-11-24), this, and
//! `oxy-api-eval`'s `test` (`internal-docs/oxy-cli-to-oxyc.md`, "Dev tooling
//! (class d)"). Up from `oxy-app` with the rest of the CLI in decompose CLI.
//!
//! The top-level `oxy compile` stays in every build: a workspace template
//! pinned to a release from before `oxyc compile` (step 8) still runs it.

use std::error::Error;

use super::compile;

// The commands under `oxy dev`. A plain comment, not `///`: `crate::dev`
// flattens this enum into its `dev` group, and clap would print a doc comment
// here as that group's `about`.
#[derive(clap::Subcommand, Debug)]
pub enum DevCommand {
    /// Compile a local workspace into the local database
    ///
    /// The same compile as the top-level `oxy compile` (which stays for the
    /// workspace template's CI gate): walks every recognized YAML/SQL file,
    /// parses it, and writes a `revisions` row plus per-entity rows. `--promote`
    /// makes the new revision the one the workspace serves.
    Compile(compile::CompileArgs),
}

/// Run one of [`DevCommand`].
pub async fn run(command: DevCommand) -> Result<(), Box<dyn Error>> {
    match command {
        DevCommand::Compile(compile_args) => super::compile_command(compile_args).await,
    }
    Ok(())
}
