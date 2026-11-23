//! A warehouse whose pre-built connector has no read-only path, but which the
//! platform can give a read-only connector for (an `airhouse_managed` one over
//! a read-only credential): the start runs react on the replacement, and so
//! does the resume after an `ask_user` answer, which rebuilds the run.

use super::*;

fn airhouse_like(script: Vec<Decision>) -> Arc<Host> {
    Arc::new(Host {
        airhouse_like: true,
        ..Host::build(true, script)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn react_starts_and_resumes_on_the_read_only_replacement() {
    let db = db().await;
    let ws = Uuid::new_v4();
    let run_id = queued_chat(&db, ws).await;
    let starter = airhouse_like(vec![ask_user()]);
    latency_pass(&db, ws, &starter).await;
    until(&db, &run_id, "the react run never parked", |r| {
        r.task_status.as_deref() == Some("awaiting_input") && r.driver_id.is_none()
    })
    .await;
    assert_eq!(
        starter.engines_built.load(Ordering::SeqCst),
        1,
        "started on react"
    );
    let row = crud::get_run(&db, &run_id).await.unwrap().unwrap();
    assert_eq!(row.metadata.unwrap()["runtime"], "react");

    queue_answer(&db, &run_id, "2024").await.unwrap();
    let resumer = airhouse_like(vec![Decision::Reply(turn("2024 sold most", vec![]))]);
    latency_pass(&db, ws, &resumer).await;
    until(&db, &run_id, "the resumed run never ended", ended).await;
    assert_resumed_with_the_answer(&db, &run_id, &resumer).await;
}
