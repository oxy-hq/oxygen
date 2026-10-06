//! Where the solver's markdown documents come from.
//!
//! `resolve_context` globs them from `base_dir`. A host that answers for the
//! agent's markdown overrides that through `BuildContext::domain_docs`, and
//! these pin the three cases the override has to keep apart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use async_trait::async_trait;

use super::*;

struct StubConnector;

#[async_trait]
impl DatabaseConnector for StubConnector {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        _sql: &str,
        _limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        Err(ConnectorError::Other("stub".into()))
    }
}

/// A `base_dir` holding one markdown file the agent's glob reaches.
fn base_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxy_domain_docs_{name}_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/glossary.md"), "# from base_dir").unwrap();
    dir
}

async fn built_with(base_dir: &PathBuf, domain_docs: Option<Vec<String>>) -> Vec<String> {
    let config = AgentConfig::from_yaml("context:\n  - ./docs/*.md\n").unwrap();
    let mut connectors: HashMap<String, Arc<dyn DatabaseConnector>> = HashMap::new();
    connectors.insert("warehouse".to_string(), Arc::new(StubConnector));
    let build_ctx = BuildContext {
        extra_connectors: connectors,
        extra_default_connector: Some("warehouse".to_string()),
        domain_docs,
        ..Default::default()
    };
    let (solver, _) = config
        .build_solver_with_context(base_dir, build_ctx)
        .await
        .expect("the solver builds");
    solver.domain_docs
}

/// No host answered: the documents are the ones under `base_dir`, as before.
#[tokio::test]
async fn without_a_host_answer_the_documents_come_from_base_dir() {
    let dir = base_dir("none");
    assert_eq!(built_with(&dir, None).await, ["# from base_dir"]);
    std::fs::remove_dir_all(&dir).ok();
}

/// The host answered: its documents are the agent's, and what `base_dir` holds
/// is not added to them. On a node with the files that would inject each
/// document twice; on one without, `base_dir` has none to add.
#[tokio::test]
async fn a_host_answer_replaces_what_base_dir_holds() {
    let dir = base_dir("some");
    assert_eq!(
        built_with(&dir, Some(vec!["# from the revision".to_string()])).await,
        ["# from the revision"]
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The host answered "none": that is the answer, and `base_dir` is not a
/// second opinion. The agent's definition and its documents come from the
/// same revision, and a file added to disk since is not in it.
#[tokio::test]
async fn an_empty_host_answer_is_still_the_answer() {
    let dir = base_dir("empty");
    assert!(built_with(&dir, Some(vec![])).await.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}
