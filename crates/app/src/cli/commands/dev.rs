//! oxy-app's half of `oxy dev …`: `seed` and the dev `compile`. Compiled only
//! with the `dev-tools` feature, which `oxy-server` turns on by default (debug
//! builds, `just up`, CI's agentic job, the fleet image) and the release build
//! turns off (`.github/actions/build-oxy-cli`), so a shipped binary cannot seed.
//! `oxy-server` composes the `dev` group from this and `oxy-api-eval`'s `test`
//! (`internal-docs/oxy-cli-to-oxyc.md`, "Dev tooling (class d)").
//!
//! The top-level `oxy compile` stays in every build: a workspace template
//! pinned to a release from before `oxyc compile` (step 8) still runs it.

use std::error::Error;

use ::oxy::theme::StyledText;
use clap::Parser;
use oxy_shared::errors::OxyError;

use super::{compile, seed, seed_org_workspace, seed_partners};
use crate::server::router::{ExampleAppsSeam, SurfaceSeams};

// The commands under `oxy dev`. A plain comment, not `///`: `oxy-server`
// flattens this enum into its `dev` group, and clap would print a doc comment
// here as that group's `about`.
#[derive(clap::Subcommand, Debug)]
pub enum DevCommand {
    /// Seed the demo workspace, partners, tenants and platform grants into a local database
    ///
    /// Writes the rows routes and tokens later depend on, loads the demo's
    /// Airhouse data, and compiles + promotes every seeded workspace. Refuses a
    /// database that does not look local.
    Seed(SeedArgs),
    /// Compile a local workspace into the local database
    ///
    /// The same compile as the top-level `oxy compile` (which stays for the
    /// workspace template's CI gate): walks every recognized YAML/SQL file,
    /// parses it, and writes a `revisions` row plus per-entity rows. `--promote`
    /// makes the new revision the one the workspace serves.
    Compile(compile::CompileArgs),
}

#[derive(Parser, Debug)]
pub struct SeedArgs {
    /// Override the workspace path (defaults to `./examples`).
    #[clap(long)]
    pub workspace_path: Option<std::path::PathBuf>,
    /// Tear down instead of seeding: drops the demo workspace AND the seeded
    /// partner + tenant rows. Leaves the Local org + guest user in place.
    #[clap(long)]
    pub clear: bool,
    /// Skip compiling + promoting the seeded workspaces. Without the compile a
    /// seeded workspace answers `503 needs_recompile` until `oxy dev compile
    /// --promote` runs for it.
    #[clap(long, conflicts_with = "clear")]
    pub no_compile: bool,
    /// Copy the LLM keys each seeded workspace's `config.yml` references from
    /// this shell's environment into its secrets store. Opt-in because the
    /// values are your real credentials: a URL can look local and still reach a
    /// shared database (a port-forward), so the locality check alone is not
    /// consent. `just up` passes it; the database guard still applies.
    #[clap(long, conflicts_with = "clear")]
    pub llm_keys: bool,
    /// Also build the demo fixture app (`local/pulse-demo`): five real builds,
    /// a seven-day release history with the bad release live, seeded traffic
    /// and a few real runs. Local DB only; `just up` passes it.
    #[clap(long, conflicts_with = "clear")]
    pub fixtures: bool,
    /// Only give the existing org with this slug a local workspace (a Ready
    /// `Default` with a minimal `config.yml`, no repository) and print its id
    /// last. For a single-process dev or CI box: the product makes a workspace
    /// only from a GitHub import. Reuses the org's workspace when it has one.
    #[clap(long, conflicts_with_all = ["clear", "no_compile", "llm_keys", "fixtures"])]
    pub org_workspace: Option<String>,
}

/// Run one of [`DevCommand`].
pub async fn run(command: DevCommand, seams: SurfaceSeams) -> Result<(), Box<dyn Error>> {
    match command {
        DevCommand::Seed(seed_args) => {
            ::oxy::sentry_config::add_operation_context("seed", None);
            handle_seed_command(seed_args, seams.example_apps).await?;
        }
        DevCommand::Compile(compile_args) => super::compile_command(compile_args).await,
    }
    Ok(())
}

/// What a renamed top-level spelling prints before it runs, on stderr: `seed
/// --org-workspace` prints its id last on stdout and callers read it.
pub fn renamed_hint(command: &str) -> String {
    format!("`oxy {command}` is now `oxy dev {command}`; the old spelling is kept for one release.")
        .warning()
        .to_string()
}

pub(super) async fn handle_seed_command(
    seed_args: SeedArgs,
    example_apps: Option<ExampleAppsSeam>,
) -> Result<(), OxyError> {
    use seed::*;
    if let Some(slug) = seed_args.org_workspace.as_deref() {
        return seed_org_workspace::seed_org_workspace(slug)
            .await
            .map(|_| ());
    }
    if seed_args.clear {
        // Guard BOTH teardowns UP FRONT. clear_demo has no is_local() gate of its
        // own, so without this it would run (unguarded) before clear_partner_tenants
        // rejected a non-local DB — a partial, guard-bypassing teardown. Refuse
        // first; delete nothing on a remote DB.
        seed_partners::refuse_if_not_local()?;
        clear_demo(example_apps).await?;
        seed_partners::clear_partner_tenants().await
    } else {
        // Seeds the demo workspace AND (folded in) the partner + tenant data —
        // one command, no `--partners` flag — then compiles, and stores LLM keys
        // only when asked to (`--llm-keys`).
        let options = SeedOptions {
            compile: !seed_args.no_compile,
            llm_keys: seed_args.llm_keys,
            fixtures: seed_args.fixtures,
            airhouse: true,
        };
        seed_demo_with(seed_args.workspace_path, options, example_apps).await
    }
}
