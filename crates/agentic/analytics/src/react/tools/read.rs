//! `read`: numbered lines of one workspace file.

use std::sync::Arc;

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::react::files::{FileEntry, WorkspaceFiles};

const DEFAULT_LIMIT: usize = 400;
const MAX_CANDIDATES: usize = 5;

pub struct ReadTool {
    files: Arc<WorkspaceFiles>,
}

impl ReadTool {
    pub fn new(files: Arc<WorkspaceFiles>) -> Self {
        Self { files }
    }

    /// An exact path, else a bare name matching exactly one file stem.
    fn resolve(&self, path: &str) -> Option<&FileEntry> {
        if let Some(e) = self.files.get(path) {
            return Some(e);
        }
        let mut hits = self.files.entries().filter(|e| stem(&e.path) == path);
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    fn not_found(&self, path: &str) -> ToolFailure {
        let mut scored: Vec<(f64, &str)> = self
            .files
            .paths()
            .into_iter()
            .map(|(p, _)| (strsim::jaro_winkler(path, p), p))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        ToolFailure {
            error: format!("no file {path}"),
            hint: Some("use search to find files".into()),
            candidates: scored
                .into_iter()
                .take(MAX_CANDIDATES)
                .map(|(_, p)| p.to_owned())
                .collect(),
        }
    }
}

/// File name up to the first `.` (`financial/account.view.yml` -> `account`).
fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name)
}

fn int_arg(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map_or(default, |n| n as usize)
}

#[async_trait]
impl Tool for ReadTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read",
            description: "Read a workspace file with line numbers. `path` is a path from the \
                file list, or a bare name (`account`) when only one file has it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer", "description": "First line, 1-based (default 1)"},
                    "limit": {"type": "integer", "description": "Max lines (default 400)"}
                },
                "required": ["path"]
            }),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, _cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        let entry = self.resolve(path).ok_or_else(|| self.not_found(path))?;
        let offset = int_arg(&args, "offset", 1).max(1);
        let limit = int_arg(&args, "limit", DEFAULT_LIMIT).max(1);
        let lines: Vec<&str> = entry.content.lines().collect();
        let total = lines.len();
        if offset > total.max(1) {
            return Err(ToolFailure {
                error: format!("offset {offset} is past the end ({total} lines)"),
                hint: None,
                candidates: Vec::new(),
            });
        }
        let start = offset - 1;
        let end = start.saturating_add(limit).min(total);
        let mut text: String = (start..end)
            .map(|i| format!("{}\t{}\n", i + 1, lines[i]))
            .collect();
        if start > 0 || end < total {
            text.push_str(&format!("… lines {}-{} of {} shown", start + 1, end, total));
        }
        Ok(ToolReply::new(
            text,
            json!({"path": entry.path, "lines": total}),
        ))
    }
}
