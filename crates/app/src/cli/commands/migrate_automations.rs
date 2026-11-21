//! `oxy migrate-automations` — a one-release stub. The command is
//! `oxyc migrate-automations` now (`@oxy-hq/cli`, `sdk/cli`), a pure local file
//! rewrite that needs no Rust binary.
//!
//! Customers run this from scripts, so it is not deleted outright: the args
//! still parse, so an old invocation lands here and is told the exact `oxyc`
//! command to type, and the process exits `2` (a usage error). Delete this file
//! and its `SubCommand` arm one release after the `oxyc` verb ships
//! (`internal-docs/oxy-cli-to-oxyc.md`, step 4).

use clap::Parser;

#[derive(Parser, Debug)]
pub struct MigrateAutomationsArgs {
    /// Project directory to migrate (defaults to the current directory).
    #[clap(default_value = ".")]
    pub path: String,

    /// Preview the changes without modifying any files.
    #[clap(long, default_value_t = false)]
    pub dry_run: bool,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

/// What the stub prints: the `oxyc` invocation equivalent to `args`.
pub fn moved_message(args: &MigrateAutomationsArgs) -> String {
    let mut command = String::from("oxyc migrate-automations");
    if args.path != "." {
        command.push(' ');
        command.push_str(&shell_quote(&args.path));
    }
    if args.dry_run {
        command.push_str(" --dry-run");
    }
    format!(
        "`oxy migrate-automations` has moved to oxyc. Run:\n\n    {command}\n\n\
         Install it with `npm i -g @oxy-hq/cli` (or run `npx @oxy-hq/cli migrate-automations`)."
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
        args: MigrateAutomationsArgs,
    }

    fn parse(argv: &[&str]) -> MigrateAutomationsArgs {
        Probe::try_parse_from(std::iter::once("oxy").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
            .args
    }

    #[test]
    fn old_invocations_still_parse() {
        let a = parse(&[]);
        assert_eq!((a.path.as_str(), a.dry_run), (".", false));
        let a = parse(&["--dry-run"]);
        assert_eq!((a.path.as_str(), a.dry_run), (".", true));
        let a = parse(&["my-project", "--dry-run"]);
        assert_eq!((a.path.as_str(), a.dry_run), ("my-project", true));
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        assert!(moved_message(&parse(&[])).contains("\n    oxyc migrate-automations\n"));
        assert!(
            moved_message(&parse(&["proj", "--dry-run"]))
                .contains("\n    oxyc migrate-automations proj --dry-run\n")
        );
        assert!(
            moved_message(&parse(&["my proj"]))
                .contains("\n    oxyc migrate-automations 'my proj'\n")
        );
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
