//! `oxy looker test` — a one-release stub. The check is
//! `oxyc looker test <name> --workspace <id>` now (`@oxy-hq/cli`, `sdk/cli`),
//! over `POST /api/{workspace_id}/integrations/looker/{name}/test`
//! (`oxy-api-workspace-config`), which reads the deployed workspace's compiled
//! config and its secrets instead of a local checkout and the shell.
//!
//! Customers run this (`docs/build/connect-data/looker.mdx`), so it is not
//! deleted outright: the args still parse, so an old invocation lands here and
//! is told the `oxyc` command to type, and the process exits `2` (a usage
//! error). Delete this file and its `SubCommand` arm one release after the
//! `oxyc` verb ships (`internal-docs/oxy-cli-to-oxyc.md`, step 12).

use clap::Parser;

/// Looker integration commands
#[derive(Parser, Debug)]
pub struct LookerArgs {
    #[clap(subcommand)]
    pub command: LookerCommand,
}

#[derive(Parser, Debug)]
pub enum LookerCommand {
    /// Moved to `oxyc looker test` (prints the command and exits 2)
    ///
    /// Checking a Looker integration's credentials is
    /// `oxyc looker test <name> --workspace <id>` now (`npm i -g @oxy-hq/cli`).
    /// This stub stays for one release so an old invocation is told what to
    /// type instead.
    Test(LookerTestArgs),
}

#[derive(Parser, Debug)]
pub struct LookerTestArgs {
    /// Name of the Looker integration to test
    #[clap(long)]
    pub integration: Option<String>,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

/// What the stub prints: the `oxyc` invocation equivalent to `args`. The new
/// verb checks one integration per call, so a bare `oxy looker test` (which
/// checked every one) gets a `<name>` placeholder.
pub fn moved_message(args: &LookerArgs) -> String {
    let LookerCommand::Test(test) = &args.command;
    let name = test
        .integration
        .as_deref()
        .map(shell_quote)
        .unwrap_or_else(|| "<name>".to_string());
    format!(
        "`oxy looker test` has moved to oxyc. Run:\n\n    \
         oxyc looker test {name} --workspace <workspace-id>\n\n\
         It checks the integration on the deployment, with the credentials in the \
         workspace's secrets.\n\
         Install it with `npm i -g @oxy-hq/cli` (or run `npx @oxy-hq/cli looker test`)."
    )
}

/// Single-quote `s` for a POSIX shell when it holds anything but safe characters.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> LookerArgs {
        LookerArgs::try_parse_from(std::iter::once("looker").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
    }

    #[test]
    fn old_invocations_still_parse() {
        let LookerCommand::Test(a) = parse(&["test"]).command;
        assert_eq!(a.integration, None);
        let LookerCommand::Test(a) = parse(&["test", "--integration", "my_looker"]).command;
        assert_eq!(a.integration.as_deref(), Some("my_looker"));
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        assert!(
            moved_message(&parse(&["test", "--integration", "my_looker"]))
                .contains("\n    oxyc looker test my_looker --workspace <workspace-id>\n")
        );
        assert!(
            moved_message(&parse(&["test"]))
                .contains("\n    oxyc looker test <name> --workspace <workspace-id>\n")
        );
        assert!(
            moved_message(&parse(&["test", "--integration", "my looker"]))
                .contains("\n    oxyc looker test 'my looker' --workspace")
        );
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
