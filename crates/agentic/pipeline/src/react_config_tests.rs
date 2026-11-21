//! What a react build does with each `.agentic.yml` key: the keys it
//! refuses, the ones it honors, and the freshness hint. No connector or model
//! is reached: the refusal runs first.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_analytics::config::{AgentConfig, AgentRuntime, BuildContext};
use agentic_connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, ResultSummary, SqlDialect,
};
use agentic_core::result::{CellValue, QueryResult, QueryRow};

use crate::react_run::build_react_run;
use crate::react_run::load_catalog;
use crate::react_run::tests::{Five, NoBackstop, NoPlatform};

fn refusal(yaml: &str) -> String {
    let config = AgentConfig::from_yaml(&format!("runtime: react\n{yaml}")).unwrap();
    match crate::react_config::refuse_unsupported(&config, &BuildContext::default()) {
        Err(e) => e.to_string(),
        Ok(()) => panic!("expected a refusal for {yaml:?}"),
    }
}

fn refusal_text(key: &str) -> String {
    format!("runtime: react does not support `{key}` yet; remove it or use runtime: fsm")
}

/// A refused key stops the build before it touches a connector or a model:
/// the context has no connector, which would otherwise fail first.
#[tokio::test]
async fn build_refuses_semantic_engine() {
    let yaml = "runtime: react\nsemantic_engine:\n  vendor: cube\n  base_url: http://x\n";
    let config = AgentConfig::from_yaml(yaml).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = build_react_run(&NoPlatform, &config, BuildContext::default(), dir.path()).await
    else {
        panic!("the build must refuse");
    };
    assert!(err.contains(&refusal_text("semantic_engine")), "{err}");
}

#[test]
fn build_refuses_validation() {
    let msg = refusal("validation: {}\n");
    assert_eq!(msg, refusal_text("validation"));
}

#[test]
fn build_refuses_state_instructions() {
    let msg = refusal("states:\n  solving:\n    instructions: be terse\n");
    assert_eq!(msg, refusal_text("states.solving.instructions"));
}

/// No state's `max_retries` reaches react (its loop has its own budgets),
/// `solving`'s included.
#[test]
fn build_refuses_state_max_retries() {
    for state in ["solving", "clarifying"] {
        let msg = refusal(&format!("states:\n  {state}:\n    max_retries: 3\n"));
        assert_eq!(msg, refusal_text(&format!("states.{state}.max_retries")));
    }
}

/// The react loop is the `solving` client alone, so another state's
/// `model` or `thinking` would be ignored: refused, naming the key.
#[test]
fn build_refuses_model_and_thinking_on_a_state_other_than_solving() {
    let msg = refusal("states:\n  clarifying:\n    model: cheap\n");
    assert_eq!(msg, refusal_text("states.clarifying.model"));
    let msg = refusal("states:\n  interpreting:\n    thinking: disabled\n");
    assert_eq!(msg, refusal_text("states.interpreting.thinking"));
    // Sorted state names, then the key order: the same key every time.
    let msg = refusal(
        "states:\n  solving:\n    max_retries: 2\n  clarifying:\n    thinking: disabled\n    \
         model: cheap\n",
    );
    assert_eq!(msg, refusal_text("states.clarifying.thinking"));
}

#[test]
fn build_allows_state_model() {
    let yaml =
        "runtime: react\nstates:\n  solving:\n    model: some-model\n    thinking: disabled\n";
    let config = AgentConfig::from_yaml(yaml).unwrap();
    crate::react_config::refuse_unsupported(&config, &BuildContext::default()).unwrap();
}

/// A connector that answers the freshness probe with one date.
struct Probed;

#[async_trait::async_trait]
impl DatabaseConnector for Probed {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Ok(ExecutionResult {
            result: QueryResult {
                columns: vec!["max".into()],
                rows: vec![QueryRow(vec![CellValue::Text("2026-03-02".into())])],
                total_row_count: 1,
                truncated: false,
            },
            summary: ResultSummary {
                row_count: 1,
                columns: Vec::new(),
            },
        })
    }
}

#[tokio::test]
async fn freshness_hint_follows_the_date_hint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("sales.view.yml"),
        "name: sales\ntable: sales\ndatasource: warehouse\n\
         meta:\n  freshness_watermark_column: [\"day\"]\n  freshness_expected_cadence: [\"daily\"]\n\
         dimensions:\n  - name: day\n    type: date\n    expr: day\n\
         measures:\n  - name: n\n    type: count\n",
    )
    .unwrap();
    let config = AgentConfig::from_yaml("runtime: react\ncontext:\n  - ./*.view.yml\n").unwrap();
    let ctx = BuildContext {
        extra_connectors: HashMap::from([(
            "warehouse".to_string(),
            Arc::new(Probed) as Arc<dyn DatabaseConnector>,
        )]),
        ..BuildContext::default()
    };
    let catalog = load_catalog(&config, &ctx, dir.path()).unwrap();
    let hint = crate::react_config::dynamic_hint(
        "Today is 2026-03-10.".into(),
        "freshness-test",
        catalog.as_deref(),
        &ctx.extra_connectors,
        "warehouse",
    )
    .await;
    assert_eq!(
        hint,
        "Today is 2026-03-10.\nData coverage: sales through 2026-03-02 (daily)."
    );
    // No catalog, no coverage line: the date hint alone.
    let bare = crate::react_config::dynamic_hint(
        "Today is 2026-03-10.".into(),
        "freshness-test",
        None,
        &ctx.extra_connectors,
        "warehouse",
    )
    .await;
    assert_eq!(bare, "Today is 2026-03-10.");
}

/// Every BIRD react config parses and clears the refusal check; the
/// connectors the build also needs are the harness's, not this test's.
#[test]
fn bird_react_agents_build() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../evals/bird");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".react.agentic.yml") {
            continue;
        }
        let yaml = std::fs::read_to_string(&path).unwrap();
        let config = AgentConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(config.runtime, AgentRuntime::React, "{name}");
        crate::react_config::refuse_unsupported(&config, &BuildContext::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        seen += 1;
    }
    assert!(seen >= 4, "found {seen} BIRD react configs");
}

#[test]
fn build_refuses_max_tokens_on_a_provider_that_ignores_it() {
    for vendor in ["openai", "openai_compat"] {
        let msg = refusal(&format!(
            "llm:\n  vendor: {vendor}\n  model: m\n  max_tokens: 900\n"
        ));
        assert_eq!(msg, refusal_text("llm.max_tokens"), "{vendor}");
    }
}

#[test]
fn build_honors_max_tokens_on_anthropic() {
    let yaml = "runtime: react\nllm:\n  vendor: anthropic\n  model: m\n  max_tokens: 900\n";
    let config = AgentConfig::from_yaml(yaml).unwrap();
    let ctx = BuildContext::default();
    crate::react_config::refuse_unsupported(&config, &ctx).unwrap();
    let cfg = crate::react_config::engine_config(&config, &ctx, "Today is x.".into());
    assert_eq!(cfg.max_tokens, Some(900));
}

/// An agent whose databases include one with no database-level read-only
/// path gets no react run: its queries would rest on the gate alone.
#[tokio::test]
async fn build_refuses_a_database_without_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig::from_yaml("runtime: react\n").unwrap();
    let ctx = BuildContext {
        extra_connectors: HashMap::from([
            (
                "wh".to_string(),
                Arc::new(Five) as Arc<dyn DatabaseConnector>,
            ),
            (
                "bq".to_string(),
                Arc::new(NoBackstop) as Arc<dyn DatabaseConnector>,
            ),
            (
                "aa_bq".to_string(),
                Arc::new(NoBackstop) as Arc<dyn DatabaseConnector>,
            ),
        ]),
        ..BuildContext::default()
    };
    let Err(err) = build_react_run(&NoPlatform, &config, ctx, dir.path()).await else {
        panic!("the build must refuse");
    };
    assert_eq!(
        err,
        "runtime: react needs a read-only connection; aa_bq (BigQuery) has none yet"
    );
}

/// A read-only path whose connection refuses every query, as a Postgres
/// superuser login does.
struct NotReady;

#[async_trait::async_trait]
impl DatabaseConnector for NotReady {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn execute_query(&self, _: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        Ok(ExecutionResult::empty())
    }

    fn supports_read_only(&self) -> bool {
        true
    }

    async fn read_only_ready(&self) -> Result<(), ConnectorError> {
        Err(ConnectorError::Other(
            "connect as a least-privilege role".into(),
        ))
    }
}

/// A database whose read-only path would refuse every query refuses the
/// run at its build, naming the database and the reason.
#[tokio::test]
async fn build_refuses_a_database_whose_read_only_path_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig::from_yaml("runtime: react\n").unwrap();
    let ctx = BuildContext {
        extra_connectors: HashMap::from([
            (
                "wh".to_string(),
                Arc::new(Five) as Arc<dyn DatabaseConnector>,
            ),
            (
                "pg".to_string(),
                Arc::new(NotReady) as Arc<dyn DatabaseConnector>,
            ),
        ]),
        ..BuildContext::default()
    };
    let Err(err) = build_react_run(&NoPlatform, &config, ctx, dir.path()).await else {
        panic!("the build must refuse");
    };
    assert_eq!(
        err,
        "runtime: react cannot use pg (PostgreSQL): connect as a least-privilege role"
    );
}

/// `semantic_only` with no semantic model refuses the run at its build,
/// before any model call: the agent could answer nothing.
#[tokio::test]
async fn build_refuses_semantic_only_without_a_semantic_model() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig::from_yaml("runtime: react\nsemantic_only: true\n").unwrap();
    assert!(config.semantic_only);
    let ctx = BuildContext {
        extra_connectors: HashMap::from([(
            "wh".to_string(),
            Arc::new(Five) as Arc<dyn DatabaseConnector>,
        )]),
        ..BuildContext::default()
    };
    let Err(err) = build_react_run(&NoPlatform, &config, ctx, dir.path()).await else {
        panic!("the build must refuse");
    };
    assert!(
        err.starts_with("semantic_only: true needs a semantic model"),
        "{err}"
    );
}
