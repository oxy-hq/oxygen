//! `DbMemoryStore` keeps one user's notes in one workspace, in append order.
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(memory_store)'

use agentic_runtime::react::{
    DbMemoryStore, MemoryStore, MergePlan, NoteFact, NoteOp, NoteOwner, latest,
};
use chrono::{TimeZone, Utc};

use crate::integration_tests::test_db;

fn fact(id: &str, op: NoteOp, text: &str, secs: i64) -> NoteFact {
    NoteFact {
        note_id: id.into(),
        op,
        text: text.into(),
        run_id: "run".into(),
        recorded_at: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
    }
}

fn owner() -> NoteOwner {
    NoteOwner {
        workspace_id: uuid::Uuid::new_v4(),
        user_id: uuid::Uuid::new_v4(),
    }
}

fn texts(facts: Vec<NoteFact>) -> Vec<String> {
    latest(facts).into_iter().map(|n| n.text).collect()
}

/// A merge appends many puts with one timestamp, in reverse, so the prompt
/// keeps the model's order: the store must return rows as appended.
#[tokio::test]
async fn memory_store_keeps_append_order_among_equal_times() {
    let Some(db) = test_db().await else {
        return;
    };
    let store = DbMemoryStore::new(db, owner(), None);
    for (id, text) in [("c", "third"), ("b", "second"), ("a", "first")] {
        store.append(fact(id, NoteOp::Put, text, 5)).await.unwrap();
    }
    assert_eq!(
        texts(store.facts().await.unwrap()),
        ["first", "second", "third"]
    );
}

#[tokio::test]
async fn memory_store_replaces_and_deletes_by_note_id() {
    let Some(db) = test_db().await else {
        return;
    };
    let store = DbMemoryStore::new(db, owner(), Some(uuid::Uuid::new_v4()));
    store
        .append(fact("n1", NoteOp::Put, "old", 1))
        .await
        .unwrap();
    store
        .append(fact("n2", NoteOp::Put, "gone", 2))
        .await
        .unwrap();
    store
        .append(fact("n1", NoteOp::Put, "new", 3))
        .await
        .unwrap();
    store
        .append(fact("n2", NoteOp::Delete, "", 4))
        .await
        .unwrap();
    let facts = store.facts().await.unwrap();
    assert_eq!(facts.len(), 4, "every write stays in the log");
    assert_eq!(facts[3].op, NoteOp::Delete);
    assert_eq!(texts(facts), ["new"]);
}

/// Another user in the same workspace, or the same user in another
/// workspace, sees none of the notes.
#[tokio::test]
async fn memory_store_reads_only_its_owners_notes() {
    let Some(db) = test_db().await else {
        return;
    };
    let me = owner();
    let colleague = NoteOwner {
        user_id: uuid::Uuid::new_v4(),
        ..me
    };
    let elsewhere = NoteOwner {
        workspace_id: uuid::Uuid::new_v4(),
        ..me
    };
    let mine = DbMemoryStore::new(db.clone(), me, None);
    mine.append(fact("n1", NoteOp::Put, "mine", 1))
        .await
        .unwrap();
    for other in [colleague, elsewhere] {
        let theirs = DbMemoryStore::new(db.clone(), other, None);
        assert!(theirs.facts().await.unwrap().is_empty(), "{other:?}");
    }
    assert_eq!(texts(mine.facts().await.unwrap()), ["mine"]);
}

fn plan_over(store_notes: Vec<agentic_runtime::react::Note>, merged: &[&str]) -> MergePlan {
    MergePlan {
        old: store_notes,
        merged: (merged.iter().enumerate())
            .map(|(i, t)| agentic_runtime::react::Note {
                note_id: format!("m{i}"),
                text: t.to_string(),
                recorded_at: Utc::now(),
            })
            .collect(),
        usage: Default::default(),
    }
}

/// A merge plans with no lock held; a run that replaces or forgets a note
/// meanwhile makes the apply write nothing, so the merge cannot bring back
/// what the run changed.
#[tokio::test]
async fn memory_store_refuses_a_merge_whose_notes_changed() {
    let Some(db) = test_db().await else {
        return;
    };
    let store = DbMemoryStore::new(db, owner(), None);
    store
        .append(fact("a", NoteOp::Put, "alpha", 1))
        .await
        .unwrap();
    store
        .append(fact("b", NoteOp::Put, "beta", 2))
        .await
        .unwrap();
    let plan = plan_over(latest(store.facts().await.unwrap()), &["merged"]);
    store
        .append(fact("b", NoteOp::Delete, "", 3))
        .await
        .unwrap();

    assert!(!store.apply_merge(&plan, "merge").await.unwrap());
    assert_eq!(
        texts(store.facts().await.unwrap()),
        ["alpha"],
        "nothing written"
    );
}

/// A note added while the merge plans is kept beside the merged list.
#[tokio::test]
async fn memory_store_applies_a_merge_and_keeps_notes_added_since() {
    let Some(db) = test_db().await else {
        return;
    };
    let store = DbMemoryStore::new(db, owner(), None);
    store
        .append(fact("a", NoteOp::Put, "alpha", 1))
        .await
        .unwrap();
    store
        .append(fact("b", NoteOp::Put, "beta", 2))
        .await
        .unwrap();
    let plan = plan_over(latest(store.facts().await.unwrap()), &["first", "second"]);
    store
        .append(fact("c", NoteOp::Put, "new", 3))
        .await
        .unwrap();

    assert!(store.apply_merge(&plan, "merge").await.unwrap());
    let mut live = texts(store.facts().await.unwrap());
    live.sort();
    assert_eq!(live, ["first", "new", "second"]);
}
