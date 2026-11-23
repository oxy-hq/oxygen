//! The react runtime on the served path, driven by a scripted engine through
//! the same `ExecutingTask` the coordinator persists: what it forwards is
//! what lands in `agentic_run_events`.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_analytics::config::{AgentConfig, BuildContext};
use agentic_connector::DatabaseConnector;
use agentic_core::delegation::{SuspendReason, TaskOutcome};
use agentic_core::react::{Artifact, Decision, Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use serde_json::{Value, json};

use super::*;
use crate::react_run::tests::{NoBackstop, NoPlatform, call, react_run, turn};

/// [`NoPlatform`] with the `react_runtime` flag set to `.0`.
struct Flagged(bool);

#[async_trait::async_trait]
impl crate::platform::ProjectContext for Flagged {
    async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _: Option<&str>,
        _: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _: &str) -> Option<String> {
        None
    }
    async fn react_runtime_enabled(&self) -> bool {
        self.0
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for Flagged {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(&self, _: &str) -> Result<String, crate::WorkspaceReadError> {
        Err("unused".into())
    }
}

const RUN_ID: &str = "run-1";

/// Serves `built` and drives it through `into_executing_task`, returning
/// every forwarded row and the first outcome.
async fn drive(built: Result<ReactRun, String>) -> (Vec<(String, Value)>, TaskOutcome) {
    let handle = serve(built, RUN_ID.into(), Vec::new(), "q".into());
    let started = StartedPipeline {
        run_id: RUN_ID.into(),
        source_type: "analytics".into(),
        inner: ErasedHandle::React(handle),
        starting_driver: None,
    };
    let (mut task, _bridge) = started.into_executing_task();
    let outcome = task.outcomes.recv().await.expect("an outcome");
    let mut rows = Vec::new();
    while let Some(row) = task.events.recv().await {
        rows.push(row);
    }
    (rows, outcome)
}

fn count(rows: &[(String, Value)], t: &str) -> usize {
    rows.iter().filter(|(et, _)| et == t).count()
}

/// The org's flag is the only switch: flag off, every agent runs on the FSM;
/// flag on, a served run starts on react (`runtime_selected`, then the loop's
/// `react_turn`) whatever the agent's `runtime:` key says, and its run row
/// says so.
#[tokio::test]
async fn react_start_follows_the_flag_alone() {
    assert!(!picks_served(&Flagged(false), "a", true).await);
    assert!(
        !picks_served(&NoPlatform, "a", true).await,
        "the trait default is off"
    );
    assert!(picks_served(&Flagged(true), "a", true).await);
    assert!(
        !picks_served(&Flagged(true), "a", false).await,
        "a run that cannot put a question to a person stays on the fsm"
    );

    let dir = tempfile::tempdir().unwrap();
    let script = vec![Decision::Reply(turn("hello", vec![]))];
    let (rows, outcome) = drive(Ok(react_run(script, dir.path()))).await;
    let types: Vec<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(types.first(), Some(&"runtime_selected"));
    assert!(types.contains(&"react_turn"), "rows: {types:?}");
    assert_eq!(count(&rows, "done"), 1);
    assert!(
        matches!(outcome, TaskOutcome::Done { ref answer, metadata: None } if answer == "hello")
    );

    let mut metadata = json!({ "agent_id": "a" });
    stamp_runtime(&mut metadata, &runtime_patch(true, false).unwrap());
    assert_eq!(metadata, json!({ "agent_id": "a", "runtime": "react" }));
    assert_eq!(
        runtime_patch(false, false),
        None,
        "a fresh fsm run carries no key"
    );
    assert_eq!(
        runtime_patch(false, true),
        Some(json!({ "runtime": "fsm" })),
        "a re-driven row is re-stamped"
    );
}

/// A suspend writes the FSM's `awaiting_input` row and no terminal row, and
/// records what a resume routes on: `from_state: react` and the parked call.
#[tokio::test]
async fn react_suspend_writes_awaiting_input() {
    let dir = tempfile::tempdir().unwrap();
    let ask = json!({"question": "Which year?", "suggestions": ["2025", "2026"]});
    let (rows, outcome) = drive(Ok(react_run(vec![call("ask_user", ask)], dir.path()))).await;

    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0);
    let waiting: Vec<&Value> = rows
        .iter()
        .filter(|(t, _)| t == "awaiting_input")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(waiting.len(), 1, "one awaiting_input row");
    assert_eq!(
        waiting[0]["questions"],
        json!([{ "prompt": "Which year?", "suggestions": ["2025", "2026"] }])
    );
    assert_eq!(waiting[0]["trace_id"], RUN_ID);

    let TaskOutcome::Suspended {
        reason: SuspendReason::HumanInput { questions },
        resume_data,
        trace_id,
    } = outcome
    else {
        panic!("expected a human-input suspend, got {outcome:?}");
    };
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].prompt, "Which year?");
    assert_eq!(resume_data.from_state, "react");
    assert_eq!(resume_data.stage_data["call_id"], "c1");
    assert_eq!(resume_data.stage_data["run_id"], RUN_ID);
    assert_eq!(resume_data.original_input, "q");
    assert_eq!(trace_id, RUN_ID);

    let mut metadata = json!({});
    stamp_runtime(&mut metadata, &runtime_patch(true, false).unwrap());
    assert_eq!(metadata["runtime"], "react");
}

/// A build the runtime refuses (a database with no read-only path) still
/// ends the run with exactly one `error` row, so the stream closes.
#[tokio::test]
async fn react_build_failure_writes_an_error_row() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig::from_yaml("runtime: react\n").unwrap();
    let ctx = BuildContext {
        extra_connectors: HashMap::from([(
            "bq".to_string(),
            Arc::new(NoBackstop) as Arc<dyn DatabaseConnector>,
        )]),
        ..BuildContext::default()
    };
    let built = build_react_run(&NoPlatform, &config, ctx, dir.path()).await;
    assert!(built.is_err(), "the build must refuse");

    let (rows, outcome) = drive(built).await;
    let message = "runtime: react needs a read-only connection; bq (BigQuery) has none yet";
    assert_eq!(count(&rows, "error"), 1, "rows: {rows:?}");
    assert_eq!(count(&rows, "done"), 0);
    let error = &rows.iter().find(|(t, _)| t == "error").unwrap().1;
    assert_eq!(error["message"], message);
    assert_eq!(error["trace_id"], RUN_ID);
    assert!(matches!(outcome, TaskOutcome::Failed(ref m) if m == message));
}

/// A tool that asks for rows only the runtime may write, and one it may.
struct Forger;

#[async_trait::async_trait]
impl Tool for Forger {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "forge".into(),
            ..ToolDef::default()
        }
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let artifact = |t: &str| Artifact {
            event_type: t.into(),
            payload: json!({ "forged": true, "trace_id": "x", "message": "x" }),
        };
        let mut reply = ToolReply::new("ok".into(), Value::Null);
        reply.artifacts = ["done", "error", "react_turn", "forge_note"]
            .into_iter()
            .map(artifact)
            .collect();
        Ok(reply)
    }
}

/// The served forward is row-for-row, and the runtime's reserved-tag guard
/// still holds on it: a tool cannot end the run or forge a `react_*` row.
#[tokio::test]
async fn served_forward_drops_a_reserved_artifact_tag() {
    let dir = tempfile::tempdir().unwrap();
    let script = vec![
        call("forge", json!({})),
        Decision::Reply(turn("fine", vec![])),
    ];
    let mut run = react_run(script, dir.path());
    run.analytics.toolset.tools.push(Arc::new(Forger));
    let (rows, outcome) = drive(Ok(run)).await;

    assert_eq!(count(&rows, "error"), 0, "rows: {rows:?}");
    assert_eq!(count(&rows, "done"), 1);
    assert_eq!(rows.last().map(|(t, _)| t.as_str()), Some("done"));
    assert_eq!(count(&rows, "react_turn"), 2, "one per model turn");
    let reserved = |t: &str| t == "done" || t == "error" || t.starts_with("react_");
    assert!(
        !rows.iter().any(|(t, p)| p["forged"] == true && reserved(t)),
        "no forged reserved row"
    );
    assert_eq!(count(&rows, "forge_note"), 1, "an allowed artifact passes");
    assert!(matches!(outcome, TaskOutcome::Done { .. }));
}

fn inputs(thinking_mode: ThinkingMode) -> ServedInputs {
    ServedInputs {
        agent_id: "a".into(),
        workspace_id: uuid::Uuid::nil(),
        connectors: HashMap::new(),
        databases: Vec::new(),
        project_model: None,
        domain_docs: None,
        thinking_mode,
    }
}

/// An extended-thinking served chat runs react on the agent's
/// `llm.extended_thinking` model and thinking, as the FSM does.
#[test]
fn react_build_context_follows_the_extended_thinking_toggle() {
    let config = AgentConfig::from_yaml(
        "runtime: react\nllm:\n  model: base\n  extended_thinking:\n    model: deep\n    \
         thinking: disabled\n",
    )
    .unwrap();
    let ctx = build_context(&NoPlatform, &config, inputs(ThinkingMode::ExtendedThinking));
    assert_eq!(ctx.model_override.as_deref(), Some("deep"));
    let thinking = ctx.thinking_override.as_ref().map(|t| format!("{t:?}"));
    assert_eq!(thinking.as_deref(), Some("Disabled"));
    let ctx = build_context(&NoPlatform, &config, inputs(ThinkingMode::Auto));
    assert!(ctx.model_override.is_none() && ctx.thinking_override.is_none());
}

/// With the flag the only switch, an agent the react build would refuse
/// falls back to the FSM instead of failing, for every refusal the build
/// makes: no database (a brief agent), a database with no read-only path, a
/// read-only path whose login role it refuses, and a key react does not
/// honor. An agent react can serve has no reason.
#[tokio::test]
async fn an_agent_react_cannot_serve_runs_on_the_fsm() {
    use crate::react_config::tests::NotReady;
    use crate::react_run::tests::Five;
    let with = |dbs: Vec<(&str, Arc<dyn DatabaseConnector>)>| ServedInputs {
        connectors: dbs.into_iter().map(|(n, c)| (n.to_string(), c)).collect(),
        ..inputs(ThinkingMode::Auto)
    };
    async fn why(config: &AgentConfig, inputs: &ServedInputs) -> Option<String> {
        let check = ServeCheck {
            platform: &NoPlatform,
            config,
            inputs,
        };
        cannot_serve(check).await
    }
    let plain = || AgentConfig::from_yaml("instructions: hi\n").unwrap();
    let five = || Arc::new(Five) as Arc<dyn DatabaseConnector>;
    assert_eq!(why(&plain(), &with(vec![("pg", five())])).await, None);
    let cases: Vec<(AgentConfig, ServedInputs, &str)> = vec![
        (plain(), with(vec![]), "no database"),
        (
            plain(),
            with(vec![("pg", five()), ("wh", Arc::new(NoBackstop))]),
            "wh",
        ),
        (
            plain(),
            with(vec![("pg", Arc::new(NotReady))]),
            "least-privilege",
        ),
        (
            AgentConfig::from_yaml("validation: {}\n").unwrap(),
            with(vec![("pg", five())]),
            "validation",
        ),
    ];
    for (config, inputs, expected) in cases {
        let reason = why(&config, &inputs)
            .await
            .unwrap_or_else(|| panic!("{expected}: no reason"));
        assert!(
            reason.to_lowercase().contains(&expected.to_lowercase()),
            "{expected}: {reason}"
        );
    }
}
