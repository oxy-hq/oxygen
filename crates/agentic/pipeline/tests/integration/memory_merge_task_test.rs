//! The served memory merge: a run that leaves the notes overflowing queues
//! one merge per owner, and the merge's apply refuses notes a run changed
//! meanwhile. The engine is scripted: no model call.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(memory_merge_task)'

use std::sync::{Arc, Mutex};

use agentic_core::react::{
    Decision, Engine, EngineError, EventSink, ToolCallReq, Transcript, Turn, TurnUsage,
};
use agentic_core::tools::ToolDef;
use agentic_pipeline::queued_run::memory_merge::{
    MemoryMergeRequest, MergeOutcome, MergeTrigger, merge_with,
};
use agentic_runtime::react::{DbMemoryStore, MemoryStore, NoteFact, NoteOp, NoteOwner, latest};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};

use crate::automation_recovery_test::test_db;

fn owner() -> NoteOwner {
    NoteOwner {
        workspace_id: uuid::Uuid::new_v4(),
        user_id: uuid::Uuid::new_v4(),
    }
}

fn put(id: &str, text: &str) -> NoteFact {
    NoteFact {
        note_id: id.into(),
        op: NoteOp::Put,
        text: text.into(),
        run_id: "run".into(),
        recorded_at: chrono::Utc::now(),
    }
}

/// `n` notes of about 200 characters: 45 overflow the prompt's caps.
async fn seed(db: &DatabaseConnection, owner: NoteOwner, n: usize) -> DbMemoryStore {
    let store = DbMemoryStore::new(db.clone(), owner, None);
    for i in 0..n {
        let text = format!("note {i} {}", "x".repeat(190));
        store.append(put(&format!("o{i}"), &text)).await.unwrap();
    }
    store
}

async fn queued(db: &DatabaseConnection, owner: NoteOwner) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*)::int8 AS n FROM agentic_task_queue \
             WHERE spec->>'kind' = 'react_memory_merge' AND spec->'payload'->>'user_id' = $1",
            [owner.user_id.to_string().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "n").unwrap()
}

fn trigger(db: &DatabaseConnection, owner: NoteOwner) -> MergeTrigger {
    MergeTrigger {
        db: db.clone(),
        owner,
        agent_id: "agents/analytics.agentic.yml".into(),
    }
}

#[tokio::test]
async fn memory_merge_task_is_queued_once_per_owner_when_notes_overflow() {
    let Some(db) = test_db().await else {
        return;
    };
    let (full, fitting) = (owner(), owner());
    seed(&db, full, 45).await;
    seed(&db, fitting, 2).await;
    for run in ["run-1", "run-2"] {
        trigger(&db, full).after_done(run).await;
        trigger(&db, fitting).after_done(run).await;
    }
    assert_eq!(
        queued(&db, full).await,
        1,
        "a second run joins the queued merge"
    );
    assert_eq!(
        queued(&db, fitting).await,
        0,
        "notes that fit queue nothing"
    );
}

/// Saves `notes`; before it answers, runs `meanwhile` (a chat writing).
struct Saver {
    notes: Vec<&'static str>,
    meanwhile: Mutex<Option<NoteFact>>,
    store: Option<Arc<DbMemoryStore>>,
}

#[async_trait]
impl Engine for Saver {
    async fn decide(
        &self,
        _: &Transcript,
        _: &[ToolDef],
        _: &EventSink,
    ) -> Result<Decision, EngineError> {
        let pending = self.meanwhile.lock().unwrap().take();
        if let (Some(fact), Some(store)) = (pending, &self.store) {
            store.append(fact).await.unwrap();
        }
        let call = ToolCallReq {
            id: "s1".into(),
            name: "save_notes".into(),
            input: json!({ "notes": self.notes }),
        };
        Ok(Decision::CallTools(Turn {
            text: String::new(),
            tool_calls: vec![call],
            native: Value::Null,
            usage: TurnUsage::default(),
        }))
    }
}

fn request(owner: NoteOwner) -> MemoryMergeRequest {
    MemoryMergeRequest {
        workspace_id: owner.workspace_id,
        user_id: owner.user_id,
        agent_id: "agents/analytics.agentic.yml".into(),
        source_run_id: "run-1".into(),
    }
}

#[tokio::test]
async fn memory_merge_task_merges_in_the_models_order_then_has_nothing_to_do() {
    let Some(db) = test_db().await else {
        return;
    };
    let me = owner();
    let store = seed(&db, me, 45).await;
    let engine = Saver {
        notes: vec!["first", "second"],
        meanwhile: Mutex::new(None),
        store: None,
    };
    let merged = merge_with(&engine, &db, &request(me)).await.unwrap();
    assert!(matches!(merged, MergeOutcome::Merged(ref r) if (r.before, r.after) == (45, 2)));
    let live: Vec<String> = latest(store.facts().await.unwrap())
        .into_iter()
        .map(|n| n.text)
        .collect();
    assert_eq!(live, ["first", "second"]);
    let again = merge_with(&engine, &db, &request(me)).await.unwrap();
    assert!(matches!(again, MergeOutcome::Fit), "no second model call");
}

/// A chat forgets a note while the model is asked: the merge writes nothing,
/// so it cannot bring the forgotten note back.
#[tokio::test]
async fn memory_merge_task_writes_nothing_when_a_chat_changed_the_notes() {
    let Some(db) = test_db().await else {
        return;
    };
    let me = owner();
    let store = Arc::new(seed(&db, me, 45).await);
    let forget = NoteFact {
        op: NoteOp::Delete,
        ..put("o3", "")
    };
    let engine = Saver {
        notes: vec!["merged"],
        meanwhile: Mutex::new(Some(forget)),
        store: Some(store.clone()),
    };
    let outcome = merge_with(&engine, &db, &request(me)).await.unwrap();
    assert!(matches!(outcome, MergeOutcome::Changed));
    let live = latest(store.facts().await.unwrap());
    assert_eq!(
        live.len(),
        44,
        "the 44 notes left after the forget, nothing merged"
    );
    assert!(live.iter().all(|n| n.note_id != "o3"));
}
