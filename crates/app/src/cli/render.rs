//! `CliRenderer` — implementation of [`oxy::ClientRenderer`] for the
//! `oxy ask` command-line surface.
//!
//! This is intentionally a low-fidelity consumer: text streams to stdout
//! as it arrives, structured events become bracketed labels, and reasoning
//! is dimmed (subtle but visible) so debug runs make it obvious *what* the
//! agent is doing without burying the actual answer.
//!
//! Trade-offs vs. the legacy `AgentCLIHandler` (gone; `oxy run` itself moved to `oxyc run`):
//!
//! - Loses syntax-highlighted SQL and ASCII tables — those came from
//!   intercepting raw `Output::SQL` / `Output::Table` at the
//!   `EventHandler` layer. The structured `AnswerContent` stream gives us
//!   markdown-formatted SQL fences and markdown tables instead. Acceptable
//!   for `oxy ask` since the command's purpose is "ask a question and read
//!   the answer", not "operate a query workbench".
//! - Gains a uniform pipeline with the web + Slack consumers — every new
//!   surface implements the same trait, no special handler types.

use std::io::Write;

use async_trait::async_trait;
use oxy::ClientRenderer;
use oxy::exec_types::Usage;
use oxy::exec_types::event::{ArtifactKind, Step};
use oxy::theme::StyledText;
use oxy::types::ArtifactValue;

#[derive(Default)]
pub struct CliRenderer;

impl CliRenderer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ClientRenderer for CliRenderer {
    type Output = ();

    async fn on_text(&mut self, content: &str) {
        // Stream text immediately rather than line-buffering so the
        // user sees output as the LLM emits it.
        print!("{content}");
        let _ = std::io::stdout().flush();
    }

    async fn on_reasoning_started(&mut self, _id: &str) {
        println!("\n{}", "  ▼ reasoning".secondary());
    }

    async fn on_reasoning_chunk(&mut self, _id: &str, delta: &str) {
        // Indent reasoning output and render it in the secondary palette
        // so it's clearly subordinate to the agent's actual answer.
        for line in delta.split_inclusive('\n') {
            print!("{}", format!("  {line}").secondary());
        }
        let _ = std::io::stdout().flush();
    }

    async fn on_reasoning_done(&mut self, _id: &str) {
        println!("\n{}", "  ▲".secondary());
    }

    async fn on_chart(&mut self, chart_src: &str) {
        println!("\n{}", format!("[chart: {chart_src}]").primary());
    }

    async fn on_artifact_started(
        &mut self,
        _id: &str,
        title: &str,
        kind: &ArtifactKind,
        is_verified: bool,
    ) {
        let verified = if is_verified { " ✓" } else { "" };
        println!(
            "\n{}",
            format!("[{} · {title}{verified}]", kind.kind_label()).primary()
        );
    }

    async fn on_artifact_value(&mut self, _id: &str, _value: &ArtifactValue) {
        // The structured value isn't very useful in a debug CLI — the
        // accompanying Text events carry the same info in markdown form.
    }

    async fn on_artifact_done(&mut self, _id: &str, error: Option<&str>) {
        if let Some(err) = error {
            println!("{}", format!("[artifact failed: {err}]").error());
        }
    }

    async fn on_step_started(&mut self, step: &Step) {
        let label = match step.objective.as_deref() {
            Some(obj) if !obj.trim().is_empty() => format!("▶ {} — {obj}", step.kind),
            _ => format!("▶ {}", step.kind),
        };
        println!("\n{}", label.secondary());
    }

    async fn on_step_finished(&mut self, _step_id: &str, error: Option<&str>) {
        match error {
            Some(err) => println!("{}", format!("✗ {err}").error()),
            None => println!("{}", "✔".secondary()),
        }
    }

    async fn on_error(&mut self, message: &str) {
        eprintln!("{}", message.error());
    }

    async fn on_usage(&mut self, _usage: &Usage) {
        // Usage metadata is debug-noisy on the CLI; leave it to logs.
    }

    async fn finalize(self) -> Self::Output {
        // Trailing newline so the shell prompt lands on a fresh line.
        println!();
    }
}

trait ArtifactKindLabel {
    fn kind_label(&self) -> &'static str;
}

impl ArtifactKindLabel for ArtifactKind {
    fn kind_label(&self) -> &'static str {
        match self {
            ArtifactKind::Workflow { .. } => "workflow",
            ArtifactKind::Agent { .. } => "agent",
            ArtifactKind::ExecuteSQL { .. } => "execute_sql",
            ArtifactKind::SemanticQuery {} => "semantic_query",
            ArtifactKind::OmniQuery { .. } => "omni_query",
            ArtifactKind::LookerQuery { .. } => "looker_query",
            ArtifactKind::SandboxApp { .. } => "sandbox_app",
        }
    }
}
