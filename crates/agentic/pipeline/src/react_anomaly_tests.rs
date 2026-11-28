//! The anomaly tools on both react hosts: a served build and a headless run
//! offer them exactly when the platform has an anomaly store and a
//! metric-tree runner (the FSM's executor needs both), and a headless call
//! reaches the store in the platform's workspace. A scripted engine stands in
//! for the model; the host is the metric-tree tests' (`TreeHost`).

use std::sync::{Arc, Mutex};

use agentic_analytics::anomaly_store::{
    AnomalyFilter, AnomalyRecord, AnomalyStore, AnomalyStoreError, DetectAndUpsertResult,
};
use agentic_core::react::Decision;
use serde_json::{Value, json};
use uuid::Uuid;

use super::ReactRun;
use super::metric_tree_tests::{TreeHost, WORKSPACE, headless_on, served, tool_calls};
use super::tests::{call, turn};

const ANOMALY_TOOLS: [&str; 3] = ["list_anomalies", "detect_anomalies", "explain_anomaly"];

/// An empty inbox that records the workspace of every list.
#[derive(Default)]
struct Inbox {
    listed_in: Mutex<Vec<Uuid>>,
}

#[async_trait::async_trait]
impl AnomalyStore for Inbox {
    async fn list(
        &self,
        workspace_id: Uuid,
        _: AnomalyFilter,
    ) -> Result<Vec<AnomalyRecord>, AnomalyStoreError> {
        self.listed_in.lock().unwrap().push(workspace_id);
        Ok(vec![])
    }
    async fn get(&self, _: Uuid, _: Uuid) -> Result<Option<AnomalyRecord>, AnomalyStoreError> {
        Ok(None)
    }
    async fn detect_and_upsert(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        _: &str,
        _: Vec<(String, f64)>,
    ) -> Result<DetectAndUpsertResult, AnomalyStoreError> {
        Err(AnomalyStoreError::Detection(
            "not called in these tests".into(),
        ))
    }
    async fn get_explain_cache(
        &self,
        _: Uuid,
        _: Uuid,
    ) -> Result<Option<Value>, AnomalyStoreError> {
        Ok(None)
    }
    async fn set_explain_cache(&self, _: Uuid, _: Uuid, _: Value) -> Result<(), AnomalyStoreError> {
        Ok(())
    }
}

fn anomaly_tools_in(run: &ReactRun) -> Vec<&'static str> {
    let tools = run.analytics.toolset.tools.iter().map(|t| t.def().name);
    tools.filter(|n| ANOMALY_TOOLS.contains(n)).collect()
}

/// A host with or without a runner, and with an empty [`Inbox`].
fn with_inbox(with_runner: bool, script: Vec<Decision>) -> (TreeHost, Arc<Inbox>) {
    let inbox = Arc::new(Inbox::default());
    let host = TreeHost::new(with_runner, script).with_store(inbox.clone());
    (host, inbox)
}

/// A served build offers the three tools, and says when to use them, only
/// with both a store and a runner. A store alone offers nothing, and a runner
/// alone offers only the tree tools.
#[tokio::test]
async fn a_served_react_run_gets_the_anomaly_tools_only_with_a_store_and_a_runner() {
    let both = served(with_inbox(true, vec![]).0).await;
    assert_eq!(anomaly_tools_in(&both), ANOMALY_TOOLS);
    assert!(
        both.system.contains("`list_anomalies` first"),
        "{}",
        both.system
    );

    let store_only = served(with_inbox(false, vec![]).0).await;
    assert!(anomaly_tools_in(&store_only).is_empty());
    assert!(
        !store_only.system.contains("list_anomalies"),
        "{}",
        store_only.system
    );
    assert!(
        !store_only.system.contains("explain_metric"),
        "{}",
        store_only.system
    );

    let runner_only = served(TreeHost::new(true, vec![])).await;
    assert!(anomaly_tools_in(&runner_only).is_empty());
    assert!(
        !runner_only.system.contains("list_anomalies"),
        "{}",
        runner_only.system
    );
    assert!(
        runner_only.system.contains("explain_metric"),
        "{}",
        runner_only.system
    );
}

/// A headless run (evals, the CLI) calls `list_anomalies` through the
/// platform's store, in the platform's workspace, and the call is the
/// `tool_call` an eval's `expect.tools` reads.
#[tokio::test]
async fn a_headless_react_run_lists_anomalies_in_the_platforms_workspace() {
    let args = json!({
        "measure": "orders.total_order_value",
        "time_dimension": "orders.order_date",
        "granularity": "day",
        "period_start": "2026-06-01",
        "period_end": "2026-06-30",
    });
    let script = vec![
        call("list_anomalies", args),
        Decision::Reply(turn("No anomalies were stored for June 2026.", vec![])),
    ];
    let (host, inbox) = with_inbox(true, script);
    let (answer, events) = headless_on(&host).await;
    let offered = host.offered();
    for name in ANOMALY_TOOLS {
        assert!(offered.contains(&name), "{name} not offered: {offered:?}");
    }
    assert_eq!(answer.unwrap(), "No anomalies were stored for June 2026.");
    assert_eq!(tool_calls(&events), vec!["list_anomalies"]);
    assert_eq!(*inbox.listed_in.lock().unwrap(), vec![WORKSPACE]);
}

/// With a store but no runner, a headless run offers none of the three.
#[tokio::test]
async fn a_headless_react_run_without_a_runner_has_no_anomaly_tools() {
    let script = vec![Decision::Reply(turn("Nothing to check.", vec![]))];
    let (host, inbox) = with_inbox(false, script);
    let (answer, _) = headless_on(&host).await;
    let offered = host.offered();
    for name in ANOMALY_TOOLS {
        assert!(!offered.contains(&name), "{name} offered: {offered:?}");
    }
    assert_eq!(answer.unwrap(), "Nothing to check.");
    assert!(inbox.listed_in.lock().unwrap().is_empty());
}
