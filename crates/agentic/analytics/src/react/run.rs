//! Assembling a react analytics run and closing it out.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_core::events::Event;
use agentic_core::react::{DoneHook, EventSink, Tool, Toolset, Turn};
use agentic_runtime::react::{
    AskUser, Budgets, MEMORY_TOOLS, MemoryGate, Nudge, REFLECT_PROMPT, Reflect, ResultStore,
    RunMemory, memory_tools, notes_section,
};
use async_trait::async_trait;

use super::citations::{answer_result_id, parse_citations, strip_citations};
use super::files::WorkspaceFiles;
use super::prompt::{ADVISOR_NUDGE, PromptInput, system_prompt};
use super::tools::{
    ChartTool, MetricTreeTool, QueryDeps, QueryTool, ReadTool, Rollups, SearchTool,
};
use super::verified::VerifiedQueries;
use crate::events::AnalyticsEvent;
use crate::metric_tree_runner::MetricTreeRunner;
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
    /// The platform's rollups (`BuildContext::preagg`): `query` reads a
    /// covering live one for a `semantic` query. `None` changes nothing.
    pub preagg: Option<agentic_semantic::compile::PreaggContext>,
}

/// What the loop needs to run one analytics question.
pub struct ReactAnalytics {
    pub toolset: Toolset,
    /// The run's results: `query` writes them through the loop's
    /// `ToolCx::results`, and `chart` reads them here, so the loop must be
    /// given this same store (`LoopConfig::results`).
    pub results: Arc<ResultStore>,
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

/// Added with the metric-tree tools. The answer rules say "answer from query
/// results only"; this says a metric-tree result counts, and which tool fits
/// which question.
const METRIC_TREE_RULE: &str = "The metric-tree tools work on the measures the semantic model \
    builds from other measures:\n\
    - For why a measure changed between two periods, call `explain_metric` with both periods \
    as dates, before you compare the periods yourself with `query`.\n\
    - For where a measure can grow, call `find_opportunities`. For what drives a measure, \
    call `metric_sensitivity`. For what a change to a measure would do, call \
    `predict_impact`.\n\
    - You may state the numbers these tools return, as you state query results. They have \
    no result id to cite.";

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
        let verified = Arc::new(VerifiedQueries::from_files(&inputs.files));
        let system_prompt = system_prompt(PromptInput {
            instructions: &inputs.instructions,
            databases: &dbs,
            files: &inputs.files.paths(),
            semantic_only: inputs.semantic_only,
            advisor: inputs.advisor,
            verified: verified.as_slice(),
        });
        let results = Arc::new(ResultStore::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(SearchTool::new(inputs.files.clone())),
            Arc::new(ReadTool::new(inputs.files.clone())),
            Arc::new(QueryTool::new(QueryDeps {
                connectors: inputs.databases,
                catalog: inputs.catalog,
                default_connector: inputs.default_connector,
                semantic_only: inputs.semantic_only,
                verified,
                rollups: inputs.preagg.map(Rollups::new),
            })),
            Arc::new(ChartTool::new(results.clone())),
            Arc::new(AskUser),
        ];
        Self {
            toolset: Toolset {
                system_prompt,
                tools,
            },
            results,
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

    /// Adds the metric-tree tools over `runner` ([`MetricTreeTool::all`]) and
    /// the prompt rule that says when to use them. A host with no runner does
    /// not call this, so the run has neither, as on the FSM.
    pub fn with_metric_tree(mut self, runner: Arc<dyn MetricTreeRunner>) -> Self {
        (self.toolset.tools).extend(MetricTreeTool::all(runner));
        self.toolset.system_prompt.push_str("\n\n");
        self.toolset.system_prompt.push_str(METRIC_TREE_RULE);
        self
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

/// Rows a `result_table` row carries; the table pages the rest.
pub const TABLE_ROWS: usize = 50;

/// The loop's done hook: emits `AnswerResult` and `ResultTable` for the result
/// the answer cites (when its SQL is known) and rewrites the answer text
/// without its markers. A result restored without rows on a resume is read
/// back from the run's stored results ([`ResultStore::load`]); one that is in
/// neither gets no table.
pub struct AnswerFinisher {
    pub results: Arc<ResultStore>,
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
            if let Some(table) = result_table(&self.results, id).await {
                emit(ev, table).await;
            }
        }
        answer.text = strip_citations(&answer.text);
    }
}

/// The `ResultTable` row for result `id`, from memory or the stored result.
async fn result_table(results: &ResultStore, id: &str) -> Option<AnalyticsEvent> {
    let stored = results.load(id).await?;
    Some(AnalyticsEvent::ResultTable {
        result_id: id.to_string(),
        column_kinds: stored.column_kinds.clone(),
        columns: stored.columns.clone(),
        rows: stored.rows.iter().take(TABLE_ROWS).cloned().collect(),
        row_count: stored.row_count,
        truncated: stored.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::react::ResultHandle;
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
        let (ty, _) = rx.recv().await.unwrap();
        assert_eq!(ty, "result_table");
        assert!(rx.recv().await.is_none());
    }

    /// Keeps what it is given, as a host's storage would.
    #[derive(Default)]
    struct Kept(std::sync::Mutex<HashMap<String, agentic_runtime::react::ArchivedResult>>);

    #[async_trait]
    impl agentic_runtime::react::ResultArchive for Kept {
        async fn put(
            &self,
            key: &agentic_runtime::react::ArchiveKey,
            result: &agentic_runtime::react::ArchivedResult,
        ) -> Result<(), agentic_runtime::react::ArchiveError> {
            self.0.lock().unwrap().insert(key.path(), result.clone());
            Ok(())
        }
        async fn get(
            &self,
            key: &agentic_runtime::react::ArchiveKey,
        ) -> Result<
            Option<agentic_runtime::react::ArchivedResult>,
            agentic_runtime::react::ArchiveError,
        > {
            Ok(self.0.lock().unwrap().get(&key.path()).cloned())
        }
    }

    /// A run resumed in another process has the cited result's SQL but not
    /// its rows: the table is read back from the stored result, and carries
    /// the first `TABLE_ROWS` rows, the count, and the truncation flag.
    #[tokio::test]
    async fn a_resumed_answer_tables_its_result_from_storage() {
        let kept: Arc<Kept> = Arc::default();
        let ws = uuid::Uuid::new_v4();
        let run = uuid::Uuid::new_v4().to_string();
        let rows: Vec<Vec<serde_json::Value>> =
            (0..120).map(|i| vec![serde_json::json!(-i)]).collect();
        let first = ResultStore::new();
        first.attach_archive(kept.clone(), ws, &run);
        let id = first.put("select n".into(), vec!["n".into()], rows.clone());
        first.persist(&id, true).await;
        // The upload runs in its own task: wait for it to land, not for a time.
        let landed = async {
            while kept.0.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), landed)
            .await
            .expect("the result was never stored");

        let resumed = ResultStore::new();
        resumed.attach_archive(kept, ws, &run);
        resumed.restore(&id, "select n".into());
        let finisher = AnswerFinisher {
            results: Arc::new(resumed),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut answer = Turn {
            text: format!("Down {{{{result:{id}}}}}."),
            tool_calls: vec![],
            native: serde_json::Value::Null,
            usage: TurnUsage::default(),
        };
        finisher.on_done(&mut answer, &tx).await;
        drop(tx);
        assert_eq!(rx.recv().await.unwrap().0, "answer_result");
        let (ty, table) = rx.recv().await.unwrap();
        assert_eq!(ty, "result_table");
        assert_eq!(table["row_count"], 120);
        assert_eq!(table["truncated"], true);
        assert_eq!(table["column_kinds"], serde_json::json!(["number"]));
        assert_eq!(table["rows"].as_array().unwrap().len(), TABLE_ROWS);
        assert_eq!(table["rows"][3], serde_json::json!([-3]));
    }

    /// A column that turns to text after the rows memory keeps is still
    /// `text`: kinds come from every row the query returned.
    #[tokio::test]
    async fn a_columns_kind_counts_rows_past_what_memory_keeps() {
        let store = ResultStore::new();
        let mut rows: Vec<Vec<serde_json::Value>> = (0..agentic_runtime::react::MEMORY_ROWS)
            .map(|i| vec![serde_json::json!(i)])
            .collect();
        rows.push(vec![serde_json::json!("N/A")]);
        let id = store.put("select n".into(), vec!["n".into()], rows);
        store.persist(&id, false).await;
        let held = store.get(&id).unwrap();
        assert_eq!(held.rows.len(), agentic_runtime::react::MEMORY_ROWS);
        assert_eq!(held.row_count, agentic_runtime::react::MEMORY_ROWS + 1);
        let table = result_table(&store, &id).await.unwrap();
        let AnalyticsEvent::ResultTable { column_kinds, .. } = table else {
            panic!("not a table")
        };
        assert_eq!(column_kinds, vec![agentic_runtime::react::ColumnKind::Text]);
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
