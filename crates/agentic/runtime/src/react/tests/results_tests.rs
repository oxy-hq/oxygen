use agentic_core::react::{Entry, ResultHandle, Transcript};
use serde_json::{Value, json};

use super::super::events::ReactEvent;
use super::super::{ResultStore, rebuild};

fn rows(n: usize) -> Vec<Vec<Value>> {
    (0..n).map(|i| vec![json!(i)]).collect()
}

#[test]
fn react_ids_are_sequential_per_run() {
    let s = ResultStore::new();
    assert_eq!(s.put("a".into(), vec![], vec![]), "r1");
    assert_eq!(s.put("b".into(), vec![], vec![]), "r2");
    assert_eq!(ResultStore::new().put("c".into(), vec![], vec![]), "r1");
}

#[test]
fn react_preview_caps_rows_and_states_truncation() {
    let s = ResultStore::new();
    let id = s.put("q".into(), vec!["n".into()], rows(250));
    let p = s.preview(&id, 100).unwrap();
    assert_eq!((p.rows.len(), p.row_count, p.truncated), (100, 250, true));
    let all = s.preview(&id, 250).unwrap();
    assert!(!all.truncated);
    assert!(s.preview("nope", 5).is_none());
}

#[test]
fn react_restore_never_lets_put_reuse_an_id() {
    let s = ResultStore::new();
    s.restore("r3", "select 3".into());
    assert_eq!(s.put("x".into(), vec![], vec![]), "r4");
    assert!(s.get("r3").is_none());
}

#[test]
fn react_rebuild_restores_known_ids_without_rows() {
    let value = json!({"result_id": "r1", "sql": "select 1"});
    let log = vec![
        ReactEvent::ReactToolResult {
            call_id: "c1".into(),
            name: "query".into(),
            text: "ok".into(),
            is_error: false,
            value,
        }
        .to_row(),
    ];
    let seed = Transcript {
        system: "s".into(),
        entries: vec![Entry::User("q".into())],
    };
    let store = ResultStore::new();
    rebuild(seed, &log, Some(&store as &dyn ResultHandle)).unwrap();
    assert!(store.get("r1").is_none());
    assert_eq!(store.sql_for("r1").as_deref(), Some("select 1"));
    assert_eq!(store.put("n".into(), vec![], vec![]), "r2");
}
