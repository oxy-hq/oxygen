//! `search`: case-insensitive substring match over paths and lines.

use std::sync::Arc;

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::react::files::{FileKind, WorkspaceFiles};

const MAX_MATCHES: usize = 50;

pub struct SearchTool {
    files: Arc<WorkspaceFiles>,
}

impl SearchTool {
    pub fn new(files: Arc<WorkspaceFiles>) -> Self {
        Self { files }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "search",
            description: "Find text in the workspace files (views, topics, SQL, docs). \
                Case-insensitive substring, not a regex; matches file paths and lines. \
                Returns `path:line: text`.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Text to find"},
                    "kind": {"type": "string", "enum": ["view", "topic", "sql", "doc"],
                        "description": "Only search files of this kind"}
                },
                "required": ["pattern"]
            }),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, _cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| fail("search needs a non-empty `pattern`"))?;
        let kind =
            match args.get("kind").and_then(Value::as_str) {
                None => None,
                Some(k) => Some(FileKind::parse(k).ok_or_else(|| {
                    fail(&format!("unknown kind {k}; use view, topic, sql or doc"))
                })?),
            };
        let hits = self.matches(&pattern.to_lowercase(), kind);
        Ok(render(pattern, &hits, &self.files))
    }
}

impl SearchTool {
    fn matches(&self, needle: &str, kind: Option<FileKind>) -> Vec<String> {
        let mut hits = Vec::new();
        for e in self
            .files
            .entries()
            .filter(|e| kind.is_none_or(|k| k == e.kind))
        {
            if e.path.to_lowercase().contains(needle) {
                hits.push(format!("{}: (path match)", e.path));
            }
            for (i, line) in e.content.lines().enumerate() {
                if line.to_lowercase().contains(needle) {
                    hits.push(format!("{}:{}: {}", e.path, i + 1, line.trim_end()));
                }
            }
        }
        hits
    }
}

fn render(pattern: &str, hits: &[String], files: &WorkspaceFiles) -> ToolReply {
    if hits.is_empty() {
        let mut kinds: Vec<&str> = files.paths().iter().map(|(_, k)| k.label()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let text = format!(
            "no matches for \"{pattern}\"; kinds that exist: {}",
            kinds.join(", ")
        );
        return ToolReply::new(text, json!({"matches": 0}));
    }
    let mut text = hits[..hits.len().min(MAX_MATCHES)].join("\n");
    if hits.len() > MAX_MATCHES {
        text.push_str(&format!(
            "\n… {} more matches (narrow the pattern)",
            hits.len() - MAX_MATCHES
        ));
    }
    ToolReply::new(text, json!({"matches": hits.len()}))
}

fn fail(msg: &str) -> ToolFailure {
    ToolFailure {
        error: msg.to_owned(),
        hint: None,
        candidates: Vec::new(),
    }
}
