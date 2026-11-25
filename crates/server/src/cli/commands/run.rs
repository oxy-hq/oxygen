//! `oxy run` — a one-release stub. The command is `oxyc run` now
//! (`@oxy-hq/cli`, `sdk/cli`): a `.sql` file's text is sent to
//! `POST /api/{workspace_id}/sql/query` and runs against the deployed
//! database; an automation is started with
//! `POST /api/{workspace_id}/agentic-automations/runs` and runs the definition
//! the deployment has compiled, on the worker fleet. Nothing runs in-process.
//!
//! Customers run this from scripts, so it is not deleted outright: the args
//! still parse, so an old invocation lands here and is told the exact `oxyc`
//! command to type, and the process exits `2` (a usage error). Delete this file
//! and its `SubCommand` arm one release after the `oxyc` verb ships
//! (`internal-docs/oxy-cli-to-oxyc.md`, step 10).

use clap::Parser;
use clap::builder::ValueParser;

fn parse_variable(env: &str) -> Result<(String, String), String> {
    env.split_once('=')
        .map(|(var, value)| (var.to_owned(), value.to_owned()))
        .ok_or_else(|| "Invalid variable format. Must be in the form of VAR=VALUE".to_string())
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Path to the file to execute (.sql, .automation.yml, or .procedure.yml)
    pub file: String,

    /// Database connection to use for SQL execution
    #[clap(long)]
    pub database: Option<String>,

    /// Template variables in the format VAR=VALUE
    #[clap(long, short = 'v', value_parser = ValueParser::new(parse_variable), num_args = 1..)]
    pub variables: Vec<(String, String)>,

    /// Reserved; it did nothing.
    pub question: Option<String>,

    /// It did nothing; `oxyc run` has no such flag.
    #[clap(long, default_value_t = false, group = "named")]
    pub retry: bool,

    /// It did nothing; `oxyc run` has no such flag.
    #[clap(long, group = "unnamed", conflicts_with = "named")]
    pub retry_from: Option<String>,

    /// Preview SQL without executing it. `oxyc run` has no dry run.
    #[clap(long, default_value_t = false)]
    pub dry_run: bool,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

fn is_automation(file: &str) -> bool {
    [
        ".automation.yml",
        ".automation.yaml",
        ".procedure.yml",
        ".procedure.yaml",
    ]
    .iter()
    .any(|ext| file.ends_with(ext))
}

/// What the stub prints: the `oxyc` invocation equivalent to `args`, or, for
/// `--dry-run`, why there is none. The workspace is a deployed one now, so the
/// caller fills in its id.
pub fn moved_message(args: &RunArgs) -> String {
    if args.dry_run {
        return "`oxy run --dry-run` has no oxyc equivalent: the deployment's query route \
                cannot dry-run, and `oxyc run` executes the query. Install oxyc with \
                `npm i -g @oxy-hq/cli` and see `oxyc run --help`."
            .to_string();
    }
    let mut command = format!("oxyc run {}", shell_quote(&args.file));
    if let Some(database) = &args.database {
        command.push_str(" --database ");
        command.push_str(&shell_quote(database));
    }
    for (var, value) in &args.variables {
        command.push_str(" -v ");
        command.push_str(&shell_quote(&format!("{var}={value}")));
    }
    command.push_str(" --workspace <workspace-id>");
    let what = if is_automation(&args.file) {
        format!(
            "The deployment runs the automation it has COMPILED, not this file: push it first, \
             or try a branch with `oxyc preview run <branch> procedure {} --workspace \
             <workspace-id>`.",
            shell_quote(&args.file)
        )
    } else {
        "The SQL is read here and runs against the DEPLOYED database of that name; \
         --database is required when the workspace has more than one."
            .to_string()
    };
    format!(
        "`oxy run` has moved to oxyc. From the root of your checkout, run:\n\n    {command}\n\n\
         {what} Install oxyc with `npm i -g @oxy-hq/cli` (or run `npx @oxy-hq/cli run`)."
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
        args: RunArgs,
    }

    fn parse(argv: &[&str]) -> RunArgs {
        Probe::try_parse_from(std::iter::once("oxy").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
            .args
    }

    /// The invocations the docs, the `oxy-automations` skill and the old
    /// integration tests (`crates/app/tests/platform/run.rs`,
    /// `crates/core/tests/integration/semantic_variables.rs`) used.
    #[test]
    fn old_invocations_still_parse() {
        let a = parse(&[
            "data/example_intervals.sql",
            "--database",
            "primary_database",
        ]);
        assert_eq!(a.database.as_deref(), Some("primary_database"));
        let a = parse(&[
            "data/example_weekly_rejected.sql",
            "-v",
            "variable_a=1",
            "variable_b=testalias",
            "variable_c=*",
        ]);
        assert_eq!(a.variables.len(), 3);
        assert_eq!(a.variables[2], ("variable_c".into(), "*".into()));
        let a = parse(&["procedures/x.automation.yml", "--retry"]);
        assert!(a.retry);
        assert!(parse(&["q.sql", "--dry-run"]).dry_run);
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        let sql = moved_message(&parse(&[
            "data/w.sql",
            "--database",
            "local",
            "-v",
            "a=1",
            "b=my value",
        ]));
        assert!(
            sql.contains(
                "\n    oxyc run data/w.sql --database local -v a=1 -v 'b=my value' --workspace <workspace-id>\n"
            ),
            "{sql}"
        );
        assert!(sql.contains("DEPLOYED database"));
        let automation = moved_message(&parse(&["procedures/r.automation.yml", "--retry"]));
        assert!(
            automation.contains(
                "\n    oxyc run procedures/r.automation.yml --workspace <workspace-id>\n"
            ),
            "{automation}"
        );
        assert!(
            automation.contains("oxyc preview run <branch> procedure procedures/r.automation.yml")
        );
    }

    /// `--dry-run` never turns into a command that executes.
    #[test]
    fn dry_run_prints_no_command() {
        let message = moved_message(&parse(&["q.sql", "--database", "local", "--dry-run"]));
        assert!(message.contains("no oxyc equivalent"), "{message}");
        assert!(!message.contains("oxyc run q.sql"), "{message}");
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
