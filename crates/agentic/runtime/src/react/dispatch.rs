//! Runs one turn's tool calls: consecutive parallel-safe calls concurrently,
//! everything else one at a time, results in call order.

use std::time::Instant;

use agentic_core::events::CoreEvent;
use agentic_core::react::{
    Artifact, Effect, Entry, EventSink, Tool, ToolCallReq, ToolCx, ToolFailure, ToolReply, Toolset,
};
use futures::future::join_all;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::driver::LoopConfig;
use super::events::{ReactEvent, emit_core, emit_react, send_row};
use super::results::persistable_value;

/// Everything a dispatch needs besides the toolset and the calls.
pub struct DispatchCx<'a> {
    pub cfg: &'a LoopConfig,
    pub ev: &'a EventSink,
    pub cancel: &'a CancellationToken,
}

/// A `Suspend` tool's question; the run parks on `call_id`.
#[derive(Debug, Clone)]
pub struct SuspendInfo {
    pub call_id: String,
    pub question: String,
    pub suggestions: Vec<String>,
}

/// What one turn's dispatch produced: the results recorded before any suspend
/// (in call order) and the suspend, if one stopped the turn.
#[derive(Debug, Default)]
pub struct Dispatched {
    pub entries: Vec<Entry>,
    pub suspended: Option<SuspendInfo>,
}

type CallOutcome = Result<(ToolReply, Effect), ToolFailure>;

pub async fn dispatch_turn(
    toolset: &Toolset,
    calls: &[ToolCallReq],
    cx: DispatchCx<'_>,
) -> Dispatched {
    let mut out = Dispatched::default();
    for batch in batches(toolset, calls) {
        // The first suspend ends the turn: nothing after it runs.
        for call in batch {
            // A suspend's ToolCall row has no ToolResult: the call stays open
            // while the run is parked, which is how the UI shows "waiting".
            emit_call(cx.ev, call).await;
        }
        let started = Instant::now();
        let outcomes = join_all(batch.iter().map(|c| call_one(toolset, c, &cx))).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        for (call, mut outcome) in batch.iter().zip(outcomes) {
            if let Ok((reply, Effect::Suspend)) = &outcome {
                out.suspended = Some(suspend_info(call, reply));
                let call_id = call.id.clone();
                emit_react(cx.ev, ReactEvent::ReactSuspended { call_id }).await;
                return out;
            }
            let artifacts = take_artifacts(&mut outcome);
            out.entries
                .push(record(cx.ev, call, outcome, duration_ms).await);
            for a in artifacts.into_iter().filter(artifact_allowed) {
                send_row(cx.ev, (a.event_type, a.payload)).await;
            }
        }
    }
    out
}

/// The artifacts of a successful call, taken so they emit after its result rows.
fn take_artifacts(o: &mut CallOutcome) -> Vec<Artifact> {
    o.as_mut()
        .map(|(r, _)| std::mem::take(&mut r.artifacts))
        .unwrap_or_default()
}

/// An artifact may not pose as a row only the runtime writes: a `done` or
/// `error` would end the run twice, a `react_*` would forge the loop's own
/// record. Such an artifact is dropped, with a warning.
fn artifact_allowed(a: &Artifact) -> bool {
    let t = a.event_type.as_str();
    let reserved = t == "done" || t == "error" || t.starts_with("react_");
    if reserved {
        tracing::warn!(
            event_type = t,
            "tool artifact uses a reserved event type; dropped"
        );
    }
    !reserved
}

/// Maximal runs of consecutive parallel-safe calls; every other call alone.
fn batches<'a>(toolset: &Toolset, calls: &'a [ToolCallReq]) -> Vec<&'a [ToolCallReq]> {
    // A `Suspend` tool is never batched, whatever its `parallel_safe` says:
    // the batch would drop its batch-mates' results.
    let safe = |c: &ToolCallReq| {
        find(toolset, &c.name).is_some_and(|t| t.parallel_safe() && t.effect() != Effect::Suspend)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < calls.len() {
        let mut j = i + 1;
        if safe(&calls[i]) {
            while j < calls.len() && safe(&calls[j]) {
                j += 1;
            }
        }
        out.push(&calls[i..j]);
        i = j;
    }
    out
}

pub(super) fn find<'a>(toolset: &'a Toolset, name: &str) -> Option<&'a dyn Tool> {
    toolset
        .tools
        .iter()
        .find(|t| t.def().name == name)
        .map(|t| t.as_ref())
}

async fn emit_call(ev: &EventSink, call: &ToolCallReq) {
    let e = CoreEvent::ToolCall {
        name: call.name.clone(),
        input: call.input.to_string(),
        llm_duration_ms: 0,
        sub_spec_index: None,
    };
    emit_core(ev, e).await;
}

/// Emits the result rows for one call and returns its transcript entry.
async fn record(ev: &EventSink, call: &ToolCallReq, o: CallOutcome, duration_ms: u64) -> Entry {
    let (text, is_error, value) = match o {
        Ok((r, _)) => (r.text, false, persistable_value(&r.value)),
        Err(f) => (f.to_model_text(), true, Value::Null),
    };
    record_with_value(ev, call, (text, is_error, value), duration_ms).await
}

/// Emits the result rows for a call whose result text is already known (a
/// resume's answer) and returns its transcript entry.
pub(super) async fn record_text(
    ev: &EventSink,
    call: &ToolCallReq,
    text: String,
    is_error: bool,
    duration_ms: u64,
) -> Entry {
    record_with_value(ev, call, (text, is_error, Value::Null), duration_ms).await
}

/// As [`record_text`], also persisting the `{result_id, sql}` subset of the tool's value on the
/// `react_tool_result` row (it carries `result_id` + `sql`, see `ResultStore`).
async fn record_with_value(
    ev: &EventSink,
    call: &ToolCallReq,
    (text, is_error, value): (String, bool, Value),
    duration_ms: u64,
) -> Entry {
    let e = CoreEvent::ToolResult {
        name: call.name.clone(),
        output: text.clone(),
        duration_ms,
        is_error,
        sub_spec_index: None,
    };
    emit_core(ev, e).await;
    let row = ReactEvent::ReactToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        text: text.clone(),
        is_error,
        value,
    };
    emit_react(ev, row).await;
    Entry::ToolResult {
        call_id: call.id.clone(),
        text,
        is_error,
    }
}

/// Looks the tool up and runs it; unknown names are a [`ToolFailure`].
async fn call_one(toolset: &Toolset, call: &ToolCallReq, cx: &DispatchCx<'_>) -> CallOutcome {
    let Some(tool) = find(toolset, &call.name) else {
        return Err(ToolFailure {
            error: format!("unknown tool `{}`", call.name),
            hint: None,
            candidates: toolset
                .tools
                .iter()
                .map(|t| t.def().name.to_string())
                .collect(),
        });
    };
    let tcx = ToolCx {
        run_id: cx.cfg.run_id.clone(),
        call_id: call.id.clone(),
        cancel: cx.cancel.clone(),
        results: cx.cfg.results.clone(),
    };
    let reply = tool.call(call.input.clone(), &tcx).await?;
    Ok((reply, tool.effect()))
}

/// The suspend from a `Suspend` tool's reply value (see `Effect::Suspend`).
fn suspend_info(call: &ToolCallReq, reply: &ToolReply) -> SuspendInfo {
    let question = reply
        .value
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or(&reply.text)
        .to_string();
    let suggestions = reply
        .value
        .get("suggestions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    SuspendInfo {
        call_id: call.id.clone(),
        question,
        suggestions,
    }
}
