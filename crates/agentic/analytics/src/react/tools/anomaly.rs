//! The anomaly tools: `list_anomalies`, `detect_anomalies` and
//! `explain_anomaly`. Each one is the FSM's tool of the same name
//! (`crate::tools::execute_anomaly_tool`) behind the react `Tool` trait, with
//! a description written for this toolset. The names are the FSM's, so an
//! eval case's `expect.tools` names the same call on both runtimes.
//!
//! Offered only when the host gives both an [`AnomalyStore`] and a
//! [`MetricTreeRunner`]
//! ([`crate::react::run::ReactAnalytics::with_anomalies`]). The FSM's executor
//! needs both: `detect_anomalies` reads its series and `explain_anomaly` runs
//! its explain through the runner, and the FSM offers these tools only inside
//! its metric-tree loop, which needs a runner. Every read and write is scoped
//! to the platform's workspace.

use std::sync::Arc;

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::{ToolDef, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::metric_tree::tree_reply;
use crate::anomaly_store::AnomalyStore;
use crate::metric_tree_runner::MetricTreeRunner;
use crate::tools::{AnomalyToolContext, anomaly_tools, execute_anomaly_tool};

const MEASURE_DESC: &str = "The measure as `view.measure`, for example \
    `orders.total_order_value`. Find it with `search` or `read` in the view files.";

const TIME_DIMENSION_DESC: &str = "The measure's time dimension as `view.dimension`, for \
    example `orders.order_date`.";

const LIST_DESC: &str = "List the anomalies already found for a measure in a period: the \
    monitors' scheduled scans and earlier `detect_anomalies` calls store them. Use it first \
    for questions like \"were there any anomalies in daily revenue in June?\" or \"is order \
    count behaving unusually?\". Give the measure, its time dimension, the grain and the \
    period as dates. Each anomaly carries its `period_start`: answer only about the ones \
    inside the period you were asked about. An empty list means nothing was stored for \
    that period, not that the period was normal.";

const DETECT_DESC: &str = "Check one bucket of a measure for an anomaly now, and store what \
    it finds. Use it when `list_anomalies` returns nothing for a period you must answer \
    about. It reads the measure's series from `period_start` to `period_end` and scores \
    only the last bucket against the buckets before it, so the period must end at the \
    bucket you ask about and hold at least eight seasonal cycles before it (for a daily \
    measure, about two months of days). With fewer, it returns a message that there is not \
    enough data.";

const EXPLAIN_DESC: &str = "Explain what caused one anomaly: which parts of the measure and \
    which segments moved, compared with the same bucket one seasonal cycle earlier. Give as \
    `anomaly_id` the `id` of an anomaly that `list_anomalies` or `detect_anomalies` returned.";

/// The react description for an FSM anomaly tool name; `None` for a tool
/// this module has not described yet, which keeps the FSM's.
fn description(name: &str) -> Option<&'static str> {
    match name {
        "list_anomalies" => Some(LIST_DESC),
        "detect_anomalies" => Some(DETECT_DESC),
        "explain_anomaly" => Some(EXPLAIN_DESC),
        _ => None,
    }
}

/// The FSM's def for `fsm`, described for react: plain words, and members
/// found with `search` (react has no `search_catalog`).
fn react_def(mut fsm: ToolDef) -> ToolDef {
    if let Some(text) = description(fsm.name) {
        fsm.description = text;
    }
    let params = &mut fsm.parameters;
    if let Some(measure) = params.pointer_mut("/properties/measure/description") {
        *measure = json!(MEASURE_DESC);
    }
    if let Some(td) = params.pointer_mut("/properties/time_dimension/description") {
        *td = json!(TIME_DIMENSION_DESC);
    }
    fsm.strict = false;
    fsm
}

/// What a write to the anomaly store does to the loop. `list_anomalies` only
/// reads. `detect_anomalies` upserts inbox rows and `explain_anomaly` writes
/// its explain cache, so both are writes: they run alone, and a crash that
/// leaves one with no recorded result refuses the resume rather than run it
/// twice.
fn effect_of(name: &str) -> Effect {
    match name {
        "list_anomalies" => Effect::Read,
        _ => Effect::Write,
    }
}

/// The ports every anomaly tool shares.
#[derive(Clone)]
pub struct AnomalyPorts {
    pub store: Arc<dyn AnomalyStore>,
    pub runner: Arc<dyn MetricTreeRunner>,
    /// Scopes every read and write (`PlatformContext::workspace_id`).
    pub workspace_id: Uuid,
}

/// One anomaly tool over the host's store and runner.
pub struct AnomalyTool {
    def: ToolDef,
    ports: AnomalyPorts,
}

impl AnomalyTool {
    /// The three tools, in the FSM's order, sharing `ports`.
    pub fn all(ports: AnomalyPorts) -> Vec<Arc<dyn Tool>> {
        anomaly_tools()
            .into_iter()
            .map(|def| {
                Arc::new(Self {
                    def: react_def(def),
                    ports: ports.clone(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

#[async_trait]
impl Tool for AnomalyTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }

    fn effect(&self) -> Effect {
        effect_of(self.def.name)
    }

    async fn call(&self, args: Value, _cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let name = self.def.name;
        let ctx = AnomalyToolContext {
            workspace_id: self.ports.workspace_id,
            store: self.ports.store.as_ref(),
            runner: self.ports.runner.as_ref(),
        };
        let value = execute_anomaly_tool(name, args, &ctx)
            .await
            .map_err(|e| failure(name, e))?;
        Ok(tree_reply(value))
    }
}

/// A failed call, with what to try next.
fn failure(name: &str, e: ToolError) -> ToolFailure {
    let hint = match (&e, name) {
        (ToolError::BadParams(_), "explain_anomaly") => {
            "pass as `anomaly_id` the `id` of an anomaly that `list_anomalies` or `detect_anomalies` returned"
                .to_string()
        }
        (ToolError::BadParams(_), _) => format!(
            "fix what the error names, then call again; check the arguments against the \
             `{name}` schema, and use `view.member` ids from the view files for the measure \
             and time dimension"
        ),
        (_, "explain_anomaly") => "call `list_anomalies` for a current `id`, or explain the \
             change with `explain_metric` over the anomaly's period"
            .to_string(),
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
