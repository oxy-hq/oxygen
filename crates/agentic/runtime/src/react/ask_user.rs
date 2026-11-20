//! The production `ask_user` tool: parks the run on a question for the human.

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

/// Suspends the run with a question and optional suggested answers. The
/// question and suggestions ride in `ToolReply.value`, the convention the
/// dispatcher reads (see `Effect::Suspend`).
pub struct AskUser;

#[async_trait]
impl Tool for AskUser {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "ask_user",
            description: "Ask the user a clarifying question and wait for the answer. Use it \
                only when the request is ambiguous and the available context cannot settle it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "The question to ask."},
                    "suggestions": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional suggested answers."
                    }
                },
                "required": ["question"]
            }),
            ..ToolDef::default()
        }
    }

    fn effect(&self) -> Effect {
        Effect::Suspend
    }

    async fn call(&self, args: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let question = args
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| ToolFailure {
                error: "ask_user needs a question".into(),
                hint: Some("pass a question string".into()),
                candidates: vec![],
            })?
            .to_string();
        let suggestions: Vec<String> = args
            .get("suggestions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let value = json!({ "question": question, "suggestions": suggestions });
        Ok(ToolReply::new(question, value))
    }
}
