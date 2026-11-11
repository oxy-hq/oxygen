//! The `oxy` command line as a shim: it parses argv and hands the chosen
//! subcommand to whoever owns it. It knows no subcommand, no server and no
//! platform crate.
//!
//! # Owners
//!
//! A crate that runs subcommands declares them as a `#[derive(clap::Subcommand)]`
//! enum, with each command's help text on its variant, and registers the enum
//! together with its run function as an [`Owner`]. The binary (`oxy-server`)
//! lists the owners and calls [`run`]. Help lists the owners' commands in
//! registration order.
//!
//! `C` is whatever the binary hands each owner when it dispatches: for
//! `oxy-server` that is the `SurfaceSeams` it composes. The shim never looks
//! inside it.
//!
//! # Not to be confused with `oxyc`
//!
//! `@oxy-hq/cli` (`oxyc`, `sdk/cli/`) is the TypeScript client for the HTTP
//! API. This crate is the Rust `oxy` binary's argument parser.

use std::error::Error;
use std::future::Future;
use std::pin::Pin;

use clap::{ArgMatches, Args, Command, Subcommand};

/// What an owner's run function returns. `Box<dyn Error>` so the binary's
/// `main` keeps one error path: print, record to Sentry, flush telemetry.
pub type RunResult = Result<(), Box<dyn Error>>;

/// A boxed future of an owner's run.
pub type RunFuture = Pin<Box<dyn Future<Output = RunResult>>>;

/// One crate's subcommands, and how to run whichever of them was picked.
pub struct Owner<C> {
    augment: fn(Command) -> Command,
    has: fn(&str) -> bool,
    run: Box<dyn FnOnce(&ArgMatches, C) -> Result<RunFuture, clap::Error>>,
}

impl<C: 'static> Owner<C> {
    /// Register the subcommands of `S`, run by `run`.
    pub fn of<S: Subcommand + 'static>(run: fn(S, C) -> RunFuture) -> Self {
        Self {
            augment: S::augment_subcommands,
            has: S::has_subcommand,
            run: Box::new(move |matches, ctx| Ok(run(S::from_arg_matches(matches)?, ctx))),
        }
    }
}

// The top-level arguments, shared by every subcommand. A plain comment: clap
// prints a doc comment here as the root command's `about`, above `Usage:`.
#[derive(Args, Debug)]
struct Global {
    /// The question to ask or command to execute
    ///
    /// When no subcommand is provided, this input will be processed
    /// as a question for the default AI agent or as a query suggestion.
    #[clap(default_value = "")]
    input: String,

    /// Output format: 'text' (default) or 'code' for SQL
    ///
    /// Control how results are displayed in the terminal.
    /// Use 'code' for syntax-highlighted SQL output.
    #[clap(long, value_name = "FORMAT")]
    output: Option<String>,
}

/// The command's name, which `--version` prints first. It is `oxy-app` — the
/// crate that defined the parser before this one — and must stay so: the
/// workspace template's `validate.yaml` reads the version out of a release
/// build's `oxy-app version: <x.y.z>` line.
const NAME: &str = "oxy-app";

/// `--version`'s long form: the debug build says so; a release build names
/// its commit and workflow run when CI built it.
fn long_version() -> &'static str {
    let text = if cfg!(debug_assertions) {
        debug_long_version(env!("CARGO_PKG_VERSION"))
    } else {
        release_long_version(
            env!("CARGO_PKG_VERSION"),
            option_env!("GITHUB_SERVER_URL"),
            option_env!("GITHUB_REPOSITORY"),
            option_env!("GITHUB_SHA"),
            option_env!("GITHUB_RUN_ID"),
        )
    };
    Box::leak(text.into_boxed_str())
}

fn debug_long_version(version: &str) -> String {
    format!(
        "version {}, built locally as debug, rust ver {}",
        version,
        rustc_version_runtime::version(),
    )
}

fn release_long_version(
    version: &str,
    server: Option<&str>,
    repo: Option<&str>,
    sha: Option<&str>,
    run_id: Option<&str>,
) -> String {
    let commit_link = match (server, repo, sha) {
        (Some(server), Some(repo), Some(sha)) => {
            format!("{}/{}/commit/{} ({})", server, repo, sha, sha)
        }
        _ => sha.unwrap_or("unknown").to_string(),
    };
    let workflow_link = match (server, repo, run_id) {
        (Some(server), Some(repo), Some(run_id)) => {
            format!("{}/{}/actions/runs/{} ({})", server, repo, run_id, run_id)
        }
        _ => run_id.unwrap_or("unknown").to_string(),
    };
    format!(
        "version: {}\n\
        rust version: {}\n\
        commit: {commit_link}\n\
        workflow url: {workflow_link}\n",
        version,
        rustc_version_runtime::version(),
    )
}

/// The whole command tree: the top-level arguments, then each owner's
/// subcommands in order.
fn command<C>(owners: &[Owner<C>]) -> Command {
    let root = Command::new(NAME)
        .author(env!("CARGO_PKG_AUTHORS"))
        .version(env!("CARGO_PKG_VERSION"))
        .long_version(long_version());
    let root = Global::augment_args(root);
    owners.iter().fold(root, |cmd, owner| (owner.augment)(cmd))
}

/// Parse argv, call `prepare` with the subcommand's name, and run it with its
/// owner. No subcommand prints the help.
///
/// `prepare` runs after parsing and before dispatch, whichever owner the
/// command belongs to: the process-wide setup every command needs (panic hook,
/// Sentry breadcrumb).
pub async fn run<C: 'static>(
    owners: Vec<Owner<C>>,
    ctx: C,
    prepare: impl FnOnce(Option<&str>),
) -> RunResult {
    let matches = command(&owners).get_matches();
    let name = matches.subcommand_name().map(str::to_owned);
    prepare(name.as_deref());
    let Some(name) = name else {
        // Printed from a fresh tree, as before: a parsed one has `bin_name`
        // set from argv, which would change the usage line.
        command(&owners).print_help()?;
        return Ok(());
    };
    let owner = owners
        .into_iter()
        .find(|o| (o.has)(&name))
        .expect("clap only matches a subcommand some owner registered");
    let future = (owner.run)(&matches, ctx).map_err(|e| e.exit())?;
    future.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;

    /// The workspace template (`sdk/cli/template/.github/workflows/validate.yaml`)
    /// reads a release build's version with
    /// `sed -n 's/^oxy-app version: \([0-9][0-9.]*\).*/\1/p'`.
    #[test]
    fn a_release_version_still_reads_as_the_template_expects() {
        let rendered = Command::new(NAME)
            .version("0.5.159")
            .long_version(&*Box::leak(
                release_long_version("0.5.159", None, None, Some("abc"), None).into_boxed_str(),
            ))
            .render_long_version();
        let first = rendered.lines().next().unwrap();
        assert_eq!(first, "oxy-app version: 0.5.159");
    }

    #[derive(Subcommand, Debug, PartialEq)]
    enum First {
        /// a
        Alpha,
    }

    #[derive(Subcommand, Debug, PartialEq)]
    enum Second {
        /// b
        Beta { n: u32 },
    }

    #[test]
    fn owners_list_in_order_and_each_parses_its_own() {
        let owners: Vec<Owner<()>> = vec![
            Owner::of::<First>(|_, _| Box::pin(async { Ok(()) })),
            Owner::of::<Second>(|_, _| Box::pin(async { Ok(()) })),
        ];
        let cmd = command(&owners);
        let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        assert_eq!(names, ["alpha", "beta"]);

        let m = command(&owners)
            .try_get_matches_from(["oxy", "beta", "7"])
            .unwrap();
        assert!(!First::has_subcommand(m.subcommand_name().unwrap()));
        assert_eq!(Second::from_arg_matches(&m).unwrap(), Second::Beta { n: 7 });
    }
}
