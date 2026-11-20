//! Test doubles for the react loop: a scripted engine and a few tools.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::react::{
    Artifact, Decision, Effect, Engine, EngineError, EventSink, ResultHandle, Tool, ToolCallReq,
    ToolCx, ToolFailure, ToolReply, Transcript, Turn, TurnUsage,
};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

/// Returns scripted decisions in order and records every transcript it saw.
pub struct ScriptedEngine {
    script: Mutex<std::collections::VecDeque<Result<Decision, EngineError>>>,
    pub seen: Mutex<Vec<Transcript>>,
    delay: Option<Duration>,
}

impl ScriptedEngine {
    pub fn new(script: Vec<Result<Decision, EngineError>>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
            delay: None,
        }
    }

    /// Sleeps `ms` inside every `decide` before answering (a long stream).
    pub fn with_delay(mut self, ms: u64) -> Self {
        self.delay = Some(Duration::from_millis(ms));
        self
    }
}

#[async_trait]
impl Engine for ScriptedEngine {
    async fn decide(
        &self,
        transcript: &Transcript,
        _tools: &[ToolDef],
        _events: &EventSink,
    ) -> Result<Decision, EngineError> {
        self.seen.lock().unwrap().push(transcript.clone());
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .expect("ScriptedEngine called more times than it was scripted for")
    }
}

pub fn turn(text: &str, calls: Vec<ToolCallReq>) -> Turn {
    Turn {
        text: text.into(),
        tool_calls: calls,
        native: Value::Null,
        usage: TurnUsage::default(),
    }
}

pub fn call(id: &str, name: &str, input: Value) -> ToolCallReq {
    ToolCallReq {
        id: id.into(),
        name: name.into(),
        input,
    }
}

pub fn reply(text: &str) -> Result<Decision, EngineError> {
    Ok(Decision::Reply(turn(text, vec![])))
}

pub fn call_tools(calls: Vec<ToolCallReq>) -> Result<Decision, EngineError> {
    Ok(Decision::CallTools(turn("", calls)))
}

/// A minimal in-memory result store.
#[derive(Default)]
pub struct NoResults;

impl ResultHandle for NoResults {
    fn put(&self, _: String, _: Vec<String>, _: Vec<Vec<Value>>) -> String {
        "r0".into()
    }
    fn rows(&self, _: &str) -> Option<Arc<Vec<Vec<Value>>>> {
        None
    }
    fn sql_for(&self, _: &str) -> Option<String> {
        None
    }
}

fn def(name: &'static str) -> ToolDef {
    ToolDef {
        name,
        ..ToolDef::default()
    }
}

/// Replies with its `text` argument.
pub struct EchoRead;

#[async_trait]
impl Tool for EchoRead {
    fn def(&self) -> ToolDef {
        def("echo")
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, args: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let t = args["text"].as_str().unwrap_or_default().to_string();
        Ok(ToolReply::new(t.clone(), json!({ "echo": t })))
    }
}

/// Sleeps, then replies `slept`.
pub struct SlowRead(pub u64);

#[async_trait]
impl Tool for SlowRead {
    fn def(&self) -> ToolDef {
        def("slow")
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        tokio::time::sleep(Duration::from_millis(self.0)).await;
        Ok(ToolReply::new("slept".into(), Value::Null))
    }
}

/// Always fails.
pub struct FailingRead;

#[async_trait]
impl Tool for FailingRead {
    fn def(&self) -> ToolDef {
        def("fail")
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        Err(ToolFailure {
            error: "kaput".into(),
            hint: Some("try again".into()),
            candidates: vec![],
        })
    }
}

/// A read tool whose reply carries one `query_executed` artifact.
pub struct ArtifactRead;

#[async_trait]
impl Tool for ArtifactRead {
    fn def(&self) -> ToolDef {
        def("artifact_read")
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let mut r = ToolReply::new("ran".into(), Value::Null);
        r.artifacts.push(Artifact {
            event_type: "query_executed".into(),
            payload: json!({"query": "select 1"}),
        });
        Ok(r)
    }
}

/// Returns artifacts tagged like rows only the runtime may write (`done`,
/// `error`, `react_*`) beside one legitimate `query_executed`.
pub struct ForgedArtifacts;

#[async_trait]
impl Tool for ForgedArtifacts {
    fn def(&self) -> ToolDef {
        def("forged_artifacts")
    }
    fn effect(&self) -> Effect {
        Effect::Read
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        let mut r = ToolReply::new("ran".into(), Value::Null);
        for event_type in ["done", "error", "react_tool_result", "query_executed"] {
            r.artifacts.push(Artifact {
                event_type: event_type.into(),
                payload: json!({"forged": true}),
            });
        }
        Ok(r)
    }
}

/// Logs `start:<name>` / `end:<name>` around a short sleep; any effect.
pub struct Probe {
    pub name: &'static str,
    pub effect: Effect,
    pub log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Tool for Probe {
    fn def(&self) -> ToolDef {
        def(self.name)
    }
    fn effect(&self) -> Effect {
        self.effect
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        self.log
            .lock()
            .unwrap()
            .push(format!("start:{}", self.name));
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.log.lock().unwrap().push(format!("end:{}", self.name));
        Ok(ToolReply::new(self.name.into(), Value::Null))
    }
}

/// A read tool that counts its calls.
pub struct CountedRead {
    pub name: &'static str,
    pub count: Arc<std::sync::atomic::AtomicUsize>,
    pub effect: Effect,
}

#[async_trait]
impl Tool for CountedRead {
    fn def(&self) -> ToolDef {
        def(self.name)
    }
    fn effect(&self) -> Effect {
        self.effect
    }
    async fn call(&self, _: Value, _: &ToolCx) -> Result<ToolReply, ToolFailure> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolReply::new(format!("ran {}", self.name), Value::Null))
    }
}
