//! Automation configuration types.
//!
//! These types parse the same YAML format as `oxy::config::model` but are
//! self-contained — no dependency on the oxy core crate. Task types that the
//! orchestrator doesn't inspect (ExecuteSQL, OmniQuery, etc.) are represented
//! as opaque `serde_json::Value`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Automation ────────────────────────────────────────────────────────────────

/// Top-level automation configuration parsed from `.automation.yml` / `.procedure.yml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default)]
    pub name: String,
    pub tasks: Vec<TaskConfig>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub variables: Option<HashMap<String, Value>>,
    pub consistency_prompt: Option<String>,
    /// Model reference for the consistency evaluator (e.g. `"claude-haiku-4-5"`).
    /// Resolved via project `config.yml` model definitions.
    pub consistency_model: Option<String>,
}

// ── Task ────────────────────────────────────────────────────────────────────

/// A single task within an automation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    pub name: String,
    #[serde(flatten)]
    pub task_type: TaskType,
}

/// Automation task types. Variants the orchestrator inspects have typed configs;
/// delegated variants are opaque JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaskType {
    // ── Orchestrator-inspected types ────────────────────────────────────
    #[serde(rename = "agent")]
    Agent(AgentTaskConfig),
    #[serde(rename = "formatter")]
    Formatter(FormatterConfig),
    #[serde(rename = "conditional")]
    Conditional(ConditionalConfig),
    #[serde(rename = "loop_sequential")]
    LoopSequential(LoopConfig),
    #[serde(rename = "workflow")]
    SubAutomation(SubAutomationConfig),
    /// Inspected rather than delegated-opaque: the decider reads it to build
    /// a `TaskSpec::Airway`, reusing the existing airway run path instead of
    /// routing through `step_executor` (which cannot reach a
    /// `DatabaseConnection` and speaks request/response, not streaming).
    #[serde(rename = "airway")]
    Airway(AirwayConfig),

    // ── Delegated types (opaque to orchestrator) ────────────────────────
    #[serde(rename = "execute_sql")]
    ExecuteSql(Value),
    #[serde(rename = "semantic_query")]
    SemanticQuery(Value),
    #[serde(rename = "omni_query")]
    OmniQuery(Value),
    #[serde(rename = "looker_query")]
    LookerQuery(Value),
    #[serde(rename = "http_request")]
    HttpRequest(Value),

    // `type: visualize` was a legacy automation task that ran an LLM to
    // render a chart from the previous step's data. The chat agent's
    // `visualize` *tool* (different surface) covers the same need now,
    // so the task variant is retired. `#[serde(other)]` catches any
    // leftover usage as `Unknown`, which the executor surfaces with a
    // clear "unknown task type" error.
    //
    // The retirement is now complete on both sides: `oxy_core`'s config
    // `TaskType` used to keep a `Visualize` variant, so the validator
    // accepted a task this executor rejected and the generated schemas
    // advertised it to the IDE. Both enums now agree.
    #[serde(other)]
    Unknown,
}

impl TaskType {
    /// Canonical kebab-case identifier matching the YAML `type:` discriminator
    /// — also what the frontend keys off of when rendering per-task content.
    pub fn name(&self) -> &'static str {
        match self {
            TaskType::Agent(_) => "agent",
            TaskType::Formatter(_) => "formatter",
            TaskType::Conditional(_) => "conditional",
            TaskType::LoopSequential(_) => "loop_sequential",
            TaskType::SubAutomation(_) => "workflow",
            TaskType::ExecuteSql(_) => "execute_sql",
            TaskType::SemanticQuery(_) => "semantic_query",
            TaskType::OmniQuery(_) => "omni_query",
            TaskType::LookerQuery(_) => "looker_query",
            TaskType::HttpRequest(_) => "http_request",
            TaskType::Airway(_) => "airway",
            TaskType::Unknown => "unknown",
        }
    }
}

// ── Inner task configs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskConfig {
    pub agent_ref: String,
    pub prompt: String,
    #[serde(default = "default_one")]
    pub consistency_run: usize,
    #[serde(default)]
    pub retry: usize,
    pub variables: Option<HashMap<String, Value>>,
    pub consistency_prompt: Option<String>,
    /// Model reference for the consistency evaluator (overrides automation-level).
    pub consistency_model: Option<String>,
    /// Output-shaping switch for the agent. Default is
    /// `AgentOutputMode::Answer` — the analytics agent runs the
    /// full pipeline and produces a natural-language answer.
    /// `AgentOutputMode::Sql` terminates after the agent generates
    /// SQL: pre-validated paths (semantic-model, verified `.sql`
    /// files, vendor engines) skip execution entirely; LLM-generated
    /// SQL runs a `LIMIT 0` smoke check before terminating. The
    /// terminal answer is the SQL text. Applies to **analytics agents**
    /// (`.agentic.yml`); ignored for the built-in builder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<AgentOutputConfig>,
}

/// Output shaping for an agent task.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentOutputConfig {
    #[serde(default)]
    pub mode: AgentOutputMode,
}

/// Output mode for an analytics agent task.
///
/// `Answer` (the default) runs the full analytics FSM
/// (clarifying -> specifying -> solving -> executing -> interpreting)
/// and emits a natural-language answer.
///
/// `Sql` shortcuts the FSM after the SQL is produced: pre-validated
/// paths (semantic-model compile, verified `.sql` file match, vendor
/// engine) skip the executing state entirely; LLM-generated SQL runs
/// a `LIMIT 0` smoke check. Automation delegation is incoherent with
/// SQL-gen mode (the SQL is only known after the automation runs) and
/// is rejected at runtime when this mode is in effect.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutputMode {
    #[default]
    Answer,
    Sql,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterConfig {
    pub template: String,
}

/// Config for an `airway` automation step. Mirrors
/// `oxy_core::config::model::AirwayTask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirwayConfig {
    /// Workspace-relative path to the `.airway.yml` pipeline spec.
    pub pipeline: String,
    /// Explicit subset of the spec's resources to run. `None`/empty runs all.
    #[serde(default)]
    pub resources: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAutomationConfig {
    pub src: PathBuf,
    pub variables: Option<HashMap<String, Value>>,
    /// Child automation's tasks, pre-resolved at automation load time.
    ///
    /// Populated by [`crate::resolve::resolve_sub_automations`] before the
    /// run starts so the decider can emit the full nested task DAG in
    /// `subrun_started` without doing async file IO at decide-time.
    /// Persisted as part of `AutomationRunState.workflow` so resumes see
    /// the same tree without re-resolving. Empty when the child file
    /// is missing, fails to parse, or appears in a sub-automation cycle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_tasks: Vec<TaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Loop values: either a JSON array or a Jinja2 template string.
    pub values: Value,
    pub tasks: Vec<TaskConfig>,
    #[serde(default = "default_one")]
    pub concurrency: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalConfig {
    pub conditions: Vec<ConditionBranch>,
    #[serde(default, rename = "else")]
    pub else_tasks: Option<Vec<TaskConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionBranch {
    #[serde(rename = "if")]
    pub condition: String,
    pub tasks: Vec<TaskConfig>,
}

// ── Semantic query config (re-exported from agentic-semantic) ──────────────
//
// These types now live in `agentic_semantic::config` so the analytics
// domain can construct them without taking a dep on `agentic-automation`.
// Re-exported here to keep existing call sites working.

pub use agentic_semantic::config::{
    ArrayFilter, DateRangeFilter, ScalarFilter, SemanticFilter, SemanticFilterType, SemanticOrder,
    SemanticQueryConfig, TimeDimensionConfig, TimeGranularity,
};

// ── Defaults ────────────────────────────────────────────────────────────────

fn default_one() -> usize {
    1
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airway_step_parses_and_keeps_its_wire_tag() {
        // The wire tag has to match `oxy_core::config::model::TaskType`'s
        // `#[serde(rename = "airway")]` exactly — the two enums are separate
        // types parsing the same YAML, and a mismatch would only surface at
        // run time as an `Unknown` step (a "unknown task type" failure).
        let yaml = r#"
name: ingest_then_rollup
tasks:
  - name: ingest
    type: airway
    pipeline: pipelines/restaurant_analytics.airway.yml
  - name: sales_rollup
    type: execute_sql
    database: pokehouse
    sql_file: rollups/sales_daily_metrics.sql
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tasks.len(), 2);

        match &config.tasks[0].task_type {
            TaskType::Airway(cfg) => {
                assert_eq!(cfg.pipeline, "pipelines/restaurant_analytics.airway.yml");
                assert!(cfg.resources.is_none(), "omitted `resources` runs all");
            }
            other => panic!("expected an Airway step, got {}", other.name()),
        }
        assert_eq!(config.tasks[0].task_type.name(), "airway");
    }

    #[test]
    fn airway_step_accepts_an_explicit_resource_subset() {
        let yaml = r#"
name: partial
tasks:
  - name: ingest
    type: airway
    pipeline: p.airway.yml
    resources: [orders, order_checks]
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.tasks[0].task_type {
            TaskType::Airway(cfg) => {
                assert_eq!(
                    cfg.resources.as_deref(),
                    Some(&["orders".to_string(), "order_checks".to_string()][..])
                );
            }
            other => panic!("expected an Airway step, got {}", other.name()),
        }
    }

    #[test]
    fn retired_visualize_step_falls_through_to_unknown() {
        // `type: visualize` was retired as a task (the chat agent's
        // `visualize` *tool* replaced it). It must keep parsing — no
        // `deny_unknown_fields` — and land on `Unknown`, which the executor
        // rejects with a clear message rather than silently no-op'ing.
        let yaml = r#"
name: legacy
tasks:
  - name: chart
    type: visualize
    prompt: "plot it"
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(config.tasks[0].task_type, TaskType::Unknown));
        assert_eq!(config.tasks[0].task_type.name(), "unknown");
    }

    #[test]
    fn test_parse_simple_automation() {
        let yaml = r#"
name: test_automation
tasks:
  - name: query_data
    type: execute_sql
    database: my_db
    sql_query: "SELECT * FROM orders"
  - name: summarize
    type: formatter
    template: "Total: {{ query_data }}"
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "test_automation");
        assert_eq!(config.tasks.len(), 2);
        assert_eq!(config.tasks[0].name, "query_data");
        assert!(matches!(config.tasks[0].task_type, TaskType::ExecuteSql(_)));
        assert_eq!(config.tasks[1].name, "summarize");
        assert!(matches!(config.tasks[1].task_type, TaskType::Formatter(_)));
    }

    #[test]
    fn test_parse_agent_task() {
        let yaml = r#"
name: agent_step
tasks:
  - name: analyze
    type: agent
    agent_ref: agents/default.agentic.yml
    prompt: "Analyze the data"
    consistency_run: 3
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        let TaskType::Agent(agent) = &config.tasks[0].task_type else {
            panic!("expected Agent");
        };
        assert_eq!(agent.agent_ref, "agents/default.agentic.yml");
        assert_eq!(agent.prompt, "Analyze the data");
        assert_eq!(agent.consistency_run, 3);
    }

    #[test]
    fn test_parse_loop_task() {
        let yaml = r#"
name: loop_test
tasks:
  - name: per_item
    type: loop_sequential
    values: [apple, banana, cherry]
    concurrency: 2
    tasks:
      - name: detail
        type: execute_sql
        database: db
        sql_query: "SELECT 1"
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        let TaskType::LoopSequential(loop_cfg) = &config.tasks[0].task_type else {
            panic!("expected LoopSequential");
        };
        assert_eq!(loop_cfg.concurrency, 2);
        assert_eq!(loop_cfg.tasks.len(), 1);
        assert!(loop_cfg.values.is_array());
    }

    #[test]
    fn test_parse_sub_automation() {
        let yaml = r#"
name: parent
tasks:
  - name: child
    type: workflow
    src: procedures/child.procedure.yml
    variables:
      fruit: apple
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        let TaskType::SubAutomation(wf) = &config.tasks[0].task_type else {
            panic!("expected SubAutomation");
        };
        assert_eq!(wf.src.to_str().unwrap(), "procedures/child.procedure.yml");
        assert!(wf.variables.is_some());
    }

    #[test]
    fn test_unknown_task_type() {
        let yaml = r#"
name: test
tasks:
  - name: mystery
    type: future_task_type
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(config.tasks[0].task_type, TaskType::Unknown));
    }

    /// `export:` and `cache:` wrote into the workspace directory and are
    /// refused at compile (`oxy-compile`). A revision compiled before that
    /// still holds them; they must not stop the run from loading, and
    /// nothing reads them.
    #[test]
    fn test_retired_export_and_cache_keys_are_ignored_on_load() {
        let yaml = r#"
name: test
tasks:
  - name: sql
    type: agent
    agent_ref: agents/sql.agentic.yml
    prompt: "generate SQL"
    cache:
      enabled: true
      path: "out/cache/{{ groupings.value }}.sql"
    export:
      path: out/x.csv
      format: csv
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(config.tasks[0].task_type, TaskType::Agent(_)));
        let reserialised = serde_json::to_value(&config.tasks[0]).unwrap();
        assert!(reserialised.get("cache").is_none());
        assert!(reserialised.get("export").is_none());
    }

    /// `output: { mode: sql }` round-trips through the agent task
    /// config so the analytics pipeline can shortcut the FSM after
    /// SQL is produced.
    #[test]
    fn test_parse_agent_output_sql_mode() {
        let yaml = r#"
name: test
tasks:
  - name: gen_sql
    type: agent
    agent_ref: agents/sales.agentic.yml
    prompt: "{{ question }}"
    output:
      mode: sql
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        let TaskType::Agent(agent) = &config.tasks[0].task_type else {
            panic!("expected agent task");
        };
        let output = agent.output.as_ref().expect("output parsed");
        assert_eq!(output.mode, AgentOutputMode::Sql);
    }

    /// Default output mode is `Answer` — existing automations without an
    /// `output:` block keep their natural-language interpretation.
    #[test]
    fn test_agent_output_mode_defaults_to_answer() {
        let yaml = r#"
name: test
tasks:
  - name: ask
    type: agent
    agent_ref: a.yml
    prompt: "x"
"#;
        let config: AutomationConfig = serde_yaml::from_str(yaml).unwrap();
        let TaskType::Agent(agent) = &config.tasks[0].task_type else {
            panic!("expected agent task");
        };
        assert!(agent.output.is_none());
    }
}
