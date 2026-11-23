use agentic_core::react::{Engine, EngineError};
use chrono::{TimeZone, Utc};
use serde_json::json;

use super::*;
use crate::react::memory::tests::VecStore;
use crate::react::testkit::{ScriptedEngine, call, call_tools, reply};

fn put(id: &str, text: &str, secs: i64) -> NoteFact {
    NoteFact {
        note_id: id.into(),
        op: NoteOp::Put,
        text: text.into(),
        run_id: "r".into(),
        recorded_at: Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap(),
    }
}

/// `n` notes of about 200 characters: 45 overflow the prompt's caps.
fn store_of(n: usize) -> VecStore {
    let facts = (0..n)
        .map(|i| {
            put(
                &format!("o{i}"),
                &format!("note {i} {}", "x".repeat(190)),
                i as i64,
            )
        })
        .collect();
    VecStore(std::sync::Mutex::new(facts))
}

fn save(notes: serde_json::Value) -> Result<Decision, EngineError> {
    call_tools(vec![call("s1", "save_notes", json!({ "notes": notes }))])
}

fn live(store: &VecStore) -> Vec<String> {
    let facts = store.0.lock().unwrap().clone();
    latest(facts).into_iter().map(|n| n.text).collect()
}

async fn merge(e: &dyn Engine, store: &VecStore) -> Result<Option<MergeReport>, MergeError> {
    merge_notes(e, store, "merge-run").await
}

#[tokio::test]
async fn notes_that_fit_make_no_model_call() {
    let store = store_of(5);
    // Scripted for no calls: one would panic.
    let e = ScriptedEngine::new(vec![]);
    assert!(merge(&e, &store).await.unwrap().is_none());
    assert_eq!(store.0.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn an_overflowing_log_is_merged_in_the_models_order_and_then_fits() {
    let store = store_of(45);
    let e = ScriptedEngine::new(vec![save(json!(["first", " second ", "", "third"]))]);
    let report = merge(&e, &store).await.unwrap().unwrap();
    assert_eq!((report.before, report.after), (45, 3));
    assert_eq!(live(&store), vec!["first", "second", "third"]);

    let asked = e.seen.lock().unwrap()[0].entries.clone();
    let Entry::User(listing) = &asked[0] else {
        panic!("no listing")
    };
    assert!(
        listing.contains("note 0 "),
        "the oldest note, cut from the prompt, is merged too"
    );
    assert!(listing.find("note 44 ").unwrap() < listing.find("note 0 ").unwrap());

    let again = ScriptedEngine::new(vec![]);
    assert!(
        merge(&again, &store).await.unwrap().is_none(),
        "a second merge is a no-op"
    );
}

fn bad_replies() -> Vec<Result<Decision, EngineError>> {
    let long = "y".repeat(MAX_MERGED_NOTE_CHARS + 1);
    let too_many: Vec<String> = (0..41).map(|i| format!("n{i}")).collect();
    vec![
        reply("I merged them"),
        save(json!([])),
        save(json!([long])),
        save(json!(too_many)),
    ]
}

#[tokio::test]
async fn three_replies_that_break_a_cap_or_save_nothing_write_nothing() {
    for i in 0..bad_replies().len() {
        let store = store_of(45);
        let script = (0..3).map(|_| bad_replies().remove(i)).collect();
        let e = ScriptedEngine::new(script);
        let err = merge(&e, &store).await.unwrap_err();
        assert!(matches!(err, MergeError::Invalid(_)), "{err}");
        assert_eq!(store.0.lock().unwrap().len(), 45);
    }
}

#[tokio::test]
async fn a_bad_reply_is_sent_back_with_the_reason_and_the_retry_is_saved() {
    for bad in bad_replies() {
        let store = store_of(45);
        let e = ScriptedEngine::new(vec![bad, save(json!(["fixed"]))]);
        let report = merge(&e, &store).await.unwrap().unwrap();
        assert_eq!(report.after, 1);
        assert_eq!(live(&store), vec!["fixed"]);
        let second = e.seen.lock().unwrap()[1].entries.clone();
        let told = match second.last().unwrap() {
            Entry::ToolResult { text, is_error, .. } => {
                assert!(is_error);
                text.clone()
            }
            Entry::User(text) => text.clone(),
            other => panic!("{other:?}"),
        };
        assert!(told.starts_with("Not saved: "), "{told}");
    }
}

#[tokio::test]
async fn an_engine_error_writes_nothing() {
    let store = store_of(45);
    let e = ScriptedEngine::new(vec![Err(EngineError::Fatal("boom".into()))]);
    assert!(matches!(
        merge(&e, &store).await,
        Err(MergeError::Engine(_))
    ));
    assert_eq!(store.0.lock().unwrap().len(), 45);
}

#[tokio::test]
async fn a_merged_note_may_join_two_facts_past_the_single_note_limit() {
    let store = store_of(45);
    let joined = "z".repeat(MAX_NOTE_CHARS + 50);
    let e = ScriptedEngine::new(vec![save(json!([joined.clone()]))]);
    merge(&e, &store).await.unwrap().unwrap();
    assert_eq!(live(&store), vec![joined]);
}

#[tokio::test]
async fn a_bad_reply_with_two_calls_gets_a_result_for_each() {
    let store = store_of(45);
    let long = "y".repeat(MAX_MERGED_NOTE_CHARS + 1);
    let two = call_tools(vec![
        call("a", "save_notes", json!({ "notes": [long] })),
        call("b", "save_notes", json!({ "notes": ["other"] })),
    ]);
    let e = ScriptedEngine::new(vec![two, save(json!(["fixed"]))]);
    merge(&e, &store).await.unwrap().unwrap();
    let second = e.seen.lock().unwrap()[1].entries.clone();
    let results: Vec<(String, String)> = (second.iter())
        .filter_map(|x| match x {
            Entry::ToolResult {
                call_id,
                text,
                is_error: true,
            } => Some((call_id.clone(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2, "{results:?}");
    assert_eq!(results[0].0, "a");
    assert!(
        results[0].1.starts_with("Not saved: note 1 has"),
        "{}",
        results[0].1
    );
    assert_eq!(results[1].0, "b");
    assert!(results[1].1.starts_with("Ignored"), "{}", results[1].1);
}

#[test]
fn a_plan_still_applies_only_while_its_old_notes_are_unchanged() {
    let note = |id: &str, text: &str| Note {
        note_id: id.into(),
        text: text.into(),
        recorded_at: Utc::now(),
    };
    let plan = MergePlan {
        old: vec![note("a", "alpha"), note("b", "beta")],
        merged: vec![note("m", "merged")],
        usage: TurnUsage::default(),
    };
    let same = [note("b", "beta"), note("a", "alpha")];
    let added = [note("c", "new"), note("b", "beta"), note("a", "alpha")];
    let replaced = [note("b", "beta v2"), note("a", "alpha")];
    let forgotten = [note("a", "alpha")];
    assert!(plan.still_applies(&same));
    assert!(plan.still_applies(&added), "a note added since is kept");
    assert!(!plan.still_applies(&replaced));
    assert!(!plan.still_applies(&forgotten));
    let facts = plan.facts("r");
    assert_eq!(facts.iter().filter(|f| f.op == NoteOp::Delete).count(), 2);
}
