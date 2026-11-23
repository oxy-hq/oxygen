//! `remember` and `forget`: the two writes to a run's note log.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agentic_core::react::{Effect, Tool, ToolCx, ToolFailure, ToolReply};
use agentic_core::tools::ToolDef;
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{MAX_NOTE_CHARS, MemoryStore, Note, NoteFact, NoteOp};

/// The ids a run may replace or forget: its prompt's notes and the ones it
/// wrote. Shared by the two tools of one run.
#[derive(Clone)]
struct Known(Arc<Mutex<HashSet<String>>>);

impl Known {
    fn has(&self, id: &str) -> bool {
        self.0.lock().is_ok_and(|s| s.contains(id))
    }
    fn set(&self, id: &str, live: bool) {
        if let Ok(mut s) = self.0.lock() {
            if live {
                s.insert(id.to_string());
            } else {
                s.remove(id);
            }
        }
    }
}

/// Whether memory calls may run now. The memory tools sit in the run's toolset
/// from its first turn (so the reflection turn keeps the prompt cache), but
/// their calls run only while the reflection turn holds this open.
#[derive(Clone, Debug, Default)]
pub struct MemoryGate(Arc<AtomicBool>);

impl MemoryGate {
    pub(crate) fn set_open(&self, open: bool) {
        self.0.store(open, Ordering::SeqCst);
    }
    fn check(&self, tool: &str) -> Result<(), ToolFailure> {
        if self.0.load(Ordering::SeqCst) {
            return Ok(());
        }
        Err(failure(format!(
            "{tool} runs only when the run asks for notes after the answer; answer the \
             question first"
        )))
    }
}

fn failure(error: impl Into<String>) -> ToolFailure {
    ToolFailure {
        error: error.into(),
        hint: None,
        candidates: Vec::new(),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim)
}

async fn write(store: &dyn MemoryStore, fact: NoteFact) -> Result<(), ToolFailure> {
    store.append(fact).await.map_err(|e| {
        tracing::warn!(error = %e, "react memory write failed");
        failure(format!("could not save: {e}"))
    })
}

fn fact(note_id: String, op: NoteOp, text: String, cx: &ToolCx) -> NoteFact {
    NoteFact {
        note_id,
        op,
        text,
        run_id: cx.run_id.clone(),
        recorded_at: chrono::Utc::now(),
    }
}

pub struct RememberTool {
    store: Arc<dyn MemoryStore>,
    known: Known,
    gate: MemoryGate,
}

pub struct ForgetTool {
    store: Arc<dyn MemoryStore>,
    known: Known,
    gate: MemoryGate,
}

/// The two tools for one run that writes to `store`, knowing `notes`. Their
/// calls run only while `gate` is open.
pub fn memory_tools(
    store: Arc<dyn MemoryStore>,
    notes: &[Note],
    gate: &MemoryGate,
) -> Vec<Arc<dyn Tool>> {
    let ids = notes.iter().map(|n| n.note_id.clone()).collect();
    let known = Known(Arc::new(Mutex::new(ids)));
    vec![
        Arc::new(RememberTool {
            store: store.clone(),
            known: known.clone(),
            gate: gate.clone(),
        }),
        Arc::new(ForgetTool {
            store,
            known,
            gate: gate.clone(),
        }),
    ]
}

#[async_trait]
impl Tool for RememberTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "remember",
            description: "Save one short note for later runs in this workspace: how to answer \
                a kind of question (the view, measure and filter), a query shape that failed \
                and the one that worked, or a correction from the user. One fact per note, at \
                most 400 characters; call it again for another fact. Never a number from a \
                result. `replaces`: the id of a note this one corrects.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "replaces": {"type": "string"}
                },
                "required": ["text"]
            }),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    async fn call(&self, args: Value, cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        self.gate.check("remember")?;
        let text = str_arg(&args, "text").unwrap_or("");
        if text.is_empty() {
            return Err(failure("text is empty"));
        }
        if text.chars().count() > MAX_NOTE_CHARS {
            return Err(failure(format!(
                "a note has at most {MAX_NOTE_CHARS} characters"
            )));
        }
        let id = match str_arg(&args, "replaces").filter(|s| !s.is_empty()) {
            Some(id) if !self.known.has(id) => return Err(failure(format!("no note {id}"))),
            Some(id) => id.to_string(),
            None => format!("n{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        };
        let f = fact(id.clone(), NoteOp::Put, text.to_string(), cx);
        write(&*self.store, f).await?;
        self.known.set(&id, true);
        Ok(ToolReply::new(
            format!("saved {id}"),
            json!({"note_id": id}),
        ))
    }
}

#[async_trait]
impl Tool for ForgetTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "forget",
            description: "Delete a note that is wrong or no longer true, by its id.",
            parameters: json!({
                "type": "object",
                "properties": {"note_id": {"type": "string"}},
                "required": ["note_id"]
            }),
            strict: false,
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    async fn call(&self, args: Value, cx: &ToolCx) -> Result<ToolReply, ToolFailure> {
        self.gate.check("forget")?;
        let id = str_arg(&args, "note_id").unwrap_or("");
        if !self.known.has(id) {
            return Err(failure(format!("no note {id}")));
        }
        write(
            &*self.store,
            fact(id.to_string(), NoteOp::Delete, String::new(), cx),
        )
        .await?;
        self.known.set(id, false);
        Ok(ToolReply::new(
            format!("deleted {id}"),
            json!({"note_id": id}),
        ))
    }
}
