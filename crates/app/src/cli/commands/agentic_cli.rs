//! `oxy agentic` — a one-release stub. The commands are `oxyc agentic …` now
//! (`@oxy-hq/cli`, `sdk/cli`), over the workspace's analytics routes:
//! `POST /api/{workspace_id}/analytics/runs` and its SSE
//! `…/runs/{id}/events`, `…/runs/{id}/answer`, `…/runs/{id}/cancel`, and the
//! coordinator reads `…/analytics/coordinator/{runs, active-runs, runs/{id}/tree}`.
//! The server runs the DEPLOYED agent (its compiled revision), not a file in a
//! local checkout, and the laptop needs an `oxyc` login instead of
//! `OXY_DATABASE_URL` and the LLM keys.
//!
//! Customers run this (skills `oxy-agents` and `oxy-evals`, `examples/config.yml`),
//! so it is not deleted outright: the args still parse, so an old invocation
//! lands here and is told the `oxyc` command to type, and the process exits `2`
//! (a usage error). Delete this file and its `SubCommand` arm one release after
//! the `oxyc` verbs ship (`internal-docs/oxy-cli-to-oxyc.md`, step 9).

use clap::Parser;
use uuid::Uuid;

#[derive(Parser, Debug)]
pub struct AgenticArgs {
    #[clap(subcommand)]
    pub command: AgenticCommand,
}

#[derive(Parser, Debug)]
pub enum AgenticCommand {
    /// Moved to `oxyc agentic run` (prints the command and exits 2)
    Run(RunArgs),
    /// Moved to `oxyc agentic list` (prints the command and exits 2)
    List(ListArgs),
    /// Moved to `oxyc agentic events` (prints the command and exits 2)
    Events(EventsArgs),
    /// Moved to `oxyc agentic answer` (prints the command and exits 2)
    Answer(AnswerArgs),
    /// Moved to `oxyc agentic cancel` (prints the command and exits 2)
    Cancel(CancelArgs),
    /// Moved to `oxyc agentic inspect` (prints the command and exits 2)
    Inspect(InspectArgs),
    /// Moved to `oxyc agentic status` (prints the command and exits 2)
    Status(StatusArgs),
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Agent config name, relative to the workspace root
    pub config: String,
    /// Question to ask the pipeline
    #[clap(long, short)]
    pub question: String,
    /// Thinking mode: "auto" (default) or "extended_thinking"
    #[clap(long, default_value = "auto")]
    pub thinking_mode: String,
    /// Output structured JSONL instead of pretty terminal
    #[clap(long)]
    pub json: bool,
    /// Workspace to run as
    #[clap(long)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Maximum number of runs to show
    #[clap(long, default_value = "20")]
    pub limit: u64,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct EventsArgs {
    /// Run ID to replay events for
    pub run_id: String,
    /// Start after this sequence number
    #[clap(long, default_value = "-1")]
    pub after_seq: i64,
    /// Output JSONL
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct AnswerArgs {
    pub run_id: String,
    /// The answer text
    #[clap(long, short)]
    pub answer: String,
}

#[derive(Parser, Debug)]
pub struct CancelArgs {
    pub run_id: String,
}

#[derive(Parser, Debug)]
pub struct InspectArgs {
    pub run_id: String,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {
    /// Watch mode: poll every N seconds (0 = single shot)
    #[clap(long, short, default_value = "0")]
    pub watch: u64,
    /// Only show active (running/suspended) runs
    #[clap(long)]
    pub active: bool,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

/// What the stub prints: the `oxyc` invocation equivalent to `args`. Every
/// verb needs the deployed workspace's id, so `--workspace` is always there.
pub fn moved_message(args: &AgenticArgs) -> String {
    let (verb, workspace) = oxyc_command(&args.command);
    let workspace = workspace
        .map(|id| id.to_string())
        .unwrap_or_else(|| "<workspace-id>".to_string());
    format!(
        "`oxy agentic` has moved to oxyc. Run:\n\n    \
         oxyc agentic {verb} --workspace {workspace}\n\n\
         The server runs the DEPLOYED agent (the workspace's compiled revision), not a \
         file in this checkout: push a change before running it.\n\
         Install it with `npm i -g @oxy-hq/cli` (or run `npx @oxy-hq/cli agentic`)."
    )
}

/// The verb and its arguments, and the workspace when the old command named one.
fn oxyc_command(command: &AgenticCommand) -> (String, Option<Uuid>) {
    match command {
        AgenticCommand::Run(a) => (run_command(a), a.workspace_id),
        AgenticCommand::List(a) => {
            let mut s = String::from("list");
            if a.limit != 20 {
                s.push_str(&format!(" --limit {}", a.limit));
            }
            (with_json(s, a.json), None)
        }
        AgenticCommand::Events(a) => {
            let mut s = format!("events {}", shell_quote(&a.run_id));
            if a.after_seq >= 0 {
                s.push_str(&format!(" --after-seq {}", a.after_seq));
            }
            (with_json(s, a.json), None)
        }
        AgenticCommand::Answer(a) => (
            format!(
                "answer {} --answer {}",
                shell_quote(&a.run_id),
                shell_quote(&a.answer)
            ),
            None,
        ),
        AgenticCommand::Cancel(a) => (format!("cancel {}", shell_quote(&a.run_id)), None),
        AgenticCommand::Inspect(a) => (
            with_json(format!("inspect {}", shell_quote(&a.run_id)), a.json),
            None,
        ),
        AgenticCommand::Status(a) => (status_command(a), None),
    }
}

fn run_command(a: &RunArgs) -> String {
    // The route takes the agent's path inside the workspace; an absolute path
    // on this machine means nothing to the deployment.
    let config = if std::path::Path::new(&a.config).is_absolute() {
        "<path/inside/the/workspace.agentic.yml>".to_string()
    } else {
        shell_quote(&a.config)
    };
    let mut s = format!("run {config} -q {}", shell_quote(&a.question));
    if a.thinking_mode == "extended_thinking" {
        s.push_str(" --thinking-mode extended_thinking");
    }
    with_json(s, a.json)
}

fn status_command(a: &StatusArgs) -> String {
    let mut s = String::from("status");
    if a.watch > 0 {
        s.push_str(&format!(" --watch {}", a.watch));
    }
    if a.active {
        s.push_str(" --active");
    }
    with_json(s, a.json)
}

fn with_json(mut s: String, json: bool) -> String {
    if json {
        s.push_str(" --json");
    }
    s
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

    fn parse(argv: &[&str]) -> AgenticArgs {
        AgenticArgs::try_parse_from(std::iter::once("agentic").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
    }

    fn command_line(argv: &[&str]) -> String {
        let msg = moved_message(&parse(argv));
        msg.lines()
            .find(|l| l.trim_start().starts_with("oxyc agentic"))
            .expect("the message names the oxyc command")
            .trim()
            .to_string()
    }

    /// The invocations the `oxy-agents` / `oxy-evals` skills and
    /// `examples/config.yml` taught.
    #[test]
    fn old_invocations_still_parse() {
        let AgenticCommand::Run(a) = parse(&[
            "run",
            "agents/sales.agentic.yml",
            "-q",
            "Revenue by region?",
            "--json",
        ])
        .command
        else {
            panic!("run")
        };
        assert_eq!(
            (a.config.as_str(), a.question.as_str(), a.json),
            ("agents/sales.agentic.yml", "Revenue by region?", true)
        );
        assert!(matches!(
            parse(&["list", "--limit", "5"]).command,
            AgenticCommand::List(_)
        ));
        assert!(matches!(
            parse(&["events", "r-1", "--after-seq", "3", "--json"]).command,
            AgenticCommand::Events(_)
        ));
        assert!(matches!(
            parse(&["answer", "r-1", "-a", "yes"]).command,
            AgenticCommand::Answer(_)
        ));
        assert!(matches!(
            parse(&["cancel", "r-1"]).command,
            AgenticCommand::Cancel(_)
        ));
        assert!(matches!(
            parse(&["inspect", "r-1", "--json"]).command,
            AgenticCommand::Inspect(_)
        ));
        assert!(matches!(
            parse(&["status", "-w", "2", "--active", "--json"]).command,
            AgenticCommand::Status(_)
        ));
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        assert_eq!(
            command_line(&["run", "oltp", "-q", "how many orders are open?", "--json"]),
            "oxyc agentic run oltp -q 'how many orders are open?' --json --workspace <workspace-id>"
        );
        assert_eq!(
            command_line(&[
                "run",
                "a",
                "-q",
                "q",
                "--thinking-mode",
                "extended_thinking",
                "--workspace-id",
                "11111111-2222-3333-4444-555555555555",
            ]),
            "oxyc agentic run a -q q --thinking-mode extended_thinking \
             --workspace 11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(
            command_line(&["run", "/abs/a.agentic.yml", "-q", "q"]),
            "oxyc agentic run <path/inside/the/workspace.agentic.yml> -q q --workspace <workspace-id>"
        );
        assert_eq!(
            command_line(&["list"]),
            "oxyc agentic list --workspace <workspace-id>"
        );
        assert_eq!(
            command_line(&["events", "r-1", "--after-seq", "3", "--json"]),
            "oxyc agentic events r-1 --after-seq 3 --json --workspace <workspace-id>"
        );
        assert_eq!(
            command_line(&["answer", "r-1", "-a", "it's Q3"]),
            r"oxyc agentic answer r-1 --answer 'it'\''s Q3' --workspace <workspace-id>"
        );
        assert_eq!(
            command_line(&["status", "-w", "2", "--active"]),
            "oxyc agentic status --watch 2 --active --workspace <workspace-id>"
        );
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
