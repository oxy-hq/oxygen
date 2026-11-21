//! A react run driven again from a log that already holds an attempt: a
//! start a deploy or a crash re-drives, a resume over a log that already
//! ended, a parked run recovery relaunches, and a rebuild that fails.

use agentic_core::react::ToolCallReq;

use super::*;

fn query(id: &str, sql: &str) -> ToolCallReq {
    ToolCallReq {
        id: id.into(),
        name: "query".into(),
        input: json!({ "sql": sql }),
    }
}

fn done_row(run_id: &str) -> (String, Value) {
    ("done".into(), json!({ "trace_id": run_id }))
}

fn entries_of(t: &agentic_core::react::Transcript, f: impl Fn(&Entry) -> bool) -> usize {
    t.entries.iter().filter(|e| f(e)).count()
}

/// Seeds a queued chat whose first attempt ran a query, then asked for a
/// second and died with it unanswered, and lets `host` re-drive it. Its
/// claim went back to the queue (a rolling deploy releases it; the reaper
/// requeues a crashed one), so the pass starts it again.
async fn redriven(host: &Arc<Host>) -> (DatabaseConnection, String) {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = queued_chat(&db, ws).await;
    let first = [
        ReactEvent::ReactTurn {
            turn: turn("", vec![query("q1", "SELECT 1 AS n")]),
        }
        .to_row(),
        ReactEvent::ReactToolResult {
            call_id: "q1".into(),
            name: "query".into(),
            text: "n\n1".into(),
            is_error: false,
            value: Value::Null,
        }
        .to_row(),
        ReactEvent::ReactTurn {
            turn: turn("", vec![query("q2", "SELECT 2 AS n")]),
        }
        .to_row(),
    ];
    // What Task 8's first attempt stamped before its first react row.
    crud::merge_run_metadata(&db, &run_id, json!({ "runtime": "react" }))
        .await
        .unwrap();
    append(&db, &run_id, &first).await;
    latency_pass(&db, ws, host).await;
    until(&db, &run_id, "the re-driven run never ended", ended).await;
    (db, run_id)
}

/// The start continued the log: the model saw one transcript, the seed once
/// and both turns, with the unanswered query run now and the answered one
/// not re-run; the run is still stamped react and ended with one `done`.
async fn assert_continued(db: &DatabaseConnection, run_id: &str, host: &Host) {
    let run = crud::get_run(db, run_id).await.unwrap().unwrap();
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );
    assert_eq!(run.metadata.unwrap()["runtime"], "react");
    assert_eq!(host.fsm_params.load(Ordering::SeqCst), 0, "not the fsm");
    let seen = host.seen();
    assert_eq!(seen.len(), 1, "one model call, after the pending query");
    let t = &seen[0];
    assert_eq!(entries_of(t, |e| matches!(e, Entry::User(_))), 1, "{t:?}");
    assert_eq!(
        entries_of(t, |e| matches!(e, Entry::Assistant(_))),
        2,
        "{t:?}"
    );
    let results: Vec<&str> = t
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(results, ["q1", "q2"], "{t:?}");
    let rows = rows(db, run_id).await;
    assert_eq!(count(&rows, "done"), 1, "{rows:?}");
    assert_eq!(count(&rows, "error"), 0, "{rows:?}");
    assert_eq!(count(&rows, "react_turn"), 3, "{rows:?}");
    assert_eq!(
        count(&rows, "react_tool_result"),
        2,
        "q1 not re-run: {rows:?}"
    );
    assert_eq!(
        count(&rows, "runtime_selected"),
        0,
        "not started again: {rows:?}"
    );
}

/// A re-driven start finds the react log and continues it.
#[tokio::test(flavor = "multi_thread")]
async fn react_redrive_continues_the_log_it_finds() {
    let host = Host::new(true, vec![reply()]);
    let (db, run_id) = redriven(&host).await;
    assert_continued(&db, &run_id, &host).await;
}

/// The log, not the flag or the agent's key, decides a re-drive: rolled back
/// (flag off, the agent no longer opting in), an in-flight react run still
/// finishes on react and stays stamped `runtime: react`.
#[tokio::test(flavor = "multi_thread")]
async fn react_redrive_ignores_the_flag_and_the_agent_key() {
    let host = Arc::new(Host {
        agent: "databases: [warehouse]\n",
        ..Host::build(false, vec![reply()])
    });
    let (db, run_id) = redriven(&host).await;
    assert_continued(&db, &run_id, &host).await;
}

/// Recovers a run whose `done` reached the log (after a reply citing a
/// result) before its process died, on a host with or without its
/// warehouse, and returns the run row once it ended.
async fn recovered_after_a_recorded_done(
    warehouse: bool,
) -> (
    DatabaseConnection,
    String,
    agentic_runtime::entity::run::Model,
) {
    let db = db().await;
    let ws = Uuid::new_v4();
    let reply = ReactEvent::ReactTurn {
        turn: turn("2024 sold most {{result:r1|primary}}.", vec![]),
    };
    let run_id = seed_react_run(&db, ws, &[reply.to_row()]).await;
    append(&db, &run_id, &[done_row(&run_id)]).await;
    lapse(&db, &run_id).await;

    let host = Arc::new(Host {
        warehouse,
        ..Host::build(true, vec![])
    });
    recover_stranded(&db, ws, &host).await;
    let run = until(&db, &run_id, "the recovered run never ended", ended).await;
    assert!(
        host.seen().is_empty(),
        "no model call after a recorded done"
    );
    (db, run_id, run)
}

/// The recorded end stands, its answer without citation markers, and no
/// row follows the `done`.
async fn assert_recorded_done(
    db: &DatabaseConnection,
    run_id: &str,
    run: &agentic_runtime::entity::run::Model,
) {
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );
    assert_eq!(run.answer.as_deref(), Some("2024 sold most."));
    let rows = rows(db, run_id).await;
    assert_eq!(count(&rows, "done"), 1, "{rows:?}");
    assert_eq!(count(&rows, "error"), 0, "{rows:?}");
    assert_eq!(count(&rows, "answer_result"), 0, "not re-emitted: {rows:?}");
}

/// The run's `done` reached the log, then its process died before the run
/// row said so. Recovery's rebuild now fails (no warehouse), and the
/// recorded end stands: no `error` row after the `done`.
#[tokio::test(flavor = "multi_thread")]
async fn react_rebuild_failure_after_a_recorded_done_writes_nothing() {
    let (db, run_id, run) = recovered_after_a_recorded_done(false).await;
    assert_recorded_done(&db, &run_id, &run).await;
}

/// The same with a rebuild that succeeds: the runtime returns the recorded
/// end without calling the model, and its answer comes back stripped too.
#[tokio::test(flavor = "multi_thread")]
async fn react_recovery_after_a_recorded_done_returns_the_stripped_answer() {
    let (db, run_id, run) = recovered_after_a_recorded_done(true).await;
    assert_recorded_done(&db, &run_id, &run).await;
}

/// A parked run recovery relaunches with no answer (it passes `""`): the
/// question is asked again, and no empty answer is recorded for it.
#[tokio::test(flavor = "multi_thread")]
async fn react_relaunched_parked_run_asks_again() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = parked_react_run(&db, ws).await;
    lapse(&db, &run_id).await;

    let host = Host::new(true, vec![]);
    recover_stranded(&db, ws, &host).await;
    until(&db, &run_id, "the relaunched run never parked again", |r| {
        r.task_status.as_deref() == Some("awaiting_input")
    })
    .await;

    assert!(
        host.seen().is_empty(),
        "the pending ask needs no model call"
    );
    let rows = rows(&db, &run_id).await;
    assert_eq!(count(&rows, "awaiting_input"), 2, "asked again: {rows:?}");
    assert_eq!(count(&rows, "react_suspended"), 2, "{rows:?}");
    assert!(
        !rows
            .iter()
            .any(|(t, p)| t == "react_tool_result" && p["call_id"] == "c1"),
        "no answer recorded for c1: {rows:?}"
    );
    assert_eq!(count(&rows, "done") + count(&rows, "error"), 0, "{rows:?}");
    let checkpoint = crud::get_suspension(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(checkpoint.stage_data["call_id"], "c1");
}

/// An answered react run whose rebuild fails (its warehouse is gone) ends
/// with exactly one `error` row.
#[tokio::test(flavor = "multi_thread")]
async fn react_resume_rebuild_failure_writes_one_error_row() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = parked_react_run(&db, ws).await;
    assert_eq!(
        queue_answer(&db, &run_id, "2024").await.unwrap(),
        QueuedAnswer::Queued
    );
    let host = Arc::new(Host {
        warehouse: false,
        ..Host::build(true, vec![reply()])
    });
    latency_pass(&db, ws, &host).await;
    let run = until(&db, &run_id, "the resumed run never ended", ended).await;

    assert_eq!(run.task_status.as_deref(), Some("failed"));
    let rows = rows(&db, &run_id).await;
    assert_eq!(count(&rows, "error"), 1, "{rows:?}");
    assert_eq!(count(&rows, "done"), 0, "{rows:?}");
    let error = &rows.iter().find(|(t, _)| t == "error").unwrap().1;
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("no databases configured"), "{message}");
    assert!(host.seen().is_empty());
}
