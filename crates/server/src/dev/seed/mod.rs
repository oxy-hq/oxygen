//! `oxy dev seed`: the demo workspace, partners, tenants and platform grants,
//! written straight into a local database. Up from `oxy-app`'s
//! `cli/commands/seed*` in decompose SD (2026-11-24), and compiled, like the
//! rest of `dev`, only with the `dev-tools` feature, so a release binary cannot
//! seed (`internal-docs/oxy-cli-to-oxyc.md`, "Dev tooling (class d)").
//!
//! The custom-app half of the seed (the example apps, their storage figures,
//! the demo fixture) is `oxy-api-custom-apps`', reached through the
//! `ExampleAppsSeam` the composition root builds. What that half needs to know
//! about the seed goes to it as an argument ([`facts`]), so the surface never
//! depends on the seed. [`fixture`] composes the two for the integration tests.
//!
//! The files keep the names they had in `oxy-app` (`seed.rs`, `seed_partners.rs`,
//! …), so the `super::seed_*` paths between them did not change in the move.

mod seed;
mod seed_airhouse;
mod seed_compile;
mod seed_llm_keys;
mod seed_org_workspace;
mod seed_partners;
mod seed_platform_grants;
mod seed_threads;

use clap::Parser;
use oxy_app_core::seams::{DemoSeedFacts, ExampleAppsSeam};
use oxy_shared::errors::OxyError;

pub use seed::{SeedOptions, clear_demo, seed_demo, seed_demo_with, seeded_persona_emails};

/// What the seed knows that its custom-app half asks
/// (`ExampleAppsSeam::{deploy, fixtures}`): the three lookups
/// `oxy-api-custom-apps` called on `oxy_app::surface::seed` until decompose SD.
pub fn facts() -> DemoSeedFacts {
    DemoSeedFacts {
        is_local_db: seed_partners::is_local_db,
        seeded_workspace_id: seed_partners::seeded_workspace_id,
        unscoped_app_operator_email: seed_platform_grants::unscoped_app_operator_email,
    }
}

/// The fixture the integration tests are built on, in `oxy-api-custom-apps`,
/// `oxy-api-workspace-config` and here: the demo seed with the example apps
/// (`oxy_api_custom_apps::seed::example_apps`). They were that crate's own
/// `seed::{seed_demo, clear_demo}` until decompose SD, when the seed they wrap
/// moved above it.
pub mod fixture {
    use oxy_shared::errors::OxyError;

    pub use super::facts;

    /// Drop the demo workspace row and every seeded example app (`oxy dev seed --clear`).
    pub async fn clear_demo() -> Result<(), OxyError> {
        super::clear_demo(Some(oxy_api_custom_apps::seed::example_apps())).await
    }

    /// The demo seed's rows and the example apps, no compile, no LLM keys and
    /// no Airhouse tenants. [`super::seed_demo`] seeds the rows alone.
    pub async fn seed_demo(workspace_path: Option<std::path::PathBuf>) -> Result<(), OxyError> {
        super::seed_demo_with(
            workspace_path,
            super::SeedOptions {
                compile: false,
                llm_keys: false,
                fixtures: false,
                airhouse: false,
            },
            Some(oxy_api_custom_apps::seed::example_apps()),
        )
        .await
    }
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

pub(crate) async fn handle_seed_command(
    seed_args: SeedArgs,
    example_apps: Option<ExampleAppsSeam>,
) -> Result<(), OxyError> {
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

#[cfg(test)]
mod tests {
    /// The dev sign-in's persona table (`oxy-api-identity`) names the emails
    /// this seed creates. The table is compiled in; the seed is generated from
    /// name pools and partner operator counts. Derive the same answers from the
    /// seed itself. `oxy-api-identity`'s until decompose SD, when the seed it
    /// reads came up here: a surface's `src` may not take `oxy-server`.
    #[test]
    fn seeded_personas_match_the_seed() {
        for (name, email) in super::seeded_persona_emails() {
            let persona = oxy_api_identity::sign_in::dev_login_persona_email(name)
                .unwrap_or_else(|| panic!("seed names unknown persona {name}"));
            assert_eq!(persona, Some(email.as_str()), "{name}");
        }
    }
}
