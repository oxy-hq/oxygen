//! `oxy validate` — a one-release stub. The command is `oxyc validate` now
//! (`@oxy-hq/cli`, `sdk/cli`): it checks the same files offline, with no Rust
//! binary and no token, and covers what this one did — `config.yml`, every
//! automation's references (`execute_sql.database`, Omni and Looker
//! `integration`) and rules, agentic agents, and `.view.yml` / `.topic.yml`
//! against the generated `view.json` / `topic.json`. It also resolves an
//! agentic agent's `databases:` and `llm.ref`, which this command never did.
//!
//! Customers run this from scripts and CI, so it is not deleted outright: the
//! args still parse, so an old invocation lands here and is told the exact
//! `oxyc` command to type, and the process exits `2` (a usage error). Delete
//! this file and its `SubCommand` arm one release after the `oxyc` verb ships
//! (`internal-docs/oxy-cli-to-oxyc.md`, step 11).

use clap::Parser;

#[derive(Parser, Debug)]
pub struct ValidateArgs {
    /// Validate a specific file instead of all configuration files
    #[clap(long, short)]
    pub file: Option<std::path::PathBuf>,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

/// What the stub prints: the `oxyc` invocation equivalent to `args`.
pub fn moved_message(args: &ValidateArgs) -> String {
    let mut command = String::from("oxyc validate");
    if let Some(file) = &args.file {
        command.push_str(" --file ");
        command.push_str(&shell_quote(&file.to_string_lossy()));
    }
    format!(
        "`oxy validate` has moved to oxyc. From inside your workspace, run:\n\n    {command}\n\n\
         It checks the same files offline and needs no login. It exits 9 when a check \
         fails (this command exited 1). Install oxyc with `npm i -g @oxy-hq/cli` \
         (or run `npx @oxy-hq/cli validate`)."
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

    #[derive(Parser, Debug)]
    struct Probe {
        #[clap(flatten)]
        args: ValidateArgs,
    }

    fn parse(argv: &[&str]) -> ValidateArgs {
        Probe::try_parse_from(std::iter::once("oxy").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
            .args
    }

    /// The invocations the docs and skills used: bare, `--file`, and `-f`.
    #[test]
    fn old_invocations_still_parse() {
        assert!(parse(&[]).file.is_none());
        assert_eq!(
            parse(&["--file", "agents/sales.agentic.yml"]).file,
            Some("agents/sales.agentic.yml".into())
        );
        assert_eq!(
            parse(&["-f", "semantics/views/orders.view.yml"]).file,
            Some("semantics/views/orders.view.yml".into())
        );
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        assert!(moved_message(&parse(&[])).contains("\n    oxyc validate\n"));
        assert!(
            moved_message(&parse(&["-f", "a.automation.yml"]))
                .contains("\n    oxyc validate --file a.automation.yml\n")
        );
        assert!(
            moved_message(&parse(&["--file", "my views/o.view.yml"]))
                .contains("\n    oxyc validate --file 'my views/o.view.yml'\n")
        );
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
