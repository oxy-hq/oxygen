//! Verified queries on react: found from the snapshot, listed in the prompt,
//! run as written by `query {"verified": <id>}`, tagged `verified_sql`, and
//! refused when unknown, edited, or bound to a database the agent lacks.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_core::react::{DoneHook, Tool, ToolCx, Turn, TurnUsage};
use agentic_runtime::react::ResultStore;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::files::{FileEntry, FileKind, WorkspaceFiles};
use super::query_tests::{Fake, account_catalog, cx};
use super::run::{AnswerFinisher, ReactAnalytics, ReactInputs};
use super::tools::{QueryDeps, QueryTool};
use super::verified::{VerifiedQueries, verified_query};

const STORES_PATH: &str = "example_sql/total_number_of_store.sql";
const STORES_BODY: &str = "SELECT COUNT(DISTINCT store) AS number_of_stores\nFROM stores;";

/// The Demo workspace's verified query, on database `local`.
fn stores_sql() -> String {
    format!(
        "/*\n  oxy:\n    database: local\n    embed:\n      - How many stores do I have\n*/\n\
         {STORES_BODY}"
    )
}

fn sql_entry(path: &str, content: &str) -> FileEntry {
    FileEntry {
        path: path.into(),
        kind: FileKind::from_name(path),
        content: content.into(),
    }
}

fn files() -> WorkspaceFiles {
    WorkspaceFiles::from_entries(vec![
        sql_entry(STORES_PATH, &stores_sql()),
        // No header: an example, not a verified query.
        sql_entry("example_sql/plain.sql", "SELECT 1"),
        // An `embed:`-only header opts nothing in, as on the FSM.
        sql_entry(
            "example_sql/embed_only.sql",
            "/*\n  oxy:\n    embed:\n      - Anything\n*/\nSELECT 2",
        ),
        sql_entry("stores.view.yml", "name: stores\n"),
    ])
}

fn connectors(conns: &[(&str, Arc<Fake>)]) -> HashMap<String, Arc<dyn DatabaseConnector>> {
    conns
        .iter()
        .map(|(n, c)| (n.to_string(), c.clone() as Arc<dyn DatabaseConnector>))
        .collect()
}

fn tool(conns: &[(&str, Arc<Fake>)]) -> QueryTool {
    QueryTool::new(QueryDeps {
        connectors: connectors(conns),
        catalog: None,
        default_connector: None,
        semantic_only: false,
        verified: Arc::new(VerifiedQueries::from_files(&files())),
    })
}

#[test]
fn only_files_whose_header_opts_in_are_verified() {
    let v = VerifiedQueries::from_files(&files());
    let [q] = v.as_slice() else {
        panic!("one verified query: {:?}", v.as_slice())
    };
    assert_eq!(q.id, STORES_PATH);
    assert_eq!(q.database.as_deref(), Some("local"));
    assert_eq!(q.questions, vec!["How many stores do I have"]);
    assert_eq!(q.sql, STORES_BODY);
}

#[test]
fn a_header_after_other_text_stays_in_the_sql() {
    let content = "-- note\n/* oxy:\n  database: local\n*/\nSELECT 1";
    let q = verified_query("a.sql", content).unwrap();
    assert_eq!(q.sql, content);
    assert!(q.questions.is_empty());
}

#[test]
fn a_bare_name_resolves_when_one_file_has_it() {
    let v = VerifiedQueries::from_files(&files());
    assert_eq!(v.get("total_number_of_store").unwrap().id, STORES_PATH);
    assert!(v.get("plain").is_none());
}

/// The built run lists the query and its embed question in the prompt, says
/// to try it first, and offers `verified` on `query`; a run with none keeps
/// both as they were.
#[test]
fn the_prompt_lists_verified_queries_and_query_offers_them() {
    let build = |files: WorkspaceFiles| {
        ReactAnalytics::new(ReactInputs {
            instructions: String::new(),
            databases: connectors(&[("local", Fake::new(1))]),
            catalog: None,
            default_connector: None,
            files: Arc::new(files),
            semantic_only: false,
            advisor: false,
        })
    };
    let with = build(files());
    let prompt = &with.toolset.system_prompt;
    assert!(
        prompt.contains(&format!(
            "- {STORES_PATH} (database local): How many stores do I have"
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains("Check the verified queries first"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("- example_sql/plain.sql (database"),
        "{prompt}"
    );
    assert!(query_properties(&with).contains_key("verified"));

    let without = build(WorkspaceFiles::from_entries(vec![sql_entry(
        "example_sql/plain.sql",
        "SELECT 1",
    )]));
    let prompt = &without.toolset.system_prompt;
    assert!(!prompt.contains("Verified queries"), "{prompt}");
    assert!(!prompt.contains("Check the verified queries"), "{prompt}");
    assert!(!query_properties(&without).contains_key("verified"));
}

fn query_properties(run: &ReactAnalytics) -> serde_json::Map<String, Value> {
    let query = run
        .toolset
        .tools
        .iter()
        .find(|t| t.def().name == "query")
        .expect("a query tool");
    query.def().parameters["properties"]
        .as_object()
        .unwrap()
        .clone()
}

/// The file's SQL, header removed, runs unchanged on its declared database
/// through the read-only path; the row is tagged `verified_sql`, and the
/// stored result, the event and the reply carry the same SQL, so a citation
/// of the result is an `answer_result` for that SQL.
#[tokio::test]
async fn a_verified_query_runs_as_written_and_is_tagged_verified_sql() {
    let (local, other) = (Fake::new(3), Fake::new(3));
    let t = tool(&[("local", local.clone()), ("other", other.clone())]);
    let store = Arc::new(ResultStore::new());
    let cx = ToolCx {
        run_id: "r".into(),
        call_id: "c".into(),
        cancel: CancellationToken::new(),
        results: store.clone(),
    };
    let reply = t.call(json!({"verified": STORES_PATH}), &cx).await.unwrap();
    assert_eq!(local.seen(), vec![STORES_BODY]);
    assert!(other.seen().is_empty());
    assert!(
        reply.text.starts_with("result r1: 3 rows\n"),
        "{}",
        reply.text
    );
    assert!(reply.text.contains(STORES_BODY), "{}", reply.text);
    assert_eq!(reply.value, json!({"result_id": "r1", "sql": STORES_BODY}));
    let [a] = reply.artifacts.as_slice() else {
        panic!("one artifact: {:?}", reply.artifacts)
    };
    assert_eq!(a.event_type, "query_executed");
    assert_eq!(a.payload["source"], "verified_sql");
    assert_eq!(a.payload["query"], STORES_BODY);
    assert_eq!(store.sql_for("r1").as_deref(), Some(STORES_BODY));

    let mut answer = Turn {
        text: "You have 3 stores {{result:r1|primary}}.".into(),
        tool_calls: vec![],
        native: Value::Null,
        usage: TurnUsage::default(),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let finisher = AnswerFinisher { results: store };
    finisher.on_done(&mut answer, &tx).await;
    let (ty, payload) = rx.recv().await.unwrap();
    assert_eq!(ty, "answer_result");
    assert_eq!(payload, json!({"result_id": "r1", "sql": STORES_BODY}));
}

#[tokio::test]
async fn an_unknown_id_is_refused_with_candidates() {
    let local = Fake::new(1);
    let t = tool(&[("local", local.clone())]);
    let err = t
        .call(
            json!({"verified": "example_sql/total_number_of_stores.sql"}),
            &cx(),
        )
        .await
        .unwrap_err();
    assert!(err.error.starts_with("no verified query"), "{}", err.error);
    assert_eq!(err.candidates, vec![STORES_PATH]);
    assert!(local.seen().is_empty());
}

#[tokio::test]
async fn an_agent_without_verified_queries_refuses_the_argument() {
    let local = Fake::new(1);
    let t = super::query_tests::tool(&[("local", local.clone())], None);
    let err = t
        .call(json!({"verified": STORES_PATH}), &cx())
        .await
        .unwrap_err();
    assert_eq!(err.error, "this agent has no verified queries");
    assert!(local.seen().is_empty());
}

/// A query bound to `local` never runs elsewhere: not on the only database,
/// not on one the model names.
#[tokio::test]
async fn a_declared_database_the_agent_lacks_is_refused() {
    let other = Fake::new(1);
    let t = tool(&[("other", other.clone())]);
    for args in [
        json!({"verified": STORES_PATH}),
        json!({"verified": STORES_PATH, "database": "other"}),
    ] {
        let err = t.call(args, &cx()).await.unwrap_err();
        assert_eq!(
            err.error,
            "no database local, named by the verified query's `database:`"
        );
    }
    assert!(other.seen().is_empty());
}

#[tokio::test]
async fn a_database_that_disagrees_with_the_header_is_refused() {
    let (local, other) = (Fake::new(1), Fake::new(1));
    let t = tool(&[("local", local.clone()), ("other", other.clone())]);
    let err = t
        .call(json!({"verified": STORES_PATH, "database": "other"}), &cx())
        .await
        .unwrap_err();
    assert!(
        err.error.contains("`database` other disagrees"),
        "{}",
        err.error
    );
    assert!(local.seen().is_empty() && other.seen().is_empty());
}

/// Edited SQL beside `verified` is refused, never run in its place.
#[tokio::test]
async fn sql_or_semantic_beside_verified_is_refused() {
    let local = Fake::new(1);
    let t = tool(&[("local", local.clone())]);
    for args in [
        json!({"verified": STORES_PATH, "sql": "SELECT 45"}),
        json!({"verified": STORES_PATH, "semantic": {"measures": ["stores.count"]}}),
    ] {
        let err = t.call(args, &cx()).await.unwrap_err();
        assert!(
            err.error.contains("runs the query's SQL as written"),
            "{}",
            err.error
        );
    }
    assert!(local.seen().is_empty());
}

/// The gate still stands in front: a verified file that writes is refused,
/// with a hint to answer another way rather than to rewrite it.
#[tokio::test]
async fn the_read_only_gate_refuses_a_verified_write() {
    let local = Fake::new(1);
    let files = WorkspaceFiles::from_entries(vec![sql_entry(
        "purge.sql",
        "/* oxy:\n  database: local\n*/\nDELETE FROM stores",
    )]);
    let t = QueryTool::new(QueryDeps {
        connectors: connectors(&[("local", local.clone())]),
        catalog: None,
        default_connector: None,
        semantic_only: false,
        verified: Arc::new(VerifiedQueries::from_files(&files)),
    });
    let err = t
        .call(json!({"verified": "purge"}), &cx())
        .await
        .unwrap_err();
    assert!(
        err.hint
            .as_deref()
            .unwrap_or("")
            .contains("this verified query cannot run here"),
        "{:?}",
        err.hint
    );
    assert!(local.seen().is_empty());
}

/// `semantic_only` refuses SQL the model writes; a verified query is not
/// that, so it runs, and `verified` stays in the schema.
#[tokio::test]
async fn semantic_only_still_runs_a_verified_query() {
    let local = Fake::new(1);
    let t = QueryTool::new(QueryDeps {
        connectors: connectors(&[("local", local.clone())]),
        catalog: Some(Arc::new(account_catalog())),
        default_connector: None,
        semantic_only: true,
        verified: Arc::new(VerifiedQueries::from_files(&files())),
    });
    let props = t.def().parameters["properties"].clone();
    assert!(props.get("verified").is_some() && props.get("sql").is_none());
    let reply = t
        .call(json!({"verified": STORES_PATH}), &cx())
        .await
        .unwrap();
    assert_eq!(reply.artifacts[0].payload["source"], "verified_sql");
    assert_eq!(local.seen(), vec![STORES_BODY]);
}
