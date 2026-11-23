//! Assembling a react analytics run and closing it out.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_core::events::Event;
use agentic_core::react::{DoneHook, EventSink, ResultHandle, Tool, Toolset, Turn};
use agentic_runtime::react::{
    AskUser, Budgets, MEMORY_TOOLS, MemoryGate, Nudge, REFLECT_PROMPT, Reflect, RunMemory,
    memory_tools, notes_section,
};
use async_trait::async_trait;

use super::citations::{answer_result_id, parse_citations, strip_citations};
use super::files::WorkspaceFiles;
use super::prompt::{ADVISOR_NUDGE, PromptInput, system_prompt};
use super::tools::{QueryDeps, QueryTool, ReadTool, SearchTool};
use crate::events::AnalyticsEvent;
use crate::semantic::SemanticCatalog;

pub struct ReactInputs {
    pub instructions: String,
    pub databases: HashMap<String, Arc<dyn DatabaseConnector>>,
    pub catalog: Option<Arc<SemanticCatalog>>,
    /// The agent's `extra_default_connector`: where `query` goes with no
    /// `database` when there are several.
    pub default_connector: Option<String>,
    pub files: Arc<WorkspaceFiles>,
    /// The agent's `semantic_only`: `query` refuses raw `sql`.
    pub semantic_only: bool,
    /// The agent has an `llm.advisor`: the prompt says when to consult it.
    pub advisor: bool,
}

/// What the loop needs to run one analytics question.
pub struct ReactAnalytics {
    pub toolset: Toolset,
    pub budgets: Budgets,
    /// With an advisor: the loop tells the model to call it after the third
    /// query, or at the first failed one.
    pub nudge: Option<Nudge>,
    /// With memory writes on ([`Self::with_memory`]): the reflection turn.
    pub reflect: Option<Reflect>,
}

/// Added with memory writes: the memory tools are in the toolset for the
/// prompt cache, and refuse a call until the reflection turn.
const MEMORY_TOOLS_RULE: &str = "Do not call `remember` or `forget` while you answer. After \
    the answer, the run may ask you for notes.";

/// Queries after which an answer is worth a reflection turn.
const REFLECT_AFTER_QUERIES: usize = 3;

impl ReactAnalytics {
    pub fn new(inputs: ReactInputs) -> Self {
        let mut dbs: Vec<(String, String)> = inputs
            .databases
            .iter()
            .map(|(name, c)| (name.clone(), c.dialect().as_str().to_string()))
            .collect();
        dbs.sort();
        let system_prompt = system_prompt(PromptInput {
            instructions: &inputs.instructions,
            databases: &dbs,
            files: &inputs.files.paths(),
            semantic_only: inputs.semantic_only,
            advisor: inputs.advisor,
        });
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(SearchTool::new(inputs.files.clone())),
            Arc::new(ReadTool::new(inputs.files.clone())),
            Arc::new(QueryTool::new(QueryDeps {
                connectors: inputs.databases,
                catalog: inputs.catalog,
                default_connector: inputs.default_connector,
                semantic_only: inputs.semantic_only,
            })),
            Arc::new(AskUser),
        ];
        Self {
            toolset: Toolset {
                system_prompt,
                tools,
            },
            budgets: Budgets {
                max_turns: 30,
                max_tokens: 2_000_000,
                max_cost_usd: Some(3.0),
            },
            nudge: inputs.advisor.then(|| Nudge {
                tool: "query".into(),
                after_calls: 3,
                text: ADVISOR_NUDGE.into(),
            }),
            reflect: None,
        }
    }

    /// Adds `memory` to this run: its notes go in the system prompt. When it
    /// takes writes, `remember` and `forget` join the toolset for the whole
    /// run (so the reflection turn reuses the run's prompt cache), and a run
    /// that made [`REFLECT_AFTER_QUERIES`] queries, or had one fail, gets one
    /// reflection turn.
    pub fn with_memory(mut self, memory: RunMemory) -> Self {
        if let Some(section) = notes_section(&memory.notes) {
            self.toolset.system_prompt.push_str("\n\n");
            self.toolset.system_prompt.push_str(&section);
        }
        if let Some(store) = memory.writes {
            let gate = MemoryGate::default();
            (self.toolset.tools).extend(memory_tools(store, &memory.notes, &gate));
            self.toolset.system_prompt.push_str("\n\n");
            self.toolset.system_prompt.push_str(MEMORY_TOOLS_RULE);
            self.reflect = Some(Reflect {
                only: MEMORY_TOOLS.map(String::from).to_vec(),
                query_tool: "query".into(),
                after_calls: REFLECT_AFTER_QUERIES,
                prompt: REFLECT_PROMPT.into(),
                gate,
                quiet: memory.quiet,
            });
        }
        self
    }
}

async fn emit(ev: &EventSink, event: AnalyticsEvent) {
    let row = Event::<AnalyticsEvent>::Domain(event).serialize();
    // A closed sink means the run is being torn down; nothing to tell.
    let _ = ev.send(row).await;
}

/// The first row of every react run.
pub async fn emit_runtime_selected(ev: &EventSink) {
    emit(
        ev,
        AnalyticsEvent::RuntimeSelected {
            runtime: "react".into(),
        },
    )
    .await;
}

/// The loop's done hook: emits `AnswerResult` for the result the answer cites
/// (when its SQL is known) and rewrites the answer text without its markers.
pub struct AnswerFinisher {
    pub results: Arc<dyn ResultHandle>,
}

#[async_trait]
impl DoneHook for AnswerFinisher {
    async fn on_done(&self, answer: &mut Turn, ev: &EventSink) {
        let citations = parse_citations(&answer.text);
        if let Some(id) = answer_result_id(&citations)
            && let Some(sql) = self.results.sql_for(id)
        {
            let result_id = id.to_string();
            emit(ev, AnalyticsEvent::AnswerResult { result_id, sql }).await;
        }
        answer.text = strip_citations(&answer.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::react::{Turn, TurnUsage};
    use agentic_runtime::react::ResultStore;

    #[tokio::test]
    async fn finisher_emits_answer_result_for_the_primary_sql() {
        let store = ResultStore::new();
        let id = store.put("select 1".into(), vec!["a".into()], vec![]);
        assert_eq!(id, "r1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut answer = Turn {
            text: "It is 1 {{result:r1|primary}}.".into(),
            tool_calls: vec![],
            native: serde_json::Value::Null,
            usage: TurnUsage::default(),
        };
        let finisher = AnswerFinisher {
            results: Arc::new(store),
        };
        finisher.on_done(&mut answer, &tx).await;
        assert_eq!(answer.text, "It is 1.");
        drop(tx);
        let (ty, payload) = rx.recv().await.unwrap();
        assert_eq!(ty, "answer_result");
        assert_eq!(
            payload,
            serde_json::json!({"result_id": "r1", "sql": "select 1"})
        );
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn runtime_selected_row_names_react() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        emit_runtime_selected(&tx).await;
        let (ty, payload) = rx.recv().await.unwrap();
        assert_eq!(ty, "runtime_selected");
        assert_eq!(payload, serde_json::json!({"runtime": "react"}));
    }
}
