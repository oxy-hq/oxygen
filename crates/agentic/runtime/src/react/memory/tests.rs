use std::sync::{Arc, Mutex};

use agentic_core::react::{ToolCx, ToolReply};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::react::testkit::NoResults;

/// An in-memory note log.
#[derive(Default)]
pub(crate) struct VecStore(pub Mutex<Vec<NoteFact>>);

#[async_trait]
impl MemoryStore for VecStore {
    async fn facts(&self) -> Result<Vec<NoteFact>, MemoryError> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn append(&self, fact: NoteFact) -> Result<(), MemoryError> {
        self.0.lock().unwrap().push(fact);
        Ok(())
    }
}

fn fact(id: &str, op: NoteOp, text: &str, secs: i64) -> NoteFact {
    NoteFact {
        note_id: id.into(),
        op,
        text: text.into(),
        run_id: "r".into(),
        recorded_at: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
    }
}

fn note(id: &str, text: &str) -> Note {
    Note {
        note_id: id.into(),
        text: text.into(),
        recorded_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

#[test]
fn latest_keeps_the_last_put_per_id_newest_first_and_drops_deletes() {
    let facts = vec![
        fact("a", NoteOp::Put, "a1", 1),
        fact("b", NoteOp::Put, "b1", 2),
        fact("a", NoteOp::Put, "a2", 3),
        fact("c", NoteOp::Put, "c1", 4),
        fact("c", NoteOp::Delete, "", 5),
    ];
    let notes = latest(facts);
    let got: Vec<(&str, &str)> = (notes.iter())
        .map(|n| (n.note_id.as_str(), n.text.as_str()))
        .collect();
    assert_eq!(got, vec![("a", "a2"), ("b", "b1")]);
}

#[test]
fn a_put_after_a_delete_brings_the_note_back() {
    let notes = latest(vec![
        fact("a", NoteOp::Put, "a1", 1),
        fact("a", NoteOp::Delete, "", 2),
        fact("a", NoteOp::Put, "a3", 3),
    ]);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].text, "a3");
}

#[test]
fn notes_section_lists_ids_and_is_none_when_empty() {
    assert_eq!(notes_section(&[]), None);
    let s = notes_section(&[note("n1", "use account.view for balances")]).unwrap();
    assert!(s.starts_with("What you learned in earlier runs"), "{s}");
    assert!(s.contains("- [n1] use account.view for balances\n"), "{s}");
}

#[test]
fn notes_section_caps_count_and_size() {
    let many: Vec<Note> = (0..100).map(|i| note(&format!("n{i}"), "short")).collect();
    let s = notes_section(&many).unwrap();
    assert_eq!(s.matches("- [").count(), MAX_PROMPT_NOTES);
    assert!(s.contains("[n0]") && !s.contains(&format!("[n{MAX_PROMPT_NOTES}]")));
    let long = "x".repeat(MAX_NOTE_CHARS);
    let big: Vec<Note> = (0..40).map(|i| note(&format!("n{i}"), &long)).collect();
    let s = notes_section(&big).unwrap();
    assert!(s.len() <= MAX_PROMPT_CHARS, "{}", s.len());
    assert!(s.contains("[n0]"));
}

#[tokio::test]
async fn file_store_round_trips_facts_and_a_missing_file_is_empty() {
    let path = std::env::temp_dir().join(format!("oxy-mem-{}.jsonl", uuid::Uuid::new_v4()));
    let store = FileMemoryStore::new(&path);
    assert!(store.facts().await.unwrap().is_empty());
    let (a, b) = (
        fact("a", NoteOp::Put, "a1", 1),
        fact("a", NoteOp::Delete, "", 2),
    );
    store.append(a.clone()).await.unwrap();
    store.append(b.clone()).await.unwrap();
    assert_eq!(store.facts().await.unwrap(), vec![a, b]);
    std::fs::remove_file(&path).unwrap();
}

fn open_gate() -> MemoryGate {
    let gate = MemoryGate::default();
    gate.set_open(true);
    gate
}

#[tokio::test]
async fn a_shut_gate_refuses_both_tools_and_writes_nothing() {
    let store = Arc::new(VecStore::default());
    let tools = memory_tools(store.clone(), &[note("n1", "old")], &MemoryGate::default());
    let err = call(&*tools[0], json!({"text": "x"})).await.unwrap_err();
    assert!(
        err.starts_with("remember runs only when the run asks for notes"),
        "{err}"
    );
    assert!(call(&*tools[1], json!({"note_id": "n1"})).await.is_err());
    assert!(store.0.lock().unwrap().is_empty());
}

fn cx() -> ToolCx {
    ToolCx {
        run_id: "run7".into(),
        call_id: "c1".into(),
        cancel: CancellationToken::new(),
        results: Arc::new(NoResults),
    }
}

async fn call(
    tool: &dyn agentic_core::react::Tool,
    args: serde_json::Value,
) -> Result<ToolReply, String> {
    tool.call(args, &cx()).await.map_err(|f| f.error)
}

#[tokio::test]
async fn remember_appends_a_put_with_a_new_id_and_the_run_id() {
    let store = Arc::new(VecStore::default());
    let tools = memory_tools(store.clone(), &[], &open_gate());
    let reply = call(&*tools[0], json!({"text": " use district.a11 for salary "}))
        .await
        .unwrap();
    let id = reply.value["note_id"].as_str().unwrap().to_string();
    assert!(id.starts_with('n') && id.len() == 9, "{id}");
    let facts = store.0.lock().unwrap().clone();
    assert_eq!(facts.len(), 1);
    assert_eq!(
        (facts[0].op, facts[0].text.as_str()),
        (NoteOp::Put, "use district.a11 for salary")
    );
    assert_eq!(
        (facts[0].note_id.as_str(), facts[0].run_id.as_str()),
        (id.as_str(), "run7")
    );
}

#[tokio::test]
async fn remember_replaces_a_known_note_and_refuses_an_unknown_one() {
    let store = Arc::new(VecStore::default());
    let tools = memory_tools(store.clone(), &[note("n1", "old")], &open_gate());
    let reply = call(&*tools[0], json!({"text": "new", "replaces": "n1"}))
        .await
        .unwrap();
    assert_eq!(reply.value["note_id"], "n1");
    let err = call(&*tools[0], json!({"text": "x", "replaces": "n9"}))
        .await
        .unwrap_err();
    assert_eq!(err, "no note n9");
    assert_eq!(latest(store.0.lock().unwrap().clone())[0].text, "new");
}

#[tokio::test]
async fn remember_refuses_empty_and_too_long_text() {
    let store = Arc::new(VecStore::default());
    let tools = memory_tools(store.clone(), &[], &open_gate());
    assert_eq!(
        call(&*tools[0], json!({"text": "  "})).await.unwrap_err(),
        "text is empty"
    );
    let long = "x".repeat(MAX_NOTE_CHARS + 1);
    assert!(call(&*tools[0], json!({ "text": long })).await.is_err());
    assert!(store.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forget_deletes_a_known_note_once_and_knows_notes_this_run_wrote() {
    let store = Arc::new(VecStore::default());
    let tools = memory_tools(store.clone(), &[note("n1", "old")], &open_gate());
    call(&*tools[1], json!({"note_id": "n1"})).await.unwrap();
    assert_eq!(
        call(&*tools[1], json!({"note_id": "n1"}))
            .await
            .unwrap_err(),
        "no note n1"
    );
    let id = call(&*tools[0], json!({"text": "fresh"}))
        .await
        .unwrap()
        .value["note_id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&*tools[1], json!({ "note_id": id })).await.unwrap();
    assert!(latest(store.0.lock().unwrap().clone()).is_empty());
}

#[test]
fn the_remember_description_and_the_prompt_state_the_length_limit() {
    let tools = memory_tools(Arc::new(VecStore::default()), &[], &open_gate());
    let limit = format!("at most {MAX_NOTE_CHARS} characters");
    assert!(tools[0].def().description.contains(&limit));
    assert!(REFLECT_PROMPT.contains(&limit));
}

#[test]
fn both_memory_tools_are_writes() {
    let tools = memory_tools(Arc::new(VecStore::default()), &[], &open_gate());
    let names: Vec<&str> = tools.iter().map(|t| t.def().name).collect();
    assert_eq!(names, vec!["remember", "forget"]);
    assert!(
        tools
            .iter()
            .all(|t| t.effect() == agentic_core::react::Effect::Write)
    );
}
