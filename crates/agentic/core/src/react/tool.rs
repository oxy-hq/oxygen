//! The tool contract: what a react loop may call.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolDef;

/// What a tool does to the world; the loop schedules on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// No side effects; safe to run in parallel with other reads.
    Read,
    /// Mutates state; runs alone, in order.
    Write,
    /// Pauses the run for human input. By convention the tool puts
    /// `{"question": str, "suggestions": [str]}` in `ToolReply.value`.
    Suspend,
}

/// Storage for query results, so tools pass ids rather than rows. Implemented
/// by the runtime's result store.
pub trait ResultHandle: Send + Sync {
    /// Stores a result and returns its id.
    fn put(&self, sql: String, columns: Vec<String>, rows: Vec<Vec<Value>>) -> String;
    fn rows(&self, id: &str) -> Option<Arc<Vec<Vec<Value>>>>;
    fn sql_for(&self, id: &str) -> Option<String>;
    /// Registers an id recorded in an earlier task whose rows are gone; only
    /// its SQL is known. The default ignores it (a store with no memory).
    fn restore(&self, _id: &str, _sql: String) {}
}

/// Per-call context handed to a tool.
pub struct ToolCx {
    pub run_id: String,
    pub call_id: String,
    pub cancel: CancellationToken,
    pub results: Arc<dyn ResultHandle>,
}

/// A successful tool result: `text` is what the model reads, `value` is
/// structured data for the UI and the loop. Only `result_id` and `sql` (when
/// both are strings) are persisted on the event row; anything else, rows and
/// previews included, stays out of the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolReply {
    pub text: String,
    pub value: Value,
}

/// A failed tool call, rendered back to the model so it can correct itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolFailure {
    pub error: String,
    pub hint: Option<String>,
    pub candidates: Vec<String>,
}

impl ToolFailure {
    /// The text the model reads: the error, then the hint and candidates.
    pub fn to_model_text(&self) -> String {
        let mut out = self.error.clone();
        if let Some(h) = &self.hint {
            out.push_str("\nHint: ");
            out.push_str(h);
        }
        if !self.candidates.is_empty() {
            out.push_str("\nDid you mean: ");
            out.push_str(&self.candidates.join(", "));
        }
        out
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    fn effect(&self) -> Effect;
    fn parallel_safe(&self) -> bool {
        matches!(self.effect(), Effect::Read)
    }
    async fn call(&self, args: Value, cx: &ToolCx) -> Result<ToolReply, ToolFailure>;
}

/// A system prompt and the tools it describes.
pub struct Toolset {
    pub system_prompt: String,
    pub tools: Vec<Arc<dyn Tool>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolDef;
    use serde_json::Value;

    struct T(Effect);
    #[async_trait::async_trait]
    impl Tool for T {
        fn def(&self) -> ToolDef {
            ToolDef::default()
        }
        fn effect(&self) -> Effect {
            self.0
        }
        async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
            unreachable!()
        }
    }

    #[test]
    fn parallel_safe_defaults_to_reads_only() {
        assert!(T(Effect::Read).parallel_safe());
        assert!(!T(Effect::Write).parallel_safe());
        assert!(!T(Effect::Suspend).parallel_safe());
    }

    #[test]
    fn tool_failure_text_names_the_hint_and_candidates() {
        let f = ToolFailure {
            error: "unknown column revenu".into(),
            hint: Some("check the view".into()),
            candidates: vec!["revenue".into()],
        };
        let t = f.to_model_text();
        for s in ["unknown column revenu", "check the view", "revenue"] {
            assert!(t.contains(s), "{t}");
        }
    }
}
