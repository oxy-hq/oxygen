//! The metric-tree tools: `explain_metric`, `find_opportunities`,
//! `metric_sensitivity` and `predict_impact`. Each one is the FSM's tool of
//! the same name (`crate::tools::execute_metric_tree_tool`) behind the react
//! `Tool` trait, with a description written for this toolset. The names are
//! the FSM's, so an eval case's `expect.tools` names the same call on both
//! runtimes.
//!
//! Offered only when the host gives a [`MetricTreeRunner`]
//! ([`crate::react::run::ReactAnalytics::with_metric_tree`]), as on the FSM.
//! The runner loads the workspace's whole compiled semantic model and runs the
//! SQL airlayer generates on the platform's connectors; the model writes no
//! SQL here, and no file text comes back, only the op's numbers.

use std::sync::Arc;

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::{ToolDef, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::metric_tree_runner::MetricTreeRunner;
use crate::tools::{execute_metric_tree_tool, metric_tree_tools};

/// Characters of a result the model reads before the reply says it was cut.
pub const MAX_TREE_REPLY_CHARS: usize = 20_000;

const TARGET_DESC: &str = "The measure as `view.measure`, for example `orders.net_revenue`. \
    Find it with `search` or `read` in the view files.";

const EXPLAIN_DESC: &str = "Explain why a measure changed between two periods. Use it for \
    questions like \"why did revenue drop in June?\" or \"what is behind the change in \
    orders from May to June?\". It splits the change into the parts of the measure and the \
    segments that caused it, and returns how much of the change each one explains. Give \
    both periods as dates.";

const OPPORTUNITY_DESC: &str = "Find where a measure can grow. Use it for questions like \
    \"where can we grow revenue?\" or \"which segments underperform?\". For each dimension \
    of the measure's view, it compares every segment with a benchmark segment and returns \
    the segments with the largest gap, with what closing the gap is worth.";

const SENSITIVITY_DESC: &str = "List what drives a measure, strongest first. Use it for \
    questions like \"what drives revenue?\". It reads the drivers the semantic model \
    declares; it does not query data.";

const PREDICT_DESC: &str = "Estimate what a change to one or more measures does to the \
    measures built from them. Use it for questions like \"if order volume rises 10%, what \
    happens to revenue?\". Give each change as a measure and a delta. It does not query \
    data.";

/// The react description for an FSM metric-tree tool name; `None` for a tool
/// this module has not described yet, which keeps the FSM's.
fn description(name: &str) -> Option<&'static str> {
    match name {
        "explain_metric" => Some(EXPLAIN_DESC),
        "find_opportunities" => Some(OPPORTUNITY_DESC),
        "metric_sensitivity" => Some(SENSITIVITY_DESC),
        "predict_impact" => Some(PREDICT_DESC),
        _ => None,
    }
}

/// The FSM's def for `fsm`, described for react: plain words, `target` found
/// with `search` (react has no `search_catalog`), and `deep` optional (the
/// executor defaults it to `false`).
fn react_def(mut fsm: ToolDef) -> ToolDef {
    if let Some(text) = description(fsm.name) {
        fsm.description = text;
    }
    let params = &mut fsm.parameters;
    if let Some(target) = params.pointer_mut("/properties/target/description") {
        *target = json!(TARGET_DESC);
    }
    if let Some(required) = params.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|r| r != "deep");
    }
    fsm.strict = false;
    fsm
}

/// One metric-tree tool over the host's runner.
pub struct MetricTreeTool {
    def: ToolDef,
    runner: Arc<dyn MetricTreeRunner>,
}

impl MetricTreeTool {
    /// The four tools, in the FSM's order, sharing `runner`.
    pub fn all(runner: Arc<dyn MetricTreeRunner>) -> Vec<Arc<dyn Tool>> {
        metric_tree_tools()
            .into_iter()
            .map(|def| {
                Arc::new(Self {
                    def: react_def(def),
                    runner: runner.clone(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

#[async_trait]
impl Tool for MetricTreeTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    async fn call(&self, args: Value, _cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let name = self.def.name;
        let value = execute_metric_tree_tool(name, args, self.runner.clone())
            .await
            .map_err(|e| failure(name, e))?;
        Ok(tree_reply(value))
    }
}

/// The op's result as compact JSON, cut at [`MAX_TREE_REPLY_CHARS`] with the
/// cut stated. `value` keeps no rows: the tree's numbers are in the text.
pub(crate) fn tree_reply(value: Value) -> ToolReply {
    let full = value.to_string();
    let total = full.chars().count();
    let text = if total > MAX_TREE_REPLY_CHARS {
        let head: String = full.chars().take(MAX_TREE_REPLY_CHARS).collect();
        format!("{head}\n… result cut at {MAX_TREE_REPLY_CHARS} of {total} characters")
    } else {
        full
    };
    ToolReply::new(text, json!({"truncated": total > MAX_TREE_REPLY_CHARS}))
}

/// A failed op, with what to try next.
fn failure(name: &str, e: ToolError) -> ToolFailure {
    let hint = match &e {
        // `predict_impact`'s lever-conflict refusal: the arguments are
        // well formed, so the schema is not what to fix.
        ToolError::BadParams(msg) if msg.contains("ambiguous scenario") => {
            "change either a measure or the measure upstream of it, not both, then call \
             again"
                .to_string()
        }
        ToolError::BadParams(_) => format!(
            "fix what the error names, then call again; check the arguments against the \
             `{name}` schema, and use `view.member` ids from the view files for measures \
             and time dimensions"
        ),
        _ => "check that the measure and time dimension exist in the view files (use \
              `search`), or answer with `query` instead"
            .to_string(),
    };
    ToolFailure {
        error: format!("{name} failed: {e}"),
        hint: Some(hint),
        candidates: Vec::new(),
    }
}
