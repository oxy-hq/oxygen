//! A react run's prior turns are the thread's turns from before it started:
//! a turn that finished while it was parked is not one of them on resume.

use agentic_core::react::Transcript;

use super::*;

/// A new thread row: a run's `thread_id` must name one.
async fn new_thread(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    let insert = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"INSERT INTO threads (id, title, input, output, source, "references")
           VALUES ($1, '', '', '', '', '[]')"#,
        [id.into()],
    );
    db.execute_raw(insert).await.unwrap();
    id
}

/// A finished turn on `thread`, created `offset` (a Postgres interval) from
/// now: an explicit time, so the order never rests on the clock's precision.
async fn finished_turn(db: &DatabaseConnection, ws: Uuid, thread: Uuid, q: &str, offset: &str) {
    let run_id = format!("turn-{}", Uuid::new_v4());
    crud::insert_run(db, &run_id, q, Some(thread), "analytics", None, ws)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE agentic_runs SET task_status = 'done', answer = $2, \
             created_at = now() + interval '{offset}' WHERE id = $1"
        ),
        [run_id.into(), format!("answer to {q}").into()],
    ))
    .await
    .unwrap();
}

fn users(t: &Transcript) -> Vec<String> {
    t.entries
        .iter()
        .filter_map(|e| match e {
            Entry::User(q) => Some(q.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn react_resume_keeps_the_prior_turns_the_run_started_with() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let thread = new_thread(&db).await;
    finished_turn(&db, ws, thread, "earlier?", "-1 hour").await;
    let run_id = queued_chat_on(&db, ws, Some(thread)).await;
    let starter = Host::new(true, vec![ask_user()]);
    latency_pass(&db, ws, &starter).await;
    until(&db, &run_id, "the react run never parked", |r| {
        r.task_status.as_deref() == Some("awaiting_input") && r.driver_id.is_none()
    })
    .await;
    // A turn on the same thread finishes while the run is parked.
    finished_turn(&db, ws, thread, "later?", "1 hour").await;

    queue_answer(&db, &run_id, "2024").await.unwrap();
    let resumer = Host::new(true, vec![Decision::Reply(turn("2024", vec![]))]);
    latency_pass(&db, ws, &resumer).await;
    let run = until(&db, &run_id, "the resumed run never ended", ended).await;
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );

    let want = vec!["earlier?".to_string(), QUESTION.to_string()];
    assert_eq!(users(&starter.seen()[0]), want, "at the start");
    let resumed = resumer.seen();
    assert_eq!(
        users(&resumed[0]),
        want,
        "on resume: no turn from after the start"
    );
}
