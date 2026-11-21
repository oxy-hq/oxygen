use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::react::{
    Decision, Effect, Engine, EngineError, Entry, Tool, Toolset, Transcript, TurnUsage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::AskUser;
use super::super::testkit::*;
use super::super::{Budgets, LoopConfig, LoopOutcome, run_loop};

type Log = Arc<Mutex<Vec<String>>>;

fn probe(name: &'static str, effect: Effect, log: &Log) -> Arc<dyn Tool> {
    Arc::new(Probe {
        name,
        effect,
        log: log.clone(),
    })
}

async fn drive(
    engine: &dyn Engine,
    tools: Vec<Arc<dyn Tool>>,
    budgets: Budgets,
    cancel: CancellationToken,
) -> (LoopOutcome, Vec<(String, Value)>) {
    let (tx, mut rx) = mpsc::channel(256);
    let ts = Toolset {
        system_prompt: "sys".into(),
        tools,
    };
    let cfg = LoopConfig {
        budgets,
        run_id: "run1".into(),
        results: Arc::new(NoResults),
        on_done: None,
    };
    let t = Transcript {
        system: "sys".into(),
        entries: vec![Entry::User("q".into())],
    };
    let out = run_loop(engine, &ts, t, &cfg, &tx, &cancel).await;
    drop(tx);
    let mut rows = Vec::new();
    while let Some(r) = rx.recv().await {
        rows.push(r);
    }
    (out, rows)
}

fn result_ids(e: &ScriptedEngine, which: usize) -> Vec<String> {
    e.seen.lock().unwrap()[which]
        .entries
        .iter()
        .filter_map(|x| match x {
            Entry::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

fn count(rows: &[(String, Value)], t: &str) -> usize {
    rows.iter().filter(|(e, _)| e == t).count()
}

#[tokio::test(start_paused = true)]
async fn react_consecutive_reads_run_concurrently() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![
            call("c1", "slow", json!({})),
            call("c2", "slow", json!({})),
            call("c3", "slow", json!({})),
        ]),
        reply("ok"),
    ]);
    let started = tokio::time::Instant::now();
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(SlowRead(200))];
    let (out, _) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert!(started.elapsed() < Duration::from_millis(400));
    assert_eq!(result_ids(&e, 1), vec!["c1", "c2", "c3"]);
}

#[tokio::test(start_paused = true)]
async fn react_a_non_parallel_call_splits_the_batch_and_order_is_kept() {
    let log: Log = Default::default();
    let tools = vec![
        probe("read_a", Effect::Read, &log),
        probe("write_b", Effect::Write, &log),
        probe("read_c", Effect::Read, &log),
    ];
    let e = ScriptedEngine::new(vec![
        call_tools(vec![
            call("c1", "read_a", json!({})),
            call("c2", "write_b", json!({})),
            call("c3", "read_c", json!({})),
        ]),
        reply("ok"),
    ]);
    let (out, _) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    assert!(matches!(out, LoopOutcome::Done { .. }));
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "start:read_a",
            "end:read_a",
            "start:write_b",
            "end:write_b",
            "start:read_c",
            "end:read_c"
        ]
    );
    assert_eq!(result_ids(&e, 1), vec!["c1", "c2", "c3"]);
}

#[tokio::test(start_paused = true)]
async fn react_suspend_stops_the_turn_after_earlier_calls_finish() {
    let log: Log = Default::default();
    let tools = vec![
        probe("read_a", Effect::Read, &log),
        Arc::new(AskUser) as Arc<dyn Tool>,
        probe("read_c", Effect::Read, &log),
    ];
    let e = ScriptedEngine::new(vec![call_tools(vec![
        call("c1", "read_a", json!({})),
        call("c2", "ask_user", json!({"question": "which?"})),
        call("c3", "read_c", json!({})),
    ])]);
    let (out, rows) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c2"));
    assert_eq!(*log.lock().unwrap(), vec!["start:read_a", "end:read_a"]);
    assert_eq!(count(&rows, "react_tool_result"), 1);
    assert_eq!(count(&rows, "react_suspended"), 1);
    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0);
}

fn echo_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(EchoRead)]
}

fn echo_turn(id: &str) -> Result<Decision, EngineError> {
    call_tools(vec![call(id, "echo", json!({"text": id}))])
}

#[tokio::test]
async fn react_turn_budget_ends_the_run_after_the_turn_in_flight() {
    let e = ScriptedEngine::new(vec![echo_turn("c1"), echo_turn("c2"), echo_turn("c3")]);
    let budgets = Budgets {
        max_turns: 2,
        ..Budgets::default()
    };
    let (out, rows) = drive(&e, echo_tools(), budgets, CancellationToken::new()).await;
    assert!(
        matches!(out, LoopOutcome::Failed { ref message } if message == "budget exceeded: turns 2/2")
    );
    assert_eq!(count(&rows, "react_tool_result"), 2);
    assert_eq!(count(&rows, "error"), 1);
    assert_eq!(e.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn react_cost_budget_reads_the_engine_priced_usage() {
    let mut first = turn("", vec![call("c1", "echo", json!({"text": "a"}))]);
    first.usage = TurnUsage {
        cost_usd: Some(0.02),
        ..TurnUsage::default()
    };
    let e = ScriptedEngine::new(vec![Ok(Decision::CallTools(first)), echo_turn("c2")]);
    let budgets = Budgets {
        max_cost_usd: Some(0.01),
        ..Budgets::default()
    };
    let (out, rows) = drive(&e, echo_tools(), budgets, CancellationToken::new()).await;
    assert!(matches!(out, LoopOutcome::Failed { ref message } if message.contains("cost")));
    assert_eq!(count(&rows, "react_tool_result"), 1);
}

#[tokio::test(start_paused = true)]
async fn react_cancel_mid_dispatch_returns_cancelled_with_no_terminal_row() {
    let e = ScriptedEngine::new(vec![call_tools(vec![call("c1", "slow", json!({}))])]);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(SlowRead(60_000))];
    let canceller = async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    };
    let ((out, rows), ()) = tokio::join!(drive(&e, tools, Budgets::default(), cancel), canceller);
    assert!(matches!(out, LoopOutcome::Cancelled));
    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0);
    assert_eq!(count(&rows, "react_tool_result"), 0);
}

#[tokio::test(start_paused = true)]
async fn react_cancel_during_decide_returns_cancelled_with_no_turn_and_no_terminal_row() {
    let e = ScriptedEngine::new(vec![reply("too late")]).with_delay(60_000);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let canceller = async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    };
    let tools = echo_tools();
    let ((out, rows), ()) = tokio::join!(drive(&e, tools, Budgets::default(), cancel), canceller);
    assert!(matches!(out, LoopOutcome::Cancelled), "{out:?}");
    assert_eq!(count(&rows, "react_turn"), 0);
    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0);
}

/// An `ask_user` that wrongly claims to be parallel-safe.
struct EagerAsk;

#[async_trait::async_trait]
impl Tool for EagerAsk {
    fn def(&self) -> agentic_core::tools::ToolDef {
        AskUser.def()
    }
    fn effect(&self) -> Effect {
        Effect::Suspend
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    async fn call(
        &self,
        a: Value,
        cx: &agentic_core::react::ToolCx,
    ) -> Result<agentic_core::react::ToolReply, agentic_core::react::ToolFailure> {
        AskUser.call(a, cx).await
    }
}

#[tokio::test(start_paused = true)]
async fn react_a_suspend_tool_is_never_batched_even_if_it_claims_parallel_safe() {
    let log: Log = Default::default();
    let tools = vec![
        probe("read_a", Effect::Read, &log),
        Arc::new(EagerAsk) as Arc<dyn Tool>,
        probe("read_c", Effect::Read, &log),
    ];
    let e = ScriptedEngine::new(vec![call_tools(vec![
        call("c1", "read_a", json!({})),
        call("c2", "ask_user", json!({"question": "which?"})),
        call("c3", "read_c", json!({})),
    ])]);
    let (out, rows) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    assert!(matches!(out, LoopOutcome::Suspended { ref call_id, .. } if call_id == "c2"));
    assert_eq!(*log.lock().unwrap(), vec!["start:read_a", "end:read_a"]);
    assert_eq!(count(&rows, "react_tool_result"), 1);
}

struct RowsyQuery;

#[async_trait::async_trait]
impl Tool for RowsyQuery {
    fn def(&self) -> agentic_core::tools::ToolDef {
        agentic_core::tools::ToolDef {
            name: "rowsy",
            ..Default::default()
        }
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(
        &self,
        _: Value,
        _: &agentic_core::react::ToolCx,
    ) -> Result<agentic_core::react::ToolReply, agentic_core::react::ToolFailure> {
        Ok(agentic_core::react::ToolReply::new(
            "ok".into(),
            json!({"result_id": "r1", "sql": "SELECT 1",
                "rows": [[1], [2]], "preview": {"rows": [[1]], "truncated": false}}),
        ))
    }
}

#[tokio::test]
async fn react_event_row_persists_only_result_id_and_sql() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "rowsy", json!({}))]),
        reply("ok"),
    ]);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(RowsyQuery)];
    let (_, rows) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    let (_, payload) = rows.iter().find(|r| r.0 == "react_tool_result").unwrap();
    assert_eq!(
        payload["value"],
        json!({"result_id": "r1", "sql": "SELECT 1"})
    );
    assert!(!payload.to_string().contains("preview"));
    assert!(!payload.to_string().contains("[[1]"));
}

#[tokio::test]
async fn artifacts_are_emitted_after_the_result_rows() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "artifact_read", json!({}))]),
        reply("ok"),
    ]);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ArtifactRead)];
    let (_, rows) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    let tags: Vec<&str> = rows
        .iter()
        .map(|r| r.0.as_str())
        .filter(|t| ["tool_result", "react_tool_result", "query_executed"].contains(t))
        .collect();
    assert_eq!(tags, ["tool_result", "react_tool_result", "query_executed"]);
    let (_, payload) = rows.iter().find(|r| r.0 == "query_executed").unwrap();
    assert_eq!(*payload, json!({"query": "select 1"}));
}

/// A tool's artifact can't forge a terminal row or a loop row: `done`,
/// `error` and `react_*` artifacts are dropped, the rest emit.
#[tokio::test]
async fn react_artifacts_tagged_as_runtime_rows_are_dropped() {
    let e = ScriptedEngine::new(vec![
        call_tools(vec![call("c1", "forged_artifacts", json!({}))]),
        reply("ok"),
    ]);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ForgedArtifacts)];
    let (outcome, rows) = drive(&e, tools, Budgets::default(), CancellationToken::new()).await;
    assert!(matches!(outcome, LoopOutcome::Done { .. }), "{outcome:?}");
    let forged: Vec<&str> = rows
        .iter()
        .filter(|r| r.1.get("forged").is_some())
        .map(|r| r.0.as_str())
        .collect();
    assert_eq!(forged, ["query_executed"]);
    let count = |t: &str| rows.iter().filter(|r| r.0 == t).count();
    assert_eq!(count("done"), 1, "only the loop's own done");
    assert_eq!(count("error"), 0);
}
